//! Integration tests against a real Deus Ex installation. Set `DEUSEX_DIR` to the game folder; skipped otherwise.

use std::path::PathBuf;
use ue_registry::Registry;

fn game() -> Option<Registry> {
    let dir = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    Some(Registry::load_game(&dir).expect("load game"))
}

#[test]
fn layout_anchors_match_original_code() {
    let Some(reg) = game() else { return };
    let actor = reg.class_by_name("Actor").unwrap();
    let off = |n: &str| reg.find_prop(actor, n).unwrap().offset;
    // Offsets observed in field accesses of the original engine code.
    assert_eq!(off("Owner"), 0x3C);
    assert_eq!(off("Level"), 0xA8);
    assert_eq!(off("XLevel"), 0xAC);
    assert_eq!(off("CollisionRadius"), 0x1D4);
    assert_eq!(off("CollisionHeight"), 0x1D8);
    assert_eq!(off("bCollideActors"), 0x1DC);
    assert_eq!(reg.find_prop(actor, "bCollideActors").unwrap().bit_mask, 1);
    let object = reg.class_by_name("Object").unwrap();
    assert_eq!(reg.find_prop(object, "Class").unwrap().offset, 0x24);
}

#[test]
fn every_class_instantiates() {
    let Some(mut reg) = game() else { return };
    for i in 0..reg.classes.len() {
        reg.instantiate(ue_registry::ClassId(i as u32)).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn player_defaults() {
    let Some(mut reg) = game() else { return };
    let id = reg.class_by_name("DeusExPlayer").unwrap();
    let p = reg.instantiate(id).unwrap();
    assert_eq!(reg.get_string(&p, "TruePlayerName", 0), Some("JC Denton"));
    assert_eq!(reg.get_f32(&p, "CombatDifficulty", 0), Some(1.0));
    assert_eq!(reg.get_i32(&p, "SkillPointsTotal", 0), Some(5000));
    assert_eq!(reg.get_i32(&p, "Credits", 0), Some(500));
    assert_eq!(reg.get_f32(&p, "MaxFrobDistance", 0), Some(112.0));
    assert_eq!(reg.get_string(&p, "strStartMap", 0), Some("01_NYC_UNATCOIsland"));
    assert_eq!(reg.get_name(&p, "AugPrefs", 0), Some("AugVision"));
    assert_eq!(reg.get_name(&p, "AugPrefs", 8), Some("AugAqualung"));
    assert_eq!(reg.get_bool(&p, "bHelpMessages"), Some(true));
    // Inherited defaults from Actor still apply.
    assert_eq!(reg.get_f32(&p, "DrawScale", 0), Some(1.0));
}
