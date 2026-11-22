//! The game's models and textures: an index of every mesh and texture of the packages in the game folder, searched by
//! name, and their loading (geometry, pictures, the skins the game draws a mesh with).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use ue_assets::files::{GameFiles, Loaded};
use ue_assets::mesh::{LodMesh, Mesh};
use ue_assets::texture::{Format, Palette, Texture};
use ue_package::{ObjectRef, Package};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mesh,
    Texture,
}

/// One mesh or texture of the game: its object path (`Package.Group.Name`) and class.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub class: String,
    pub kind: Kind,
}

/// Texture classes whose objects carry a picture (procedural ones show their base picture).
const TEXTURE_CLASSES: &[&str] = &["Texture", "FireTexture", "WaterTexture", "WaveTexture", "WetTexture", "IceTexture", "ScriptedTexture"];

pub struct Catalog {
    pub game: PathBuf,
    pub files: GameFiles,
    pub entries: Vec<Entry>,
    skins: Option<HashMap<String, Vec<SkinSet>>>,
}

/// The textures a class of the game draws a mesh with: its `Skin` and `MultiSkins` defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinSet {
    /// `Class.Property`, e.g. `DeusEx.WeaponGEPGun.PlayerViewMesh`.
    pub source: String,
    pub skin: Option<String>,
    pub multi: [Option<String>; 8],
}

/// A mesh as the tool works with it.
pub struct LoadedMesh {
    pub path: String,
    pub lod: LodMesh,
    /// A plain `Mesh` (no level-of-detail data).
    pub plain: bool,
    /// Object paths of the mesh's own texture slots.
    pub slots: Vec<Option<String>>,
    /// Names of the sequences (resolved from the package's name table), with their first frame and frame count.
    pub seqs: Vec<(String, usize, usize, f32)>,
}

/// A picture in RGBA8.
#[derive(Clone)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Drawn masked (texel 0 transparent).
    pub masked: bool,
}

impl Catalog {
    /// Indexes the game folder `game` (`System/*.u`, `Textures/*.utx`); reads it, never writes to it.
    pub fn open(game: &Path) -> Result<Catalog, String> {
        if !game.join("System").is_dir() {
            return Err(format!("{} is not a Deus Ex folder (no System/)", game.display()));
        }
        let files = GameFiles::new(game);
        let mut entries = Vec::new();
        for (folder, ext) in [("System", "u"), ("Textures", "utx")] {
            let Ok(rd) = std::fs::read_dir(game.join(folder)) else { continue };
            let mut names: Vec<String> = rd
                .flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_str()?.to_string();
                    let (stem, x) = n.rsplit_once('.')?;
                    x.eq_ignore_ascii_case(ext).then(|| stem.to_string())
                })
                .collect();
            names.sort_by_key(|n| n.to_ascii_lowercase());
            for name in names {
                let Some(pkg) = files.package(&name) else { continue };
                for e in 0..pkg.pkg.exports.len() {
                    let class = pkg.pkg.export_class_name(e);
                    let kind = match class.as_str() {
                        "LodMesh" | "Mesh" => Kind::Mesh,
                        c if TEXTURE_CLASSES.contains(&c) => Kind::Texture,
                        _ => continue,
                    };
                    let Some(p) = pkg.pkg.object_path(ObjectRef::Export(e)) else { continue };
                    entries.push(Entry { path: format!("{name}.{p}"), class: class.to_string(), kind });
                }
            }
        }
        Ok(Catalog { game: game.to_path_buf(), files, entries, skins: None })
    }

    /// Entries whose path holds every word of `query` (any case); `kind` limits them to meshes or textures.
    pub fn search(&self, query: &str, kind: Option<Kind>) -> Vec<&Entry> {
        let words: Vec<String> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
        self.entries
            .iter()
            .filter(|e| kind.is_none_or(|k| e.kind == k))
            .filter(|e| {
                let p = e.path.to_ascii_lowercase();
                words.iter().all(|w| p.contains(w.as_str()))
            })
            .collect()
    }

    /// The entry with exactly this object path (any case), else the only one whose path ends in `.<name>`.
    pub fn find(&self, name: &str, kind: Kind) -> Result<&Entry, String> {
        let l = name.to_ascii_lowercase();
        if let Some(e) = self.entries.iter().find(|e| e.kind == kind && e.path.eq_ignore_ascii_case(name)) {
            return Ok(e);
        }
        let tail = format!(".{l}");
        let m: Vec<&Entry> = self.entries.iter().filter(|e| e.kind == kind && e.path.to_ascii_lowercase().ends_with(&tail)).collect();
        match m.len() {
            1 => Ok(m[0]),
            0 => Err(format!("no {} called {name} (try `search`)", if kind == Kind::Mesh { "mesh" } else { "texture" })),
            _ => Err(format!("{name} is ambiguous: {}", m.iter().map(|e| e.path.as_str()).collect::<Vec<_>>().join(", "))),
        }
    }

    fn export(&self, path: &str) -> Result<(Rc<Loaded>, usize), String> {
        let (pkg_name, rest) = path.split_once('.').ok_or_else(|| format!("{path}: not an object path"))?;
        let pkg = self.files.package(pkg_name).ok_or_else(|| format!("package {pkg_name} not found"))?;
        let e = pkg.find(rest).ok_or_else(|| format!("{path} not found"))?;
        Ok((pkg, e))
    }

    /// Loads a mesh; `with_mod` applies the mods folder's replacement geometry as the game would.
    pub fn mesh(&self, path: &str, with_mod: bool) -> Result<LoadedMesh, String> {
        let (pkg, e) = self.export(path)?;
        let class = pkg.pkg.export_class_name(e);
        let (lod, plain) = match class.as_str() {
            "LodMesh" => (LodMesh::load(&pkg.pkg, e).map_err(|e| format!("{path}: {e}"))?, false),
            "Mesh" => (LodMesh::from_plain(Mesh::load(&pkg.pkg, e).map_err(|e| format!("{path}: {e}"))?), true),
            c => return Err(format!("{path} is a {c}, not a mesh")),
        };
        let full = format!("{}.{}", pkg.name, pkg.pkg.object_path(ObjectRef::Export(e)).unwrap_or_default());
        let lod = if with_mod { ue_assets::mesh_mod::with_mod(&full, lod) } else { lod };
        let slots = lod
            .mesh
            .textures
            .iter()
            .map(|&r| {
                let (tp, te) = self.files.resolve(&pkg, r)?;
                tp.pkg.object_path(ObjectRef::Export(te)).map(|p| format!("{}.{p}", tp.name))
            })
            .collect();
        let seqs = lod.mesh.anim_seqs.iter().map(|s| (name(&pkg.pkg, s.name), s.start_frame.max(0) as usize, s.num_frames.max(1) as usize, s.rate)).collect();
        Ok(LoadedMesh { path: full, lod, plain, slots, seqs })
    }

    /// A texture's top mip level in RGBA8 (the mods folder's replacement picture is not used: this is the game's).
    pub fn picture(&self, path: &str) -> Result<Picture, String> {
        let (pkg, e) = self.export(path)?;
        let t = Texture::load_any(&pkg.pkg, e).map_err(|e| format!("{path}: {e}"))?;
        let pal = if t.palette == 0 && t.format != Format::P8 {
            Palette::none()
        } else {
            match self.files.resolve(&pkg, t.palette) {
                Some((pp, pe)) => Palette::load(&pp.pkg, pe).map_err(|e| format!("{path} palette: {e}"))?,
                None => Palette::none(),
            }
        };
        let rgba = t.mip_rgba(0, &pal).map_err(|e| format!("{path}: {e}"))?;
        Ok(Picture { w: t.mips[0].u_size, h: t.mips[0].v_size, rgba, masked: t.masked })
    }

    /// The skin sets the game's classes draw `mesh` with (classes whose `Mesh`, `PlayerViewMesh`, `PickupViewMesh` or
    /// `ThirdPersonMesh` default is that mesh). Loads the script packages on first use.
    pub fn skins_for(&mut self, mesh: &str) -> Vec<SkinSet> {
        if self.skins.is_none() {
            self.skins = Some(class_skins(&self.game).unwrap_or_else(|e| {
                eprintln!("skins: {e}");
                HashMap::new()
            }));
        }
        self.skins.as_ref().unwrap().get(&mesh.to_ascii_lowercase()).cloned().unwrap_or_default()
    }

    /// The textures mesh slot `i` is drawn with under `skins`, as `Actor.GetMeshTexture` picks them: `MultiSkins[i]`,
    /// else the mesh's own texture of slot `i` (for slots past 0), else `Skin`, else the slot's own texture.
    pub fn slot_texture(mesh: &LoadedMesh, skins: Option<&SkinSet>, i: usize) -> Option<String> {
        let own = mesh.slots.get(i).cloned().flatten();
        let Some(s) = skins else { return own };
        if let Some(m) = s.multi.get(i).cloned().flatten() {
            return Some(m);
        }
        if i > 0 && own.is_some() {
            return own;
        }
        s.skin.clone().or(own)
    }
}

/// How a class is held in first person by the player (`Handedness`): its `PlayerViewMesh`, the offset from the eye
/// (`DeusExWeapon.CalcDrawOffset`: `0.9 / DeusExPlayer.Default.FOVAngle * PlayerViewOffset`, the offset scaled by
/// 100 in `Weapon.SetHand`) and its `PlayerViewScale`.
pub fn first_person(game: &Path, class: &str) -> Result<(String, [f32; 3], f32), String> {
    let mut reg = ue_registry::Registry::load_game(game).map_err(|e| e.to_string())?;
    let id = reg.class_by_name(class).ok_or_else(|| format!("no class {class}"))?;
    let inst = reg.instantiate(id).map_err(|e| format!("{class}: {e:?}"))?;
    let mesh = reg.get_i32(&inst, "PlayerViewMesh", 0).filter(|&v| v != 0).ok_or_else(|| format!("{class} has no PlayerViewMesh"))?;
    let mesh = reg.obj_path(ue_registry::ObjId(mesh as u32)).to_string();
    let off = reg.find_prop(id, "PlayerViewOffset").map(|p| p.offset as usize).ok_or("no PlayerViewOffset")?;
    let f = |k: usize| f32::from_le_bytes(inst.data[off + k * 4..off + k * 4 + 4].try_into().unwrap());
    let scale = reg.get_f32(&inst, "PlayerViewScale", 0).unwrap_or(1.0);
    let player = reg.class_by_name("DeusExPlayer").ok_or("no DeusExPlayer class")?;
    let pinst = reg.instantiate(player).map_err(|e| format!("DeusExPlayer: {e:?}"))?;
    let fov = reg.get_f32(&pinst, "FOVAngle", 0).filter(|&f| f > 0.0).unwrap_or(75.0);
    // `Weapon.SetHand(Handedness)` (-1: right hand, as configured by default; 0: centred)
    let hand = reg.get_f32(&pinst, "Handedness", 0).unwrap_or(-1.0);
    let o = if hand == 0.0 { [f(0) * 0.88, -0.2 * f(1), f(2) * 1.12] } else { [f(0), f(1) * hand, f(2)] };
    Ok((mesh, o.map(|c| 0.9 / fov * c * 100.0), scale))
}

fn name(pkg: &Package, i: i32) -> String {
    pkg.name(i).to_string()
}

/// Every class default that names a mesh, by lower-case mesh path.
fn class_skins(game: &Path) -> Result<HashMap<String, Vec<SkinSet>>, String> {
    let mut reg = ue_registry::Registry::load_game(game).map_err(|e| e.to_string())?;
    let actor = reg.class_by_name("Actor").ok_or("no Actor class")?;
    let mut out: HashMap<String, Vec<SkinSet>> = HashMap::new();
    for c in 0..reg.class_count() {
        let id = ue_registry::ClassId(c as u32);
        if !reg.chain(id).contains(&actor) {
            continue;
        }
        let Ok(inst) = reg.instantiate(id) else { continue };
        let obj = |reg: &ue_registry::Registry, n: &str, k: u32| reg.get_i32(&inst, n, k).filter(|&v| v != 0).map(|v| reg.obj_path(ue_registry::ObjId(v as u32)).to_string());
        let skin = obj(&reg, "Skin", 0);
        let multi: [Option<String>; 8] = std::array::from_fn(|k| obj(&reg, "MultiSkins", k as u32));
        let class_path = reg.class(id).path.clone();
        for prop in ["Mesh", "PlayerViewMesh", "PickupViewMesh", "ThirdPersonMesh"] {
            let Some(m) = obj(&reg, prop, 0) else { continue };
            let set = SkinSet { source: format!("{class_path}.{prop}"), skin: skin.clone(), multi: multi.clone() };
            let v = out.entry(m.to_ascii_lowercase()).or_default();
            // subclasses repeat their parent's defaults: keep the first of identical sets
            if !v.iter().any(|s| s.skin == set.skin && s.multi == set.multi) {
                v.push(set);
            }
        }
    }
    Ok(out)
}
