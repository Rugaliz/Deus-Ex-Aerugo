//! The mods folder: `<game>/Mods/`, where each mod is a folder or a `.zip` file. Mods are applied in name order
//! (case-insensitive); a later mod wins over an earlier one, and every mod wins over the game. A name starting with
//! `_` or `.` turns a mod off.
//!
//! What a mod can hold, anywhere inside it (sub-folders such as `System/`, `Textures/`, `Maps/` are optional):
//! - packages (`.u`, `.utx`, `.uax`, `.umx`, `.dx`, ...): replace the game's package of the same name, or add one
//!   (an added script package contributes its classes; an added map can be travelled to);
//! - ini files (`DeusEx.ini`, `User.ini`, ...): their keys go over the game's (the player's own settings still win);
//! - localised text (`<Package>.int`);
//! - replacement textures: `Textures/<Package>/<Group>/<Name>.png` (or `<Package>.<Group>.<Name>.png`), at any
//!   size; the texture keeps its original size for the game, only the picture changes;
//! - replacement mesh geometry: `Meshes/<Package>/<Name>.aemesh` (or `<Package>.<Name>.aemesh`), written by the mod
//!   kit (`deusex-modkit import`); the mesh keeps its animation sequences and texture slots (`ue_assets::mesh_mod`).
//!
//! The index is built once per game folder (`init`) and read through the free functions below.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// File extensions of packages.
pub const PACKAGE_EXTENSIONS: &[&str] = &["u", "utx", "dx", "uax", "umx", "unr", "ut", "usa"];

/// Where a file comes from: the disk or an entry of a zip file.
#[derive(Debug, Clone)]
pub enum Source {
    File(PathBuf),
    Zip { archive: Arc<PathBuf>, entry: String },
}

impl Source {
    /// The file's bytes.
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        match self {
            Source::File(p) => std::fs::read(p),
            Source::Zip { archive, entry } => {
                let mut zip = zip::ZipArchive::new(std::fs::File::open(archive.as_path())?).map_err(std::io::Error::other)?;
                let mut f = zip.by_name(entry).map_err(std::io::Error::other)?;
                let mut out = Vec::with_capacity(f.size() as usize);
                f.read_to_end(&mut out)?;
                Ok(out)
            }
        }
    }

    /// A readable description (`Mods/HiRes.zip:Textures/Foo.utx`).
    pub fn describe(&self) -> String {
        match self {
            Source::File(p) => p.display().to_string(),
            Source::Zip { archive, entry } => format!("{}:{entry}", archive.display()),
        }
    }
}

/// A package a mod provides: its file name's extension (lower case) and where it is.
#[derive(Debug, Clone)]
pub struct ModPackage {
    pub ext: String,
    pub source: Source,
    /// The mod it comes from.
    pub from: String,
}

/// Everything the mods folder provides.
#[derive(Debug, Default)]
pub struct Mods {
    /// The mods in the order they apply.
    pub names: Vec<String>,
    /// Packages by lower-case name (file stem); the last mod providing one wins.
    pub packages: HashMap<String, ModPackage>,
    /// Ini files by lower-case name, every mod's in order.
    pub inis: HashMap<String, Vec<Source>>,
    /// Localised text files (`.int`) by lower-case package name; the last mod wins.
    pub ints: HashMap<String, Source>,
    /// Replacement pictures by lower-case object path (`package.group.name`); the last mod wins.
    pub textures: HashMap<String, Source>,
    /// Replacement mesh geometry (`.aemesh`) by lower-case object path (`package.name`); the last mod wins.
    pub meshes: HashMap<String, Source>,
}

static INDEX: RwLock<Option<(PathBuf, Arc<Mods>)>> = RwLock::new(None);

/// Builds the index of `<game>/Mods/` (once per game folder; a call for the same folder again keeps it).
pub fn init(game_dir: &Path) -> Arc<Mods> {
    if let Some((dir, mods)) = INDEX.read().unwrap().as_ref() {
        if dir == game_dir {
            return mods.clone();
        }
    }
    let mods = Arc::new(scan(&game_dir.join("Mods")));
    *INDEX.write().unwrap() = Some((game_dir.to_path_buf(), mods.clone()));
    mods
}

/// The index for the game folder last passed to `init` (empty before that).
pub fn current() -> Arc<Mods> {
    INDEX.read().unwrap().as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
}

/// The mod package called `name` (any case), if a mod provides one.
pub fn package(name: &str) -> Option<ModPackage> {
    current().packages.get(&name.to_ascii_lowercase()).cloned()
}

/// The replacement picture of the texture `Package.Group.Name` (any case), if a mod provides one.
pub fn texture(path: &str) -> Option<Source> {
    let m = current();
    if m.textures.is_empty() {
        return None;
    }
    m.textures.get(&path.to_ascii_lowercase()).cloned()
}

/// The replacement geometry of the mesh `Package.Name` (any case), if a mod provides one.
pub fn mesh(path: &str) -> Option<Source> {
    let m = current();
    if m.meshes.is_empty() {
        return None;
    }
    m.meshes.get(&path.to_ascii_lowercase()).cloned()
}

fn scan(dir: &Path) -> Mods {
    let mut out = Mods::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    for p in entries {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('_') || name.starts_with('.') {
            continue;
        }
        let files: Vec<(String, Source)> = if p.is_dir() {
            let mut v = Vec::new();
            walk(&p, &p, &mut v);
            v
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            match zip_entries(&p) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("mods: cannot read {}: {e}", p.display());
                    continue;
                }
            }
        } else {
            continue;
        };
        let mod_name = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
        for (rel, source) in files {
            add(&mut out, &mod_name, &rel, source);
        }
        out.names.push(mod_name);
    }
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Source)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push((rel.to_string_lossy().replace('\\', "/"), Source::File(p.clone())));
        }
    }
}

fn zip_entries(path: &Path) -> zip::result::ZipResult<Vec<(String, Source)>> {
    let zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let archive = Arc::new(path.to_path_buf());
    Ok(zip
        .file_names()
        .filter(|n| !n.ends_with('/'))
        .map(|n| (n.replace('\\', "/"), Source::Zip { archive: archive.clone(), entry: n.to_string() }))
        .collect())
}

/// Files a mod provides, by their path inside the mod.
fn add(out: &mut Mods, mod_name: &str, rel: &str, source: Source) {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let Some((stem, ext)) = file.rsplit_once('.') else { return };
    let (stem_l, ext_l) = (stem.to_ascii_lowercase(), ext.to_ascii_lowercase());
    if PACKAGE_EXTENSIONS.contains(&ext_l.as_str()) {
        out.packages.insert(stem_l, ModPackage { ext: ext_l, source, from: mod_name.to_string() });
    } else if ext_l == "ini" {
        out.inis.entry(stem_l).or_default().push(source);
    } else if ext_l == "int" {
        out.ints.insert(stem_l, source);
    } else if ext_l == "png" {
        if let Some(key) = object_key(rel, "textures") {
            out.textures.insert(key, source);
        }
    } else if ext_l == "aemesh" {
        if let Some(key) = object_key(rel, "meshes") {
            out.meshes.insert(key, source);
        }
    }
}

/// The object path a replacement file stands for: the components after a `Textures` (`Meshes`) folder joined with
/// dots (`Textures/CoreTexMetal/Metal/Floor.png`), else the file name itself (`CoreTexMetal.Metal.Floor.png`).
fn object_key(rel: &str, folder: &str) -> Option<String> {
    let rel = rel.rsplit_once('.')?.0;
    let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    let after = parts.iter().rposition(|p| p.eq_ignore_ascii_case(folder)).map(|i| &parts[i + 1..]);
    let key = match after {
        Some(rest) if !rest.is_empty() => rest.join("."),
        _ => parts.last()?.to_string(),
    };
    key.contains('.').then(|| key.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_pictures_are_named_by_object_path() {
        let t = |r| object_key(r, "textures");
        assert_eq!(t("Textures/CoreTexMetal/Metal/Floor.png").as_deref(), Some("coretexmetal.metal.floor"));
        assert_eq!(t("MyPack/Textures/CoreTexMetal/Floor.png").as_deref(), Some("coretexmetal.floor"));
        assert_eq!(t("CoreTexMetal.Metal.Floor.png").as_deref(), Some("coretexmetal.metal.floor"));
        assert_eq!(t("readme.png"), None);
        assert_eq!(object_key("Meshes/DeusExItems/GEPGun.aemesh", "meshes").as_deref(), Some("deusexitems.gepgun"));
    }

    #[test]
    fn later_mods_win_and_underscore_turns_a_mod_off() {
        let dir = std::env::temp_dir().join(format!("aerugo-mods-test-{}", std::process::id()));
        let mods = dir.join("Mods");
        let _ = std::fs::remove_dir_all(&dir);
        for (m, f) in [("A", "Textures/Foo.utx"), ("B", "Foo.utx"), ("_C", "Foo.utx"), ("B", "System/DeusEx.ini"), ("A", "Meshes/Pkg/Gun.aemesh"), ("B", "Meshes/Pkg/Gun.aemesh")] {
            let p = mods.join(m).join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, m).unwrap();
        }
        // a zip mod
        {
            let f = std::fs::File::create(mods.join("Z.zip")).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("Pack/Textures/Pkg/Grp/Tex.png", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, b"png").unwrap();
            z.finish().unwrap();
        }
        let m = scan(&mods);
        assert_eq!(m.names, ["A", "B", "Z"]);
        assert_eq!(m.packages["foo"].from, "B");
        assert_eq!(m.packages["foo"].source.read().unwrap(), b"B");
        assert_eq!(m.inis["deusex"].len(), 1);
        assert_eq!(m.textures["pkg.grp.tex"].read().unwrap(), b"png");
        assert_eq!(m.meshes["pkg.gun"].read().unwrap(), b"B");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
