//! Ray-traced shadows and ambient occlusion (`dx_vk::Quality::rt_shadows`, `rt_ao`) on a real map. Needs `DEUSEX_DIR`
//! and a Vulkan device with ray queries (skipped otherwise). Pictures go to `DX_SHOTS` (default: the temp folder).

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn shots() -> PathBuf {
    std::env::var_os("DX_SHOTS").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// The training hall with a large crate on the floor two metres in front of the player, under the hall's lights.
fn hall_with_a_crate() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "00_Training").unwrap();
    let p = s.player;
    let loc = s.vm.location(p);
    let yaw = s.vm.rotation(p)[1];
    let fwd = ue_vm::rotate_vector([1.0, 0.0, 0.0], [0, yaw, 0], false);
    let c = s.vm.reg.class_by_name("CrateUnbreakableLarge").unwrap();
    let cls = Value::Obj(s.vm.reg.obj_id(&s.vm.reg.class(c).path.clone()).0);
    let at = [loc[0] + fwd[0] * 220.0, loc[1] + fwd[1] * 220.0, loc[2]];
    let crate_ = s.vm.call_by_name(p, "Spawn", vec![cls, Value::Obj(0), Value::Name(0), Value::Vector(at)]).unwrap();
    assert!(matches!(crate_, Value::Obj(o) if o != 0), "the crate spawns");
    for _ in 0..60 {
        s.step(&dx_play::Input::default(), DT);
    }
    s.pitch = -2500;
    Some(s)
}

/// With `DX_QUALITY` set to `q` (see `Session::quality`), a picture. The first one runs a few frames of the game loop
/// first (step, draw, mark what was drawn: the scripts that look at what was drawn, such as a pawn's blob shadow, have
/// their turn); the others are of the same moment, so only the picture options differ.
fn picture(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, q: &str) -> Option<dx_soft::Image> {
    std::env::set_var("DX_QUALITY", q);
    if let Some(r) = r.as_mut() {
        s.follow_quality(r).unwrap();
    }
    let frames = if r.is_none() { 4 } else { 0 };
    for _ in 0..frames {
        let _ = s.render_vk(r, 960, 540).ok()?;
        let cam = s.render_camera();
        s.mark_rendered(&cam);
        s.step(&dx_play::Input::default(), DT);
    }
    // the visibility pass and the world's lighting (the light lists) for this view
    let _ = s.render_vk(r, 960, 540).ok()?;
    let cam = s.render_camera();
    s.mark_rendered(&cam);
    let img = s.render_vk(r, 960, 540).ok()?;
    Some(img)
}

#[test]
fn objects_cast_ray_traced_shadows_on_the_floor() {
    let Some(mut s) = hall_with_a_crate() else { return };
    let mut r = None;
    let Some(off) = picture(&mut s, &mut r, "0,1,0,0,0,0,0") else { return };
    if !r.as_ref().is_some_and(|r: &dx_vk::renderer::Renderer| r.cx.rt.is_some()) {
        eprintln!("no ray queries on this device");
        return;
    }
    let on = picture(&mut s, &mut r, "0,1,0,0,0,1,0").unwrap();
    let ao = picture(&mut s, &mut r, "0,1,0,0,0,1,1").unwrap();
    for (name, img) in [("rt_off", &off), ("rt_shadows", &on), ("rt_shadows_ao", &ao)] {
        std::fs::write(shots().join(format!("{name}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    }
    // the shadows only darken
    let (mut darker, mut lighter) = (0, 0);
    for (a, b) in off.rgba.chunks(4).zip(on.rgba.chunks(4)) {
        let (la, lb) = (a[0] as i32 + a[1] as i32 + a[2] as i32, b[0] as i32 + b[1] as i32 + b[2] as i32);
        if lb + 6 < la {
            darker += 1;
        } else if lb > la + 6 {
            lighter += 1;
        }
    }
    eprintln!("shadows: {darker} pixels darker, {lighter} lighter (pictures in {})", shots().display());
    assert!(darker > 500, "the crate casts a shadow: {darker} pixels darker");
    assert!(lighter < darker / 50, "shadows never brighten: {lighter}");
    r.take().unwrap().destroy();
}

/// Paul Denton at the Liberty Island start, lit per pixel: with ray tracing, the lights the world or objects hide from
/// parts of him (his own arms included) and contact shadows darken him; nothing gets brighter.
#[test]
fn characters_get_ray_traced_shadows() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOIsland").unwrap();
    let pc = s.vm.reg.class_by_name("PaulDenton").unwrap();
    let paul = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).is_some_and(|c| s.vm.is_a(c, pc))).unwrap();
    let at = s.vm.location(paul);
    let yaw = s.vm.rotation(paul)[1];
    let fwd = ue_vm::rotate_vector([1.0, 0.0, 0.0], [0, yaw, 0], false);
    // in front of him, facing him
    let spot = [at[0] + fwd[0] * 130.0, at[1] + fwd[1] * 130.0, at[2]];
    let face = ((yaw + 32768) & 0xFFFF) as f32 * 360.0 / 65536.0;
    s.teleport(spot, face);
    for _ in 0..10 {
        s.step(&dx_play::Input::default(), DT);
    }
    let mut r = None;
    let Some(off) = picture(&mut s, &mut r, "0,1,0,0,1,0,0") else { return };
    if !r.as_ref().is_some_and(|r: &dx_vk::renderer::Renderer| r.cx.rt.is_some()) {
        return;
    }
    let shadows = picture(&mut s, &mut r, "0,1,0,0,1,1,0").unwrap();
    let on = picture(&mut s, &mut r, "0,1,0,0,1,1,1").unwrap();
    for (name, img) in [("paul_rt_off", &off), ("paul_rt_shadows", &shadows), ("paul_rt_on", &on)] {
        std::fs::write(shots().join(format!("{name}.png")), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
    }
    let (mut darker, mut lighter) = (0, 0);
    for (a, b) in off.rgba.chunks(4).zip(on.rgba.chunks(4)) {
        let (la, lb) = (a[0] as i32 + a[1] as i32 + a[2] as i32, b[0] as i32 + b[1] as i32 + b[2] as i32);
        if lb + 6 < la {
            darker += 1;
        } else if lb > la + 6 {
            lighter += 1;
        }
    }
    eprintln!("Paul: {darker} pixels darker, {lighter} lighter");
    assert!(darker > 200, "{darker}");
    assert!(lighter < darker / 20, "{lighter}");
    r.take().unwrap().destroy();
}
