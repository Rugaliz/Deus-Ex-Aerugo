//! Augmentations as the player uses them. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    run(&mut s, 5);
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

fn obj(s: &mut Session, o: u32, prop: &str) -> u32 {
    match s.vm.get_prop(o, prop, 0) {
        Ok(Value::Obj(r)) => r,
        _ => 0,
    }
}

/// Switches on the vision augmentation at `level` (0..3) and returns it.
fn vision_on(s: &mut Session, level: i32) -> u32 {
    let p = s.player;
    let system = obj(s, p, "AugmentationSystem");
    assert!(system != 0, "no augmentation system");
    let class = s.vm.reg.obj_id("DeusEx.AugVision").0;
    let aug = match s.vm.call_by_name(system, "GivePlayerAugmentation", vec![Value::Obj(class)]).unwrap() {
        Value::Obj(a) if a != 0 => a,
        _ => match s.vm.call_by_name(system, "FindAugmentation", vec![Value::Obj(class)]).unwrap() {
            Value::Obj(a) => a,
            _ => 0,
        },
    };
    assert!(aug != 0, "no vision augmentation");
    s.vm.set_prop(aug, "CurrentLevel", 0, Value::Int(level)).unwrap();
    s.vm.set_prop(p, "Energy", 0, Value::Float(100.0)).unwrap();
    s.vm.call_by_name(aug, "Activate", vec![]).unwrap();
    assert_eq!(s.vm.get_prop(aug, "bIsActive", 0).unwrap(), Value::Bool(true), "the vision augmentation did not switch on");
    aug
}

/// Vision augmentation level 1+: the HUD's augmentation window draws every nearby heat source (pawns) again through
/// `GC.DrawActor`: unlit, in the window's translucent style, glow 2 (`AugmentationDisplayWindow.DrawVisionAugmentation`).
#[test]
fn vision_augmentation_draws_pawns_through_the_window_system() {
    let Some(mut s) = new_game() else { return };
    vision_on(&mut s, 2);
    run(&mut s, 5);
    s.vm.ui_enable_views(true);
    s.ui_quads(800, 600);
    let cmds = s.vm.ui_actor_cmds().to_vec();
    let sp = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let pawns: Vec<&ue_vm::ui_gc::ActorCmd> = cmds.iter().filter(|c| c.actor != 0 && s.vm.class_of(c.actor).map(|k| s.vm.is_a(k, sp)).unwrap_or(false)).collect();
    eprintln!("{} DrawActor commands, {} of pawns", cmds.len(), pawns.len());
    assert!(!pawns.is_empty(), "no pawn drawn by the vision augmentation");
    for c in &pawns {
        let d = c.draw.as_ref().expect("a pawn without a mesh draw");
        assert!(d.unlit && d.style == 3 && (d.glow - 2.0).abs() < 1e-6, "draw overrides: unlit {} style {} glow {}", d.unlit, d.style, d.glow);
    }
    // the overrides are temporary: the pawn is drawn normally in the world
    let a = pawns[0].actor;
    assert_eq!(s.vm.get_prop(a, "bUnlit", 0).unwrap(), Value::Bool(false));
    assert!((s.vm.fp(a, "ScaleGlow") - 2.0).abs() > 1e-6 || s.vm.get_prop(a, "Style", 0).unwrap() != Value::Byte(3));
}

/// A picture of the vision augmentation (needs a Vulkan device; `out/screenshots/aug_vision.png`).
#[test]
#[ignore]
fn shot_of_the_vision_augmentation() {
    let Some(mut s) = new_game() else { return };
    vision_on(&mut s, 2);
    run(&mut s, 10);
    let mut r = None;
    let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("aug_vision.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}
