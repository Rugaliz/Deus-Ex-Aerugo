//! Music as `UGalaxyAudioSubsystem::Update` drives it (Galaxy 10609510): the login's `ClientSetMusic` starts the
//! level's song at its section, the player's `SongSection` follows the order playing, a transition fades the song out
//! (`FastFade`: by 3 dt down to -0.006 x Latency) and then starts the new section. Set `DEUSEX_DIR` to the game folder.
use dx_audio::Mixer;
use dx_play::{Input, Session};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

#[test]
fn dynamic_music_fades_and_starts_sections_as_galaxy() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    let mixer = Arc::new(Mutex::new(Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    s.audio.as_mut().unwrap().fixed_dt = Some(DT as f64);
    let p = s.player;
    let level_section = s.vm.u8p(s.vm.level, "SongSection");
    assert_eq!(s.vm.u8p(p, "Transition"), 3, "the login asked for the level's song with a fade");
    s.step(&Input::default(), DT);
    assert!(mixer.lock().unwrap().has_music());
    assert_eq!(s.vm.u8p(p, "Transition"), 0, "nothing played before: the song starts at once");
    assert_eq!(s.audio.as_ref().unwrap().music.section, level_section);
    // render a little so the order position is real, then the player hears about it
    let mut buf = vec![0f32; 4096];
    mixer.lock().unwrap().mix(&mut buf);
    s.step(&Input::default(), DT);
    assert_eq!(s.vm.u8p(p, "SongSection"), level_section, "SongSection follows the order playing");

    // combat music: section 3 with a fast fade
    let song = s.vm.op(s.vm.level, "Song");
    s.vm.call_by_name(p, "ClientSetMusic", vec![Value::Obj(song), Value::Byte(3), Value::Byte(255), Value::Byte(4)]).unwrap();
    // the fade: 1 -> below -0.24 at 3 per second = 0.4133 s, then the new section in the same update
    let mut steps = 0;
    while s.vm.u8p(p, "Transition") != 0 && steps < 100 {
        s.step(&Input::default(), DT);
        steps += 1;
        let fade = mixer.lock().unwrap().music_fade;
        assert!(fade <= 1.0);
    }
    let expect = ((1.0 + 0.006 * 40.0) / (3.0 * DT as f64)).floor() as i32 + 1;
    assert_eq!(steps, expect, "fast fade length in updates");
    assert_eq!(s.audio.as_ref().unwrap().music.section, 3);
    assert_eq!(mixer.lock().unwrap().music_fade, 1.0, "the new section starts at full volume");
    // Galaxy jumps at the next row boundary: until then its order is the section - 1 (player.md 5.3), and that is
    // what the player is told
    s.step(&Input::default(), DT);
    assert_eq!(s.vm.u8p(p, "SongSection"), 2);
    // (0.2 s: Liberty Island's section 3 is one pattern that jumps on to order 17, as in the original)
    let mut more = vec![0f32; 8820 * 2];
    mixer.lock().unwrap().mix(&mut more);
    s.step(&Input::default(), DT);
    assert_eq!(s.vm.u8p(p, "SongSection"), 3);

    // section 255 stops the music (the song stays loaded); asking again for a section restarts it without a fade
    // (nothing playing)
    s.vm.call_by_name(p, "ClientSetMusic", vec![Value::Obj(song), Value::Byte(255), Value::Byte(255), Value::Byte(1)]).unwrap();
    s.step(&Input::default(), DT);
    assert!(!mixer.lock().unwrap().has_music());
    s.step(&Input::default(), DT);
    // a stop keeps Galaxy's order where it was, and Update reports that order
    assert_eq!(s.vm.u8p(p, "SongSection"), 3, "stopped: the last order");
    s.vm.call_by_name(p, "ClientSetMusic", vec![Value::Obj(song), Value::Byte(1), Value::Byte(255), Value::Byte(3)]).unwrap();
    s.step(&Input::default(), DT);
    assert!(mixer.lock().unwrap().has_music(), "a stopped song ends at once: the new section starts in one update");
}
