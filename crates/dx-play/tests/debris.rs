//! Breaking things: a wooden crate bursts into fragments that fly, tumble, land and fade. Set `DEUSEX_DIR`.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn of_class(s: &mut Session, class: &str) -> Vec<u32> {
    let Some(c) = s.vm.reg.class_by_name(class) else { return Vec::new() };
    let all = s.vm.live_actors();
    all.into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect()
}

#[test]
fn a_broken_crate_leaves_fragments() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    run(&mut s, 5);
    let p = s.player_location();
    let mut crates: Vec<(f32, u32)> = Vec::new();
    for a in of_class(&mut s, "CrateBreakableMedGeneral").into_iter().chain(of_class(&mut s, "DeusExDecoration")) {
        let wood = matches!(s.vm.get_prop(a, "fragType", 0), Ok(Value::Obj(o)) if s.vm.reg.obj_path(ue_registry::ObjId(o)).ends_with("WoodFragment"));
        let inv = matches!(s.vm.get_prop(a, "bInvincible", 0), Ok(Value::Bool(true)));
        if wood && !inv {
            let l = s.vm.location(a);
            crates.push(((l[0] - p[0]).powi(2) + (l[1] - p[1]).powi(2), a));
        }
    }
    crates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (_, crate_) = *crates.first().expect("a breakable wooden decoration");
    let class = s.vm.class_of(crate_).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
    let at = s.vm.location(crate_);
    let before = of_class(&mut s, "DeusExFragment").len();
    let shots = std::env::var_os("DX_SHOTS").is_some();
    if shots {
        // stand 150 units away, looking at it
        let (dx, dy) = (p[0] - at[0], p[1] - at[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let spot = [at[0] + dx / len * 150.0, at[1] + dy / len * 150.0, at[2] + 20.0];
        s.teleport(spot, (-dy).atan2(-dx).to_degrees());
        run(&mut s, 5);
    }
    let player = s.player;
    let shot = s.vm.reg.names.intern("Shot");
    s.vm.send_event(crate_, "TakeDamage", vec![Value::Int(500), Value::Obj(player), Value::Vector(at), Value::Vector([0.0, 0.0, 1000.0]), Value::Name(shot)]).unwrap();
    run(&mut s, 2);
    let frags = of_class(&mut s, "DeusExFragment");
    eprintln!("{class} at {at:?}: alive {}; fragments {} -> {}; log {:?}", s.vm.is_live(crate_), before, frags.len(), s.vm.log.iter().rev().take(5).collect::<Vec<_>>());
    assert!(!s.vm.is_live(crate_), "the crate did not break");
    assert!(frags.len() > before, "no fragments");
    let f = frags[frags.len() - 1];
    let l0 = s.vm.location(f);
    let draw = s.vm.mesh_draw(f).is_some();
    let r0 = s.vm.get_prop(f, "Rotation", 0).ok();
    run(&mut s, 10);
    if shots {
        let mut r = None;
        let img = s.render_vk(&mut r, 800, 600).expect("vulkan render");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
        std::fs::write(dir.join("s12_debris.png"), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
        r.take().unwrap().destroy();
    }
    let l1 = s.vm.location(f);
    let r1 = s.vm.get_prop(f, "Rotation", 0).ok();
    eprintln!("fragment {f}: drawable {draw}, physics {:?}, {l0:?} -> {l1:?}, rotation {r0:?} -> {r1:?}", s.vm.get_prop(f, "Physics", 0));
    assert!(draw, "the fragment has nothing to draw");
    // `Spawn` without a location puts the new actor where the spawner is
    let d0 = ((l0[0] - at[0]).powi(2) + (l0[1] - at[1]).powi(2) + (l0[2] - at[2]).powi(2)).sqrt();
    assert!(d0 < 60.0, "the fragment started {d0} units from the crate");
    assert!(l0 != l1, "the fragment does not fly");
    assert!(r0 != r1, "the fragment does not tumble");
}
