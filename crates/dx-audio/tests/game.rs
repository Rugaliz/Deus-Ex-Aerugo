//! Decoding real game sounds and music. Set `DEUSEX_DIR` to the game folder.
use dx_audio::{Mixer, PcmSound};
use std::path::PathBuf;
use ue_assets::audio::{load_music, load_sound, mp3_info, wav_info};
use ue_package::Package;

fn game() -> Option<PathBuf> {
    std::env::var_os("DEUSEX_DIR").map(PathBuf::from)
}

fn exports_of(pkg: &Package, class: &str) -> Vec<usize> {
    (0..pkg.exports.len()).filter(|&i| pkg.export_class_name(i) == class).collect()
}

/// Every WAV of an effects package decodes with the header's rate and length; MP3 voice lines decode to their parsed duration.
#[test]
fn wav_and_mp3_sounds_decode_to_the_length_their_headers_promise() {
    let Some(g) = game() else { return };
    let pkg = Package::open(g.join("Sounds").join("Ambient.uax")).unwrap();
    let (mut n, mut seconds) = (0, 0.0);
    for e in exports_of(&pkg, "Sound") {
        let a = load_sound(&pkg, e).unwrap();
        let w = wav_info(&a.data).unwrap();
        let s = PcmSound::decode(&a.format, &a.data).unwrap();
        assert_eq!(s.rate, w.sample_rate);
        assert_eq!(w.format_tag, 1);
        assert_eq!(s.channels, w.channels);
        assert_eq!(s.frames(), w.data_len / (w.bits_per_sample as usize / 8) / w.channels as usize);
        assert!(s.samples.iter().any(|&x| x != 0.0), "silent sound {e}");
        n += 1;
        seconds += s.seconds();
    }
    assert!(n > 20, "{n} sounds");
    eprintln!("{n} WAV sounds, {seconds:.0} s");

    // conversation audio (MP3)
    let con = std::fs::read_dir(g.join("Sounds")).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with("DeusExConAudioAIBarks")).or_else(|| {
        std::fs::read_dir(g.join("System")).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with("DeusExConAudio"))
    });
    let Some(con) = con else { return };
    let pkg = Package::open(con).unwrap();
    let mut checked = 0;
    for e in exports_of(&pkg, "Sound").into_iter().take(40) {
        let a = load_sound(&pkg, e).unwrap();
        if !a.format.eq_ignore_ascii_case("mp3") {
            continue;
        }
        let info = mp3_info(&a.data).unwrap();
        let s = PcmSound::decode(&a.format, &a.data).unwrap();
        assert!((s.seconds() - info.duration_secs).abs() < 0.1, "decoded {} s, header says {}", s.seconds(), info.duration_secs);
        assert!(s.samples.iter().any(|&x| x.abs() > 0.01));
        checked += 1;
    }
    assert!(checked >= 5, "{checked} mp3 sounds checked");
}

/// A shipped tracker module plays: it is loaded by Galaxy's loader, has a title, and renders audible, bounded audio.
#[test]
fn music_renders_audio() {
    let Some(g) = game() else { return };
    let path = std::fs::read_dir(g.join("Music")).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().map(|x| x == "umx").unwrap_or(false)).expect("a music package");
    let pkg = Package::open(&path).unwrap();
    let e = exports_of(&pkg, "Music")[0];
    let a = load_music(&pkg, e).unwrap();
    let mut m = Mixer::new(44100);
    let title = m.play_music(&a.data, 0).unwrap();
    eprintln!("{} ({}): '{title}'", path.display(), a.format);
    let mut out = vec![0.0; 44100 * 2 * 3];
    m.mix(&mut out);
    let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
    assert!(rms > 0.01 && rms < 0.9, "rms {rms}");
    assert!(out.iter().all(|x| x.abs() <= 1.0));
    // the two channels are not identical (a real stereo mix)
    assert!(out.chunks(2).any(|f| (f[0] - f[1]).abs() > 0.01));
}

/// Every shipped song loads through the port of Galaxy's IT loader, with the facts the loader survey found
/// (docs/galaxy/loader.md section 8).
#[test]
fn galaxy_loader_reads_every_song() {
    let Some(g) = game() else { return };
    let mut paths: Vec<_> = std::fs::read_dir(g.join("Music")).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x == "umx").unwrap_or(false)).collect();
    paths.sort();
    assert_eq!(paths.len(), 35);
    let mut biggest = 0;
    for path in paths {
        let pkg = Package::open(&path).unwrap();
        let e = exports_of(&pkg, "Music")[0];
        let a = load_music(&pkg, e).unwrap();
        let song = dx_audio::galaxy::song::load(&a.data).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        assert_eq!(song.global_volume, 0x80, "{stem}");
        assert_eq!(song.mode, if stem.starts_with("Endgame1") || stem.starts_with("Endgame2") { 3 } else { 2 }, "{stem}");
        assert!(song.channels >= 1 && song.channels <= 31, "{stem}: {}", song.channels);
        assert!(song.channel_pan.iter().all(|&p| p == 0x40), "{stem}");
        for p in song.patterns.iter().flatten() {
            biggest = biggest.max(p.len());
        }
        // every loaded sample has its 31-frame tail and starts faded in from 0
        for ins in song.instruments.iter().flatten() {
            for s in &ins.slots {
                if let Some(pcm) = &s.pcm {
                    assert_eq!(pcm.len(), s.length as usize + 31);
                    assert_eq!(pcm.get(0), 0);
                }
            }
        }
        let with_env = song.instruments.iter().flatten().filter(|i| i.pan_env.flags != 0).count();
        assert_eq!(with_env, if stem == "Lebedev_Music" { 3 } else { 0 }, "{stem}");
    }
    // the translated tracks fit the 0xC101 bytes the loader allocates for each
    assert!(biggest < 0xC101, "{biggest}");
}

/// A conversation line (MP3) plays through Galaxy's stream ring: after the start delay the voice is audible.
#[test]
fn an_mp3_voice_plays_through_the_stream_ring() {
    let Some(g) = game() else { return };
    let path = g.join("System").join("DeusExConAudioIntro.u");
    let pkg = Package::open(&path).unwrap();
    let e = exports_of(&pkg, "Sound")[0];
    let a = load_sound(&pkg, e).unwrap();
    let s = std::sync::Arc::new(dx_audio::galaxy::sample::load(&a.data).unwrap());
    eprintln!("{} flags {:x} rate {}", path.display(), s.flags, s.rate);
    assert!(s.streamed());
    let mut m = Mixer::new(44100);
    m.music_volume = 0.0;
    m.speech_volume = 255;
    m.start_voice(0, &s, s.rate, 0x7fff, 0x4000);
    let mut out = vec![0f32; 44100 * 2];
    m.mix(&mut out);
    let rms = |b: &[f32]| (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt();
    eprintln!("rms first 0.15 s {} rest {}", rms(&out[..6615 * 2]), rms(&out[22050..]));
    assert!(rms(&out[22050..]) > 0.005);
}
