//! `UTexture::Get` / `Update` / `Tick` / `ConstantTimeTick`: animated textures step along their `AnimNext` chain on
//! the viewport clock, by their frame-rate settings. Set `DEUSEX_DIR` to the game folder.

use std::path::PathBuf;
use ue_registry::Registry;
use ue_vm::Vm;

fn vm() -> Option<Vm> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Vm::new(Registry::load_game(&dir).ok()?))
}

fn id(vm: &mut Vm, path: &str) -> u32 {
    vm.reg.ensure_package_for_path(path);
    vm.reg.obj_id(path).0
}

fn name(vm: &Vm, t: u32) -> String {
    vm.reg.obj_path(ue_registry::ObjId(t)).rsplit('.').next().unwrap().to_string()
}

/// `MaxFrameRate` 0: one step per update (a new time), none for the same time again.
#[test]
fn a_texture_without_a_rate_steps_once_per_new_time() {
    let Some(mut vm) = vm() else { return };
    let t = id(&mut vm, "Effects.water.WtrFall_A02");
    let get = |vm: &mut Vm, time: f64| { let r = vm.texture_get(t, time); name(vm, r) };
    assert_eq!(get(&mut vm, 1.0), "WtrFall_A03");
    assert_eq!(get(&mut vm, 1.0), "WtrFall_A03");
    assert_eq!(get(&mut vm, 1.5), "WtrFall_A04");
    assert_eq!(name(&vm, vm.texture_current(t)), "WtrFall_A04");
}

/// `MaxFrameRate` 20 (`MinFrameRate` 0, clamped to 0.01): the accumulator steps once it reaches 1/20 s and restarts at 0.
#[test]
fn a_rated_texture_waits_for_its_frame_time() {
    let Some(mut vm) = vm() else { return };
    let t = id(&mut vm, "Effects.water.WtrFall_A01");
    let get = |vm: &mut Vm, time: f64| { let r = vm.texture_get(t, time); name(vm, r) };
    assert_eq!(get(&mut vm, 0.0), "WtrFall_A01", "no time passed");
    assert_eq!(get(&mut vm, 0.03), "WtrFall_A01");
    assert_eq!(get(&mut vm, 0.06), "WtrFall_A02");
    assert_eq!(get(&mut vm, 0.07), "WtrFall_A02");
    assert_eq!(get(&mut vm, 0.2), "WtrFall_A03");
}

/// `MinFrameRate` = `MaxFrameRate` = 15: past 1/15 s the step keeps the time over it.
#[test]
fn equal_rates_keep_the_excess_time() {
    let Some(mut vm) = vm() else { return };
    let t = id(&mut vm, "Extras.Eggs.Matrix_A00");
    let get = |vm: &mut Vm, time: f64| { let r = vm.texture_get(t, time); name(vm, r) };
    assert_eq!(get(&mut vm, 0.1), "Matrix_A01");
    assert_eq!(get(&mut vm, 0.13), "Matrix_A01");
    assert_eq!(get(&mut vm, 0.14), "Matrix_A02");
}
