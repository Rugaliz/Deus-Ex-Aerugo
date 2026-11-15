//! The audio bridge on a running map: level music starts, a script's `PlaySound` reaches the mixer, `StopSound` silences it.
//! Set `DEUSEX_DIR` to the game folder.
use dx_audio::Mixer;
use dx_play::{Input, Session};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use ue_assets::files::GameFiles;
use ue_package::ObjectRef;
use ue_vm::Value;

#[test]
fn level_music_and_scripted_sounds_reach_the_mixer() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "00_Training").unwrap();
    let mixer = Arc::new(Mutex::new(Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    // the login's `ClientSetMusic` asked for the level's song; Galaxy starts it on its next update
    s.step(&Input::default(), 1.0 / 30.0);
    assert!(mixer.lock().unwrap().has_music(), "the level's song should be playing");
    assert_eq!(s.audio.as_ref().unwrap().music_title.as_deref(), Some("Training Room"));

    // a long effect from the ambience package
    let files = GameFiles::new(&g);
    let pkg = files.package("Ambient").unwrap();
    let export = (0..pkg.pkg.exports.len()).find(|&i| pkg.pkg.export_class_name(i) == "Sound").unwrap();
    let path = pkg.pkg.object_path(ObjectRef::Export(export)).unwrap();
    let sound = s.vm.reg.obj_id(&format!("Ambient.{path}"));
    s.step(&Input::default(), 1.0 / 30.0);
    let before = mixer.lock().unwrap().started;
    let id = s
        .vm
        .call_by_name(s.player, "PlaySound", vec![Value::Obj(sound.0), Value::Byte(1), Value::Float(1.0), Value::Bool(false), Value::Float(0.0), Value::Float(1.0)])
        .unwrap();
    let id = id.as_i32().unwrap_or(0);
    assert!(id > 0, "PlaySound returns the request id");
    s.step(&Input::default(), 1.0 / 30.0);
    let (started, voices) = {
        let m = mixer.lock().unwrap();
        (m.started, m.voice_count())
    };
    assert_eq!(started, before + 1, "the sound reached the mixer");
    assert!(voices >= 1);
    s.vm.call_by_name(s.player, "StopSound", vec![Value::Int(id)]).unwrap();
    s.step(&Input::default(), 1.0 / 30.0);
    // the stopped sound is gone (the player's own voice; ambient sounds of the level may still play)
    let still = mixer.lock().unwrap().voice_count();
    assert!(still < voices, "StopSound removed the voice ({voices} -> {still})");
}

/// The zone reverb follows the view actor's zone (reverb.md 3.2): a `bReverbZone` with a master gain installs it,
/// clearing the flag removes it.
#[test]
fn the_zone_reverb_follows_the_players_zone() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "00_Training").unwrap();
    let mixer = Arc::new(Mutex::new(Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    s.step(&Input::default(), 1.0 / 30.0);
    let zone = s.vm.zone_of_actor(s.player);
    assert!(zone != 0);
    s.vm.set_prop(zone, "bReverbZone", 0, Value::Bool(true)).unwrap();
    s.vm.set_prop(zone, "MasterGain", 0, Value::Byte(200)).unwrap();
    s.vm.set_prop(zone, "CutoffHz", 0, Value::Int(6000)).unwrap();
    s.step(&Input::default(), 1.0 / 30.0);
    assert!(mixer.lock().unwrap().has_reverb(), "a reverb zone installs the reverb");
    s.vm.set_prop(zone, "bReverbZone", 0, Value::Bool(false)).unwrap();
    s.step(&Input::default(), 1.0 / 30.0);
    assert!(!mixer.lock().unwrap().has_reverb(), "leaving the reverb zone removes it");
}
