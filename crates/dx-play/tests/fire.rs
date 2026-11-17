//! Fire on the player and on pawns. Set `DEUSEX_DIR`.

use std::path::PathBuf;
use ue_vm::Value;

/// The tester's freeze with the player on fire: a `Fire` destroyed while burning calls `DelayedDestroy` on its smoke
/// generator, which may already be destroyed itself. The original runs the call (a destroyed actor stays callable until
/// `CleanupDestroyed`); the port failed to find the function, and the error ended the player's `Timer`, which burns
/// and kills the player.
#[test]
fn the_player_burns_without_script_errors() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&game, "02_NYC_Underground").unwrap();
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    let p = s.player;
    s.vm.call_by_name(p, "CatchFire", vec![Value::Obj(p)]).unwrap();
    for _ in 0..450 {
        s.step_bound(1.0 / 30.0);
    }
    let issues: Vec<String> = s.issues.iter().map(|i| format!("{} {}: {}", i.class, i.event, i.error)).collect();
    assert!(issues.is_empty(), "{issues:?}");
    assert!(matches!(s.vm.get_prop(s.player, "bOnFire", 0), Ok(Value::Bool(false))), "the fire went out");
}
