//! `--bench`: the window's frame loop without a window, for measurements. Each frame advances the game by real time
//! (the 60 Hz steps, as in play), prepares and draws the frame offscreen as the window would (no presentation, no read
//! back) and waits for the GPU, turning the view slowly. Prints frame times, where they go (the step's phases, the
//! frame's preparation, the renderer, the GPU) and memory; `--csv FILE` writes one line per frame.

use dx_play::Session;
use std::collections::BTreeMap;
use std::time::Instant;

struct Frame {
    total: f64,
    steps: usize,
    step: f64,
    prep: f64,
    render: f64,
    gpu: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Resident and peak resident memory of this process in MiB (`/proc/self/status`).
fn rss_mib() -> (f64, f64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| s.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0;
    (kb("VmRSS:"), kb("VmHWM:"))
}

pub fn run(session: &mut Session, size: (u32, u32), seconds: f64, turn_deg_per_s: f32, csv: Option<String>, profile: Option<String>) -> Result<(), String> {
    // the step's phases, gathered without printing
    unsafe { std::env::set_var("DX_STEP_PROFILE", "quiet") };
    let renderer = session.make_vk_renderer(&[])?;
    println!("Vulkan device: {}; {}x{}; {seconds} s, turning {turn_deg_per_s} deg/s", renderer.cx.device_name, size.0, size.1);
    // as the window: the recording on a thread of its own (`DX_RENDER_THREAD=0`: on this one)
    let threaded = std::env::var("DX_RENDER_THREAD").map(|v| v != "0").unwrap_or(true);
    let mut rt = dx_vk::render_thread::RenderThread::new(renderer, threaded);
    println!("render thread {}", if rt.threaded() { "on" } else { "off" });
    let at = session.player_location();
    println!("player at {:.1},{:.1},{:.1}", at[0], at[1], at[2]);
    let mut prep = Vec::new();
    // `DX_BENCH_FREEZE`: the game stands still (the warm-up takes ten fixed steps of 1/60 s, then none): the same scene
    // in every frame and every run, for comparing builds
    let freeze = std::env::var_os("DX_BENCH_FREEZE").is_some();
    // warm up: textures, meshes and caches of the first views
    let mut last = Instant::now();
    let warm = Instant::now();
    let mut warm_frames = 0;
    while warm.elapsed().as_secs_f64() < 2.0 {
        let now = Instant::now();
        if !freeze {
            session.advance(now.duration_since(last).as_secs_f32());
        } else if warm_frames < 10 {
            session.advance(1.0 / 60.0);
        }
        warm_frames += 1;
        last = now;
        session.render_vk_bench_thread(&mut rt, size, &mut prep)?;
    }
    let _ = Session::take_step_profile();
    let (rss0, _) = rss_mib();
    let mut frames: Vec<Frame> = Vec::new();
    let mut prep_sum: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut render_sum: BTreeMap<&'static str, f64> = BTreeMap::new();
    // `DX_BENCH_YAW`: the view's yaw at the start in degrees, `save` the loaded game's (0 otherwise)
    let mut yaw = match std::env::var("DX_BENCH_YAW").ok() {
        Some(v) if v == "save" => (session.yaw & 0xffff) as f32 * 360.0 / 65536.0,
        v => v.and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0),
    };
    let profiler = start_profile(profile.is_some())?;
    // `DX_BENCH_CPU`: the CPU side measured steadily (the frame rate varies with what else the machine runs): from the
    // loaded game (with `DX_BENCH_FREEZE`, the same state in every build), 240 game steps of 1/60 s, then 200 visibility
    // walks of the view; medians and means. Then the run as usual.
    if std::env::var_os("DX_BENCH_CPU").is_some() {
        let stat = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.total_cmp(b));
            (v[v.len() / 2], v.iter().sum::<f64>() / v.len() as f64)
        };
        let mut steps = Vec::new();
        for _ in 0..240 {
            let t = Instant::now();
            session.advance(1.0 / 60.0);
            steps.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let cam = session.render_camera();
        let (w, h) = (size.0 as i32, size.1 as i32);
        let mut walks = Vec::new();
        for _ in 0..200 {
            let t = Instant::now();
            let _ = session.compute_visibility(&cam, w, h);
            walks.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let ((sm, sa), (wm, wa)) = (stat(&mut steps), stat(&mut walks));
        println!("cpu: step median {sm:.3} ms (mean {sa:.3}), walk median {wm:.3} ms (mean {wa:.3})");
    }
    let start = Instant::now();
    last = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let t0 = Instant::now();
        let dt = t0.duration_since(last).as_secs_f32();
        last = t0;
        yaw += turn_deg_per_s * dt;
        session.set_yaw(yaw);
        let steps = if freeze { 0 } else { session.advance(dt) };
        let step = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        // (the renderer's phases and GPU time are the frame before's)
        let (cpu_ms, gpu_ms) = session.render_vk_bench_thread(&mut rt, size, &mut prep)?;
        let draw = t1.elapsed().as_secs_f64() * 1000.0;
        let prep_ms: f64 = prep.iter().map(|p| p.1).sum();
        for &(k, v) in &prep {
            *prep_sum.entry(k).or_default() += v;
        }
        for &(k, v) in &cpu_ms {
            *render_sum.entry(k).or_default() += v;
        }
        frames.push(Frame { total: t0.elapsed().as_secs_f64() * 1000.0, steps, step, prep: prep_ms, render: draw - prep_ms, gpu: gpu_ms.unwrap_or(0.0) });
    }
    let wall = start.elapsed().as_secs_f64();
    if let Some(path) = &profile {
        write_profile(profiler, path)?;
    }
    let n = frames.len() as f64;
    let steps: usize = frames.iter().map(|f| f.steps).sum();
    let mut totals: Vec<f64> = frames.iter().map(|f| f.total).collect();
    totals.sort_by(|a, b| a.total_cmp(b));
    let avg = |f: &dyn Fn(&Frame) -> f64| frames.iter().map(f).sum::<f64>() / n;
    let with_step: Vec<&Frame> = frames.iter().filter(|f| f.steps > 0).collect();
    let without: Vec<&Frame> = frames.iter().filter(|f| f.steps == 0).collect();
    let mean = |v: &[&Frame], f: &dyn Fn(&Frame) -> f64| if v.is_empty() { 0.0 } else { v.iter().map(|x| f(x)).sum::<f64>() / v.len() as f64 };
    println!("\n{} frames in {wall:.1} s: {:.0} fps average; {steps} game steps ({:.1}/s)", frames.len(), n / wall, steps as f64 / wall);
    println!(
        "frame ms: mean {:.2}, median {:.2}, p90 {:.2}, p99 {:.2}, max {:.2}",
        avg(&|f| f.total),
        percentile(&totals, 0.5),
        percentile(&totals, 0.9),
        percentile(&totals, 0.99),
        percentile(&totals, 1.0)
    );
    println!(
        "frames with a step: {} (mean {:.2} ms: step {:.2}); without: {} (mean {:.2} ms)",
        with_step.len(),
        mean(&with_step, &|f| f.total),
        mean(&with_step, &|f| f.step),
        without.len(),
        mean(&without, &|f| f.total)
    );
    println!("per frame (ms): step {:.2}, preparation {:.2}, renderer CPU {:.2}, GPU {:.2}", avg(&|f| f.step), avg(&|f| f.prep), avg(&|f| f.render), avg(&|f| f.gpu));
    println!("\nstep phases (ms per step):");
    for (k, v) in Session::take_step_profile() {
        if v * 1000.0 / steps.max(1) as f64 >= 0.01 {
            println!("  {k:<28} {:.3}", v * 1000.0 / steps.max(1) as f64);
        }
    }
    println!("\nframe preparation (ms per frame):");
    let mut p: Vec<_> = prep_sum.into_iter().collect();
    p.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (k, v) in p {
        println!("  {k:<28} {:.3}", v / n);
    }
    println!("\nrenderer (ms per frame):");
    let mut r: Vec<_> = render_sum.into_iter().collect();
    r.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (k, v) in r {
        println!("  {k:<28} {:.3}", v / n);
    }
    if std::env::var_os("DX_VM_PROFILE").is_some() {
        // the script functions and natives by self time over warm-up and run (`DX_VM_PROFILE=1`)
        println!("\nscript functions and natives:\n{}", session.vm.profile_report(40));
    }
    let (rss, hwm) = rss_mib();
    println!("\nmemory: resident {rss:.0} MiB (peak {hwm:.0}, {:+.0} during the run)", rss - rss0);
    if let Some((used, budget)) = rt.renderer()?.cx.video_memory() {
        println!("video memory: {:.0} MiB used of {:.0} MiB budget", used as f64 / 1048576.0, budget as f64 / 1048576.0);
    }
    if let Some(path) = csv {
        let mut out = String::from("frame_ms,steps,step_ms,prep_ms,render_ms,gpu_ms\n");
        for f in &frames {
            out += &format!("{:.3},{},{:.3},{:.3},{:.3},{:.3}\n", f.total, f.steps, f.step, f.prep, f.render, f.gpu);
        }
        std::fs::write(&path, out).map_err(|e| e.to_string())?;
        println!("per frame: {path}");
    }
    // `DX_BENCH_SHOT=file.png`: the last frame the bench drew (the window's path, read back before its gamma)
    if let Ok(path) = std::env::var("DX_BENCH_SHOT") {
        if let Some((w, h, mut rgba)) = rt.renderer()?.read_last_frame().map_err(|e| e.to_string())? {
            dx_vk::apply_brightness(&mut rgba, session.brightness);
            std::fs::write(&path, dx_soft::png(w, h, &rgba)).map_err(|e| e.to_string())?;
            println!("picture: {path}");
        }
    }
    rt.into_inner()?.destroy();
    Ok(())
}

#[cfg(feature = "profile")]
type Profiler = Option<pprof::ProfilerGuard<'static>>;
#[cfg(not(feature = "profile"))]
type Profiler = ();

#[cfg(feature = "profile")]
fn start_profile(on: bool) -> Result<Profiler, String> {
    if !on {
        return Ok(None);
    }
    pprof::ProfilerGuardBuilder::default().frequency(2000).blocklist(&["libc", "libgcc", "pthread", "vdso"]).build().map(Some).map_err(|e| e.to_string())
}

#[cfg(not(feature = "profile"))]
fn start_profile(on: bool) -> Result<Profiler, String> {
    if on {
        return Err("--profile needs a build with `--features profile`".into());
    }
    Ok(())
}

/// Writes the samples as collapsed stacks (`FILE`, one `a;b;c count` line per stack, for flame graph tools) and prints the functions with the most samples, by self time and with callees.
#[cfg(feature = "profile")]
fn write_profile(profiler: Profiler, path: &str) -> Result<(), String> {
    let Some(guard) = profiler else { return Ok(()) };
    let report = guard.report().build().map_err(|e| e.to_string())?;
    let mut collapsed = String::new();
    let mut self_n: std::collections::HashMap<String, isize> = Default::default();
    let mut incl_n: std::collections::HashMap<String, isize> = Default::default();
    let mut total = 0isize;
    for (frames, &count) in report.data.iter() {
        // innermost first in `frames.frames`; each frame may hold inlined symbols (innermost first too)
        let names: Vec<String> = frames.frames.iter().flat_map(|f| f.iter().map(|s| s.name())).collect();
        let shorten = |n: &str| -> String {
            let n = n.split("::h").next().unwrap_or(n);
            n.replace(';', ":")
        };
        let mut stack: Vec<String> = names.iter().rev().map(|n| shorten(n)).collect();
        if stack.is_empty() {
            stack.push(frames.thread_name.clone());
        }
        collapsed += &format!("{} {}\n", stack.join(";"), count);
        total += count;
        *self_n.entry(stack.last().cloned().unwrap_or_default()).or_default() += count;
        let mut seen = std::collections::HashSet::new();
        for f in &stack {
            if seen.insert(f.clone()) {
                *incl_n.entry(f.clone()).or_default() += count;
            }
        }
    }
    std::fs::write(path, collapsed).map_err(|e| e.to_string())?;
    let top = |m: &std::collections::HashMap<String, isize>, title: &str, k: usize| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        println!("\n{title} ({total} samples):");
        for (name, n) in v.into_iter().take(k) {
            let short: String = name.chars().take(110).collect();
            println!("  {:5.1}%  {short}", *n as f64 * 100.0 / total.max(1) as f64);
        }
    };
    top(&self_n, "self time", 40);
    top(&incl_n, "with callees", 60);
    println!("profile: {path} (collapsed stacks)");
    Ok(())
}

#[cfg(not(feature = "profile"))]
fn write_profile(_: Profiler, _: &str) -> Result<(), String> {
    Ok(())
}
