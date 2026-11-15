//! The SDL output path, checked without a sound card: SDL's `disk` audio driver writes what the device would play into a file.
use dx_audio::galaxy::sample::{Sample, F_16BIT, F_LOOP, F_MONO};
use dx_audio::galaxy::song::Pcm;
use dx_audio::AudioEngine;
use std::sync::Arc;

#[test]
fn the_device_pulls_audio_from_the_mixer() {
    let path = std::env::temp_dir().join(format!("dx_audio_test_{}.raw", std::process::id()));
    std::env::set_var("SDL_AUDIODRIVER", "disk");
    std::env::set_var("SDL_AUDIO_DISK_OUTPUT_FILE", &path);
    let Ok(sdl) = sdl3::init() else { return };
    let Ok(audio) = sdl.audio() else {
        eprintln!("skipping: no audio subsystem");
        return;
    };
    let rate = AudioEngine::DEFAULT_RATE;
    let engine = match AudioEngine::start(&audio, rate) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };

    // a looping 440 Hz tone on effect voice 0
    let mut pcm: Vec<i16> = (0..rate as usize).map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 24000.0) as i16).collect();
    pcm.resize(rate as usize + 16, 0);
    let tone = Arc::new(Sample { flags: F_MONO | F_16BIT | F_LOOP, pan: 0x4000, volume: 0x7fff, length: rate, loop_start: 0, loop_end: rate, rate, data: Some(Arc::new(Pcm::S16(pcm))), stream: None });
    {
        let mut m = engine.mixer.lock().unwrap();
        m.sound_volume = 255;
        m.start_voice(0, &tone, rate, 0x7fff, 0x4000);
    }
    std::thread::sleep(std::time::Duration::from_millis(600));
    drop(engine);
    let bytes = std::fs::read(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    let samples: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    assert!(samples.len() > 4000, "the device wrote {} samples", samples.len());
    let peak = samples.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    assert!(peak > 0.2 && peak <= 1.0, "peak {peak}");
}
