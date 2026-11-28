//! Surveys for user reports (not checks): load a save of the player's save folder (`AERUGO_SAVES`, read only) and list
//! things near the player. Ignored; run one with `--ignored --nocapture`. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn load(slot: i32) -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let saves = PathBuf::from(std::env::var_os("AERUGO_SAVES")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.vm.save_dir = Some(saves);
    s.load_game(slot).unwrap();
    Some(s)
}

/// `SURVEY_SLOT`, `SURVEY_CLASS` (a class name, subclasses included), `SURVEY_SECONDS` to run first.
#[test]
#[ignore]
fn survey_save_actors() {
    let slot: i32 = std::env::var("SURVEY_SLOT").ok().and_then(|v| v.parse().ok()).unwrap_or(130);
    let Some(mut s) = load(slot) else { return };
    let secs: f32 = std::env::var("SURVEY_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
    for _ in 0..(secs * 30.0) as usize {
        s.step_bound(1.0 / 30.0);
    }
    let class = std::env::var("SURVEY_CLASS").unwrap_or_else(|_| "ParticleGenerator".into());
    let c = s.vm.reg.class_by_name(&class).unwrap();
    let me = s.player_location();
    let show = |s: &Session, a: u32, p: &str| match s.vm.get_prop(a, p, 0) { Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(), Ok(Value::Obj(o)) => if o == 0 { "None".into() } else { s.vm.reg.obj_path(ue_registry::ObjId(o)).to_string() }, Ok(v) => format!("{v:?}"), _ => "-".into() };
    let props: Vec<String> = std::env::var("SURVEY_PROPS").unwrap_or_else(|_| "Tag,Event,bSpewing,bFrozen,bTriggered,bInitiallyOn,bHidden,frequency,checkTime,particleTexture,proxy,attachTag,bDying,LifeSpan".into()).split(',').map(str::to_string).collect();
    eprintln!("map {} player at {me:?}", s.map_name);
    let mut rows: Vec<(f32, String)> = Vec::new();
    for a in s.vm.live_actors() {
        if !s.vm.class_of(a).is_some_and(|k| s.vm.is_a(k, c)) {
            continue;
        }
        let at = s.vm.location(a);
        let d = ((at[0] - me[0]).powi(2) + (at[1] - me[1]).powi(2) + (at[2] - me[2]).powi(2)).sqrt();
        let line = props.iter().map(|p| format!("{p}={}", show(&s, a, p))).collect::<Vec<_>>().join(" ");
        rows.push((d, format!("{} d={d:.0} at {at:?}: {line}", s.vm.name_of(a))));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, r) in rows.iter().take(40) {
        eprintln!("{r}");
    }
}

/// A picture of a save after `SURVEY_SECONDS` (Vulkan), to `out/screenshots/save<slot>.png`.
#[test]
#[ignore]
fn survey_save_shot() {
    let slot: i32 = std::env::var("SURVEY_SLOT").ok().and_then(|v| v.parse().ok()).unwrap_or(130);
    let Some(mut s) = load(slot) else { return };
    let secs: f32 = std::env::var("SURVEY_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
    for _ in 0..(secs * 30.0) as usize {
        s.step_bound(1.0 / 30.0);
    }
    // `SURVEY_VIEW="x y z pitch yaw"` (rotator units, the probe's): the view to take instead of the save's
    if let Some(v) = std::env::var("SURVEY_VIEW").ok().map(|v| v.split_whitespace().filter_map(|x| x.parse::<f32>().ok()).collect::<Vec<_>>()).filter(|v| v.len() >= 5) {
        s.teleport([v[0], v[1], v[2]], v[4] * 360.0 / 65536.0);
        for _ in 0..(secs * 30.0) as usize {
            s.step_bound(1.0 / 30.0);
        }
        s.pitch = v[3] as i32;
        let p = s.player;
        s.vm.set_prop(p, "ViewRotation", 0, Value::Rotator([v[3] as i32, v[4] as i32, 0])).unwrap();
    }
    let p = s.player;
    eprintln!("view: at {:?} pitch {} yaw {} ViewRotation {:?} BaseEyeHeight {:?} EyeHeight {:?}", s.player_location(), s.pitch, s.yaw, s.vm.get_prop(p, "ViewRotation", 0).ok(), s.vm.get_prop(p, "BaseEyeHeight", 0).ok(), s.vm.get_prop(p, "EyeHeight", 0).ok());
    let mut r = None;
    let mut img = s.render_vk(&mut r, 1280, 720).expect("vulkan render");
    for _ in 0..60 {
        s.step_bound(1.0 / 30.0);
        img = s.render_vk(&mut r, 1280, 720).expect("vulkan render");
    }
    if let Some(c) = s.vm.reg.class_by_name("ParticleGenerator") {
        let me = s.player_location();
        for a in s.vm.live_actors() {
            if !s.vm.class_of(a).is_some_and(|k| s.vm.is_a(k, c)) { continue; }
            let at = s.vm.location(a);
            let d = ((at[0] - me[0]).powi(2) + (at[1] - me[1]).powi(2) + (at[2] - me[2]).powi(2)).sqrt();
            if d > 600.0 { continue; }
            let it = match s.vm.get_prop(a, "RenderInterface", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
            let n = if it != 0 { s.vm.live_particles(it) } else { 0 };
            let proxy = match s.vm.get_prop(a, "proxy", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
            if it != 0 && proxy != 0 {
                eprintln!("proxy coll r {:?} h {:?} world {:?} actors {:?} placing {:?} at {:?}", s.vm.get_prop(proxy, "CollisionRadius", 0).ok(), s.vm.get_prop(proxy, "CollisionHeight", 0).ok(), s.vm.get_prop(proxy, "bCollideWorld", 0).ok(), s.vm.get_prop(proxy, "bCollideActors", 0).ok(), s.vm.get_prop(proxy, "bCollideWhenPlacing", 0).ok(), s.vm.location(proxy));
                if let Ok(Value::Struct(_, _)) = s.vm.get_prop(it, "Particles", 0) {}
                for i in 0..64u32 {
                    let Ok(Value::Struct(sid, bytes)) = s.vm.get_prop(it, "Particles", i) else { break };
                    let _ = sid;
                    if bytes.first().copied().unwrap_or(0) & 1 == 0 { continue; }
                    let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
                    let loc = [f(28), f(32), f(36)];
                    let ok = s.vm.far_move_actor(proxy, loc, false, false);
                    eprintln!("  particle {i} at {loc:?} far_move {ok:?} -> {:?}", s.vm.location(proxy));
                }
            }
            if it != 0 && proxy != 0 {
                let cam = s.camera();
                let (_, frames) = s.compute_visibility(&cam, 1280, 720);
                let mine: Vec<usize> = frames[0].sprites.iter().enumerate().filter(|(_, sp)| sp.id == proxy).map(|(i, _)| i).collect();
                let drawn = mine.iter().filter(|&&i| frames[0].vis.sprites[i]).count();
                eprintln!("  vis cam {:?}: {} sprites in the main frame, {} of this proxy, {drawn} drawn; rects {:?}", (cam.pos, cam.pitch, cam.yaw, cam.fov), frames[0].sprites.len(), mine.len(), mine.iter().map(|&i| { let sp = &frames[0].sprites[i]; (sp.x1, sp.y1, sp.x2, sp.y2, sp.z as i32) }).collect::<Vec<_>>());
            }
            eprintln!("gen {} d={d:.0} iface {it} particles {n} frozen {:?} spewing {:?} proxy {proxy} proxyLastRender {:?}", s.vm.name_of(a), s.vm.get_prop(a, "bFrozen", 0).ok(), s.vm.get_prop(a, "bSpewing", 0).ok(), s.vm.get_prop(proxy, "LastRenderTime", 0).ok());
        }
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("save{slot}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

#[test]
#[ignore]
fn survey_texture_size() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let path = std::env::var("SURVEY_TEXTURE").unwrap_or_else(|_| "Effects.Smoke.SmokePuff1".into());
    let mut s = Session::open(&g, "03_NYC_BrooklynBridgeStation").unwrap();
    let id = s.vm.reg.obj_id(&path).0;
    let size = s.vm.texture_size(id);
    eprintln!("fresh: id {id} path {} size {size:?} export {:?}", s.vm.reg.obj_path(ue_registry::ObjId(id)), s.vm.reg.export_by_path(&path));
    if let Some((pi, ei)) = s.vm.reg.export_by_path(&path) {
        let class = s.vm.reg.class_of_export(pi, ei);
        eprintln!("class {:?} {:?}", class, class.map(|c| s.vm.reg.class(c).path.clone()));
        if let Some(c) = class {
            eprintln!("USize prop {:?}", s.vm.reg.find_prop(c, "USize").map(|p| (p.offset, p.flags)));
        }
    }
    if let Some(mut s) = load(std::env::var("SURVEY_SLOT").ok().and_then(|v| v.parse().ok()).unwrap_or(130)) {
        let id = s.vm.reg.obj_id(&path).0;
        let size = s.vm.texture_size(id);
        eprintln!("loaded: id {id} path {} size {size:?} export {:?}", s.vm.reg.obj_path(ue_registry::ObjId(id)), s.vm.reg.export_by_path(&path));
    }
}

/// Every package of the game: exports that share their full path with a palette (the path used to mean the palette).
#[test]
#[ignore]
fn survey_palette_path_clashes() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut total = 0;
    for dir in ["Textures", "System", "Maps"] {
        let Ok(rd) = std::fs::read_dir(g.join(dir)) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("utx" | "u" | "dx"))).collect();
        files.sort();
        for f in files {
            let Ok(pkg) = ue_package::Package::open(&f) else { continue };
            let mut seen: std::collections::HashMap<String, usize> = Default::default();
            let mut clashes = Vec::new();
            for ei in 0..pkg.exports.len() {
                let Some(p) = pkg.object_path(ue_package::ObjectRef::Export(ei)) else { continue };
                let key = p.to_ascii_lowercase();
                if let Some(&first) = seen.get(&key) {
                    if pkg.export_class_name(first) == "Palette" && pkg.export_class_name(ei) != "Palette" {
                        clashes.push(format!("{p} ({})", pkg.export_class_name(ei)));
                    }
                } else {
                    seen.insert(key, ei);
                }
            }
            if !clashes.is_empty() {
                total += clashes.len();
                eprintln!("{}: {} {:?}", f.file_name().unwrap().to_string_lossy(), clashes.len(), clashes.iter().take(6).collect::<Vec<_>>());
            }
        }
    }
    eprintln!("total {total}");
}

/// Surfaces of `SURVEY_MAP` with the fake backdrop (sky) flag whose normal points up or down (floors, ceilings), with
/// their texture and a point.
#[test]
#[ignore]
fn survey_sky_floors() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let map = std::env::var("SURVEY_MAP").unwrap_or_else(|_| "15_Area51_Bunker".into());
    let s = Session::open(&g, &map).unwrap();
    let m = &s.view.model;
    let pkg = ue_package::Package::open(g.join("Maps").join(format!("{map}.dx"))).unwrap();
    let tex = |t: i32| pkg.object_path(ue_package::ObjectRef::from_index(t)).unwrap_or_default();
    for (i, sf) in m.surfs.iter().enumerate() {
        if sf.poly_flags & 0x80 == 0 {
            continue;
        }
        let n = m.vectors.get(sf.v_normal as usize).copied().unwrap_or([0.0; 3]);
        let p = m.points.get(sf.p_base as usize).copied().unwrap_or([0.0; 3]);
        if n[2].abs() > 0.7 {
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for nd in m.nodes.iter().filter(|nd| nd.i_surf == i as i32) {
                for k in 0..nd.num_vertices as usize {
                    let Some(v) = m.verts.get(nd.i_vert_pool as usize + k) else { continue };
                    let Some(pt) = m.points.get(v.p_vertex as usize) else { continue };
                    for c in 0..3 {
                        mn[c] = mn[c].min(pt[c]);
                        mx[c] = mx[c].max(pt[c]);
                    }
                }
            }
            eprintln!("surf {i} normal {n:?} base {p:?} flags {:#x} texture {} box {mn:?}..{mx:?} lightmap {}", sf.poly_flags, tex(sf.texture), sf.i_light_map);
        }
    }
}

/// A picture of `SURVEY_MAP` from `SURVEY_VIEW="x y z pitch yaw"` (rotator units) after a second, to
/// `out/screenshots/<SURVEY_NAME>.png` (Vulkan).
#[test]
#[ignore]
fn survey_map_shot() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let map = std::env::var("SURVEY_MAP").unwrap_or_else(|_| "15_Area51_Bunker".into());
    let v: Vec<f32> = std::env::var("SURVEY_VIEW").unwrap_or_default().split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let mut s = Session::open(&g, &map).unwrap();
    if v.len() >= 5 {
        s.teleport([v[0], v[1], v[2]], v[4] * 360.0 / 65536.0);
    }
    for _ in 0..30 {
        s.step_bound(1.0 / 30.0);
    }
    if v.len() >= 5 {
        s.pitch = v[3] as i32;
        let p = s.player;
        s.vm.set_prop(p, "ViewRotation", 0, Value::Rotator([v[3] as i32, v[4] as i32, 0])).unwrap();
        s.step_bound(1.0 / 30.0);
        s.pitch = v[3] as i32;
        s.vm.set_prop(p, "ViewRotation", 0, Value::Rotator([v[3] as i32, v[4] as i32, 0])).unwrap();
    }
    let mut r = None;
    let img = s.render_vk(&mut r, 1280, 720).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let name = std::env::var("SURVEY_NAME").unwrap_or_else(|_| "map_shot".into());
    std::fs::write(dir.join(format!("{name}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    eprintln!("player at {:?}", s.player_location());
}

/// Memory over level changes: travels between maps (`SURVEY_MAPS`, comma separated) and prints the object count, the
/// live ones, the bytes the dead ones still hold, and the process's resident memory.
#[test]
#[ignore]
fn survey_travel_memory() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let maps = std::env::var("SURVEY_MAPS").unwrap_or_else(|_| "01_NYC_UNATCOHQ,01_NYC_UNATCOIsland,01_NYC_UNATCOHQ,01_NYC_UNATCOIsland,01_NYC_UNATCOHQ,01_NYC_UNATCOIsland".into());
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..30 {
        s.step_bound(1.0 / 30.0);
    }
    let rss = || std::fs::read_to_string("/proc/self/status").unwrap_or_default().lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0;
    let report = |s: &Session, what: &str| {
        let (mut live, mut dead_bytes, mut dead) = (0usize, 0usize, 0usize);
        for o in &s.vm.objects {
            if o.alive {
                live += 1;
            } else {
                dead += 1;
                dead_bytes += o.mem.data.len() + o.mem.strings.iter().map(|x| x.len()).sum::<usize>();
            }
        }
        eprintln!("{what}: {} objects, {live} live, {dead} dead holding {:.1} MB; resident {:.0} MB", s.vm.objects.len(), dead_bytes as f64 / 1e6, rss());
    };
    report(&s, &s.map_name.clone());
    for m in maps.split(',') {
        let p = s.player;
        s.vm.call_by_name(p, "ClientTravel", vec![Value::Str(m.into()), Value::Byte(2), Value::Bool(true)]).unwrap();
        for _ in 0..120 {
            s.step_bound(1.0 / 30.0);
            if s.map_name.eq_ignore_ascii_case(m) {
                break;
            }
        }
        for _ in 0..30 {
            s.step_bound(1.0 / 30.0);
        }
        report(&s, m);
    }
}

/// Destroys (`SURVEY_HOW=destroy`) or kills (`kill`) the first actor of `SURVEY_CLASS` in `SURVEY_MAP` and prints what
/// the VM logged meanwhile.
#[test]
#[ignore]
fn survey_destroy_log() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let map = std::env::var("SURVEY_MAP").unwrap_or_else(|_| "01_NYC_UNATCOIsland".into());
    let class = std::env::var("SURVEY_CLASS").unwrap_or_else(|_| "Terrorist".into());
    let mut s = Session::open(&g, &map).unwrap();
    for _ in 0..10 {
        s.step_bound(1.0 / 30.0);
    }
    let c = s.vm.reg.class_by_name(&class).unwrap();
    let a = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c)).unwrap();
    s.vm.log.clear();
    if std::env::var("SURVEY_HOW").as_deref() == Ok("kill") {
        let p = s.player;
        let at = s.vm.location(a);
        let shot = s.vm.reg.names.intern("Shot");
        s.vm.call_by_name(a, "TakeDamage", vec![Value::Int(1000), Value::Obj(p), Value::Vector(at), Value::Vector([0.0; 3]), Value::Name(shot)]).unwrap();
        for _ in 0..90 {
            s.step_bound(1.0 / 30.0);
        }
    } else {
        s.vm.destroy(a).unwrap();
        s.step_bound(1.0 / 30.0);
    }
    for l in &s.vm.log {
        eprintln!("log: {l}");
    }
}

/// Every `DeusExProjectile` class with its default `bExplodes` and `bIgnoresNanoDefense`.
#[test]
#[ignore]
fn survey_projectiles() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "DX").unwrap();
    let base = s.vm.reg.class_by_name("DeusExProjectile").unwrap();
    let names: Vec<String> = (0..s.vm.reg.classes.len()).map(|i| ue_registry::ClassId(i as u32)).filter(|&c| s.vm.is_a(c, base)).map(|c| s.vm.reg.class(c).path.rsplit('.').next().unwrap_or("").to_string()).collect();
    for n in names {
        let e = s.vm.default_value_of(&n, "bExplodes", 0);
        let i = s.vm.default_value_of(&n, "bIgnoresNanoDefense", 0);
        eprintln!("{n}: bExplodes {e:?} bIgnoresNanoDefense {i:?}");
    }
}
