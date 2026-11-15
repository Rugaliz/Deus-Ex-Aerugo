//! Mesh animation on real meshes: `PlayAnim` runs a sequence to its end in the time the mesh data predicts and sends
//! `AnimEnd`; `LoopAnim` keeps going; `FinishAnim` waits for the end. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::{Value, Vm};

fn world() -> Option<Vm> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut vm = Vm::new(Registry::load_game(&g).unwrap());
    vm.init_world().unwrap();
    Some(vm)
}

/// A troop and the first of its sequences with more than ten frames, read straight from the mesh file.
fn troop_and_sequence(vm: &mut Vm) -> (u32, u32, i32, f32) {
    let c = vm.reg.class_by_name("UNATCOTroop").unwrap();
    let a = vm.spawn(c, 0, None, Some([0.0, 0.0, 0.0]), None).unwrap().unwrap();
    let Value::Obj(mesh) = vm.get_prop(a, "Mesh", 0).unwrap() else { panic!("no mesh") };
    let path = vm.reg.obj_path(ue_registry::ObjId(mesh)).to_string();
    let (pi, ei) = vm.reg.export_by_path(&path).expect("mesh export");
    let pkg = &vm.reg.packages[pi].pkg;
    let m = ue_assets::mesh::LodMesh::load(pkg, ei).unwrap().mesh;
    let s = m.anim_seqs.iter().find(|s| s.num_frames > 10 && s.rate > 0.0).expect("a long sequence");
    let (name, frames, rate) = (s.name, s.num_frames, s.rate);
    let global = vm.reg.global_name(pi, name);
    (a, global, frames, rate)
}

#[test]
fn play_anim_ends_when_the_mesh_data_says_and_sends_anim_end() {
    let Some(mut vm) = world() else { return };
    let (a, seq, frames, rate) = troop_and_sequence(&mut vm);
    assert_eq!(vm.call_by_name(a, "HasAnim", vec![Value::Name(seq)]).unwrap(), Value::Bool(true));
    vm.event_trace = Some(vec![]);
    // the first animation of an actor does not tween in; it starts at frame 0.001
    vm.call_by_name(a, "PlayAnim", vec![Value::Name(seq), Value::Float(1.0)]).unwrap();
    let last = 1.0 - 1.0 / frames as f32;
    assert_eq!(vm.get_prop(a, "AnimLast", 0).unwrap(), Value::Float(last));
    let expected = (last - 0.001) * frames as f32 / rate;
    let dt = 1.0 / 30.0;
    let mut t = 0.0;
    while vm.get_prop(a, "bAnimFinished", 0).unwrap() != Value::Bool(true) {
        vm.tick_animation(a, dt).unwrap();
        t += dt;
        assert!(t < expected + 1.0, "never finished; expected about {expected} s");
    }
    assert!((t - expected).abs() < 2.0 * dt, "finished after {t} s, the mesh data says {expected} s");
    assert_eq!(vm.get_prop(a, "AnimFrame", 0).unwrap(), Value::Float(last));
    assert_eq!(vm.get_prop(a, "AnimRate", 0).unwrap(), Value::Float(0.0));
    let ends = vm.event_trace.clone().unwrap().iter().filter(|e| e.0 == a && e.1 == "AnimEnd").count();
    assert_eq!(ends, 1, "AnimEnd sent {ends} times");
}

#[test]
fn loop_anim_wraps_and_finish_anim_waits() {
    let Some(mut vm) = world() else { return };
    let (a, seq, frames, rate) = troop_and_sequence(&mut vm);
    vm.event_trace = Some(vec![]);
    vm.call_by_name(a, "LoopAnim", vec![Value::Name(seq), Value::Float(1.0)]).unwrap();
    let period = frames as f32 / rate;
    let dt = 1.0 / 30.0;
    let mut t = 0.0;
    let mut max_frame: f32 = 0.0;
    while t < period * 2.5 {
        vm.tick_animation(a, dt).unwrap();
        t += dt;
        let f = vm.get_prop(a, "AnimFrame", 0).unwrap().as_f32().unwrap();
        assert!(f < 1.0, "frame {f} left the sequence");
        max_frame = max_frame.max(f);
    }
    assert!(max_frame > 0.9, "never got near the end ({max_frame})");
    let ends = vm.event_trace.clone().unwrap().iter().filter(|e| e.0 == a && e.1 == "AnimEnd").count();
    assert!((2..=3).contains(&ends), "a loop sends AnimEnd once per cycle, got {ends} in 2.5 cycles");
    // FinishAnim turns the loop off and waits until the end
    vm.call_by_name(a, "FinishAnim", vec![]).unwrap();
    assert_eq!(vm.get_prop(a, "bAnimLoop", 0).unwrap(), Value::Bool(false));
    assert!(vm.take_pending_latent().is_some(), "FinishAnim did not start waiting");
    // a sequence that does not exist is refused and leaves the state alone
    let bogus = vm.reg.names.intern("NoSuchSequence");
    assert_eq!(vm.call_by_name(a, "HasAnim", vec![Value::Name(bogus)]).unwrap(), Value::Bool(false));
}
