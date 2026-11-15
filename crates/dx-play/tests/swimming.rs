//! The player in water: entering, swimming and surfacing. Set `DEUSEX_DIR` to the game folder.

use dx_input::key_code;
use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;
const PHYS_FALLING: u8 = 2;
const PHYS_SWIMMING: u8 = 3;

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

fn physics(s: &mut Session) -> u8 {
    match s.vm.get_prop(s.player, "Physics", 0) {
        Ok(Value::Byte(b)) => b,
        _ => 255,
    }
}

fn in_water(s: &mut Session, p: [f32; 3]) -> bool {
    let z = s.vm.point_region(p).zone;
    matches!(s.vm.get_prop(z, "bWaterZone", 0), Ok(Value::Bool(true)))
}

/// A spot 200 units above water that is at least 300 units deep there, and the water surface height.
fn spot_above_water(s: &mut Session) -> ([f32; 3], f32) {
    let p = s.player_location();
    let mut best: Option<(f32, [f32; 3], f32)> = None;
    for gx in -40..=40 {
        for gy in -40..=40 {
            let x = p[0] + gx as f32 * 128.0;
            let y = p[1] + gy as f32 * 128.0;
            // walk down from high up to the first water point
            let mut z = p[2] + 400.0;
            while z > p[2] - 1200.0 && !in_water(s, [x, y, z]) {
                z -= 16.0;
            }
            if z <= p[2] - 1200.0 || !in_water(s, [x, y, z - 300.0]) || !in_water(s, [x, y, z - 150.0]) {
                continue;
            }
            // open air above the surface
            let top = [x, y, z + 200.0];
            if !s.vm.bsp.as_ref().unwrap().fast_line_check(top, [x, y, z - 300.0]) {
                continue;
            }
            let d = (gx * gx + gy * gy) as f32;
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, top, z));
            }
        }
    }
    let (_, top, z) = best.expect("no open water near the start");
    (top, z)
}

#[test]
fn the_player_swims_in_water() {
    let Some(mut s) = new_game() else { return };
    let (spot, surface) = spot_above_water(&mut s);
    eprintln!("water surface at z {surface:.0} below {spot:?}");
    s.teleport(spot, 0.0);
    // fall in
    let mut entered = false;
    for _ in 0..90 {
        run(&mut s, 1);
        if physics(&mut s) == PHYS_SWIMMING {
            entered = true;
            break;
        }
    }
    assert!(entered, "the player did not start swimming (physics {}, at {:?}, log {:?})", physics(&mut s), s.player_location(), s.vm.log);
    let z_enter = s.player_location()[2];
    eprintln!("swimming at z {z_enter:.0}, state {:?}", s.vm.state_name(s.player));
    assert_eq!(s.vm.state_name(s.player), Some("PlayerSwimming"));
    assert!(z_enter < surface + 60.0, "entered above the water: {z_enter} vs {surface}");

    // with no input the fall into the water is braked: the player does not sink to the bottom at falling speed
    run(&mut s, 60);
    let v = match s.vm.get_prop(s.player, "Velocity", 0) {
        Ok(Value::Vector(v)) => v,
        _ => [0.0; 3],
    };
    eprintln!("after 2 s: z {:.0}, velocity {v:?}", s.player_location()[2]);
    assert!(v[2].abs() < 150.0, "still falling fast in water: {v:?}");

    // swim forward: the script keeps `bIsWalking` set in water ("don't let the player run if swimming"), so the
    // speed is the slow-walk cap, 0.3 * WaterSpeed
    let water_speed = s.vm.get_prop(s.player, "WaterSpeed", 0).ok().and_then(|v| v.as_f32()).unwrap();
    let a = s.player_location();
    s.input.key(key_code("W").unwrap(), true);
    run(&mut s, 30);
    s.input.key(key_code("W").unwrap(), false);
    let b = s.player_location();
    let moved = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    eprintln!("swam {moved:.0} units in 1 s (WaterSpeed {water_speed})");
    assert!(moved > water_speed * 0.2 && moved < water_speed * 0.32, "swam {moved} with WaterSpeed {water_speed}");

    // jump (swim up) until the head is out: the player stays at the surface, still swimming
    s.input.key(key_code("Space").unwrap(), true);
    run(&mut s, 90);
    s.input.key(key_code("Space").unwrap(), false);
    let top = s.player_location();
    eprintln!("after swimming up: z {:.0} (surface {surface:.0}), physics {}", top[2], physics(&mut s));
    assert!(top[2] > surface - 80.0 && top[2] < surface + 80.0, "not at the surface: {top:?}");
    assert!(matches!(physics(&mut s), PHYS_SWIMMING | PHYS_FALLING));
}
