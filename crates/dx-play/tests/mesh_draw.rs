//! What `DrawActorSprite` hands `DrawLodMesh` for a mesh actor (`ue_vm::animation::MeshDraw`): the draw's poly flags
//! (Render 10b26230), the environment map and the `Matrix` texture. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

fn open() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

/// A visible mesh actor in a real zone.
fn mesh_actor(s: &mut Session) -> u32 {
    let actors = s.vm.live_actors();
    actors
        .into_iter()
        .find(|&a| a != s.player && s.vm.u8p(a, "DrawType") == 2 && s.vm.op(a, "Mesh") != 0 && !s.vm.bp(a, "bHidden") && s.vm.zone_number(a) != 0)
        .expect("a mesh actor")
}

fn flags(s: &mut Session, a: u32) -> u32 {
    s.vm.mesh_draw_opts(a, false).expect("drawn").flags
}

fn tex(s: &mut Session, path: &str) -> u32 {
    s.vm.reg.ensure_package_for_path(path);
    s.vm.reg.obj_id(path).0
}

/// 10b26230: Style 2/3/4 give 0x2/0x4/0x40 (0 and 1 nothing), bNoSmooth 0x800, bMeshEnviroMap 0x10, `PF_Flat` 0x4000
/// unless bMeshCurvy, `PF_Unlit` 0x400000 for bUnlit or a view whose `RendMap` is not 5.
#[test]
fn draw_flags_follow_the_actor() {
    let Some(mut s) = open() else { return };
    let a = mesh_actor(&mut s);
    for (name, v) in [("bUnlit", false), ("bNoSmooth", false), ("bMeshEnviroMap", false), ("bMeshCurvy", false), ("bSelected", false)] {
        s.vm.set_prop(a, name, 0, Value::Bool(v)).unwrap();
    }
    for (style, want) in [(0u8, 0x4000u32), (1, 0x4000), (2, 0x4002), (3, 0x4004), (4, 0x4040)] {
        s.vm.set_prop(a, "Style", 0, Value::Byte(style)).unwrap();
        assert_eq!(flags(&mut s, a), want, "Style {style}");
    }
    s.vm.set_prop(a, "Style", 0, Value::Byte(1)).unwrap();
    for (name, want) in [("bNoSmooth", 0x4800u32), ("bMeshEnviroMap", 0x4010), ("bMeshCurvy", 0), ("bUnlit", 0x40_4000)] {
        s.vm.set_prop(a, name, 0, Value::Bool(true)).unwrap();
        assert_eq!(flags(&mut s, a), want, "{name}");
        s.vm.set_prop(a, name, 0, Value::Bool(false)).unwrap();
    }
    // an unlit view (the Matrix cheat's `RMODE 6`)
    let p = s.player;
    s.vm.set_prop(p, "RendMap", 0, Value::Byte(6)).unwrap();
    assert_eq!(flags(&mut s, a), 0x40_4000, "RendMap 6");
    s.vm.set_prop(p, "RendMap", 0, Value::Byte(5)).unwrap();
    assert_eq!(flags(&mut s, a), 0x4000);
}

/// `DrawLodMesh`'s environment map: the actor's `Texture`, else its zone's `EnvironmentMap`, else the level's; the
/// viewing player's `Sprite` replaces every texture; `Fatness`, `AmbientGlow`, `LODBias` passed through.
#[test]
fn environment_map_and_mesh_inputs() {
    let Some(mut s) = open() else { return };
    let a = mesh_actor(&mut s);
    let (t1, t2, t3) = ("Effects.water.WtrFall_A02", "DeusExItems.Skins.ReflectionMapTex1", "DeusExItems.Skins.PinkMaskTex");
    let (o1, o2, o3) = (tex(&mut s, t1), tex(&mut s, t2), tex(&mut s, t3));
    let zone = s.vm.zone_of_actor(a);
    let level = s.vm.op(a, "Level");
    assert_ne!(zone, 0);
    s.vm.set_prop(a, "Texture", 0, Value::Obj(o1)).unwrap();
    s.vm.set_prop(zone, "EnvironmentMap", 0, Value::Obj(o2)).unwrap();
    if level != zone {
        s.vm.set_prop(level, "EnvironmentMap", 0, Value::Obj(o3)).unwrap();
    }
    let env = |s: &mut Session| s.vm.mesh_draw_opts(a, false).unwrap().env_texture;
    assert_eq!(env(&mut s).as_deref(), Some(t1));
    s.vm.set_prop(a, "Texture", 0, Value::Obj(0)).unwrap();
    assert_eq!(env(&mut s).as_deref(), Some(t2));
    s.vm.set_prop(zone, "EnvironmentMap", 0, Value::Obj(0)).unwrap();
    if level != zone {
        assert_eq!(env(&mut s).as_deref(), Some(t3));
        s.vm.set_prop(level, "EnvironmentMap", 0, Value::Obj(0)).unwrap();
    }
    assert_eq!(env(&mut s), None);

    s.vm.set_prop(a, "Fatness", 0, Value::Byte(140)).unwrap();
    s.vm.set_prop(a, "AmbientGlow", 0, Value::Byte(64)).unwrap();
    s.vm.set_prop(a, "LODBias", 0, Value::Float(2.5)).unwrap();
    let d = s.vm.mesh_draw_opts(a, false).unwrap();
    assert_eq!((d.fatness, d.ambient_glow, d.lod_bias, d.all_textures), (140, 64, 2.5, None));
    let p = s.player;
    s.vm.set_prop(p, "Sprite", 0, Value::Obj(o3)).unwrap();
    assert_eq!(s.vm.mesh_draw_opts(a, false).unwrap().all_textures.as_deref(), Some(t3));
}

/// The facing test on shipped meshes: their faces wind so that `(v1 - v0) x (v2 - v0)` points outward in world space
/// (a positive signed volume; the engine's `(v0 - v1) x (v2 - v0)` taken in its mirror-image view space), and
/// `DrawLodMesh` keeps the one-sided faces that turn toward the eye.
#[test]
fn shipped_meshes_face_outward_and_hidden_faces_are_dropped() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut view = dx_soft::MapView::open(&g, "01_NYC_UNATCOIsland").unwrap();
    for path in ["DeusExCharacters.GM_Trench", "DeusExDeco.CrateUnbreakableLarge", "DeusExDeco.NYPoliceBoat", "DeusExDeco.DXLogo", "DeusExDeco.IonStormLogo", "DeusExDeco.EidosLogo"] {
        let m = view.mesh_data(path).unwrap_or_else(|| panic!("{path}"));
        let lod = &m.lod;
        let pose = lod.frame_positions(0);
        let special = lod.special_verts();
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let mut volume = 0.0f64;
        for f in &lod.faces {
            let v: [[f32; 3]; 3] = std::array::from_fn(|k| pose[lod.wedges[f.wedge[k] as usize].vertex as usize + special]);
            volume += dot(v[0], cross(sub(v[1], v[0]), sub(v[2], v[0]))) as f64 / 6.0;
        }
        assert!(volume > 0.0, "{path}: signed volume {volume}");
        // from far along +x every kept one-sided face turns toward the eye
        let eye = [5000.0, 0.0, 0.0];
        let mview = dx_vk::lodmesh::MeshView { eye, forward: [-1.0, 0.0, 0.0], fov_deg: 90.0, frame_x: 640.0 };
        let actor = dx_vk::lodmesh::MeshActor { location: [0.0; 3], draw_scale: 1.0, lod_bias: 1.0, fatness: 128, flags: 0, glow: 0.5 };
        let world: Vec<[f32; 3]> = pose.clone();
        // the logos are plain meshes (`DrawMesh`)
        let tris = if m.plain { dx_vk::lodmesh::build_plain(&lod.mesh, &world, &mview, &actor, None) } else { dx_vk::lodmesh::build(lod, &world, &mview, &actor, None) };
        assert!(!tris.is_empty());
        for t in tris.iter().filter(|t| t.flags & dx_vk::lodmesh::PF_TWO_SIDED == 0) {
            let [a, b, c] = t.corners.map(|c| c.pos);
            assert!(dot(cross(sub(b, a), sub(c, a)), sub(a, eye)) < 1e-3, "{path}: a kept face turns away");
        }
    }
}
