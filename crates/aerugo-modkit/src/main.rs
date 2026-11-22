//! `deusex-modkit`: the mod kit of Deus Ex Aerugo. Finds the game's models and textures, shows them (animated, with
//! the skins the game uses), exports them to files common tools open (glTF 2.0 and PNG) and turns an edited model back
//! into a mesh mod for the `Mods` folder. It reads the game folder and never writes into it.

mod catalog;
mod font;
mod gltf;
mod render;
mod viewer;

use catalog::{Catalog, Kind, LoadedMesh, Picture};
use render::{Camera, Canvas, DrawOptions};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const USAGE: &str = "\
deusex-modkit - the mod kit of Deus Ex Aerugo (reads the game folder, never writes into it)

usage: deusex-modkit [--game <Deus Ex folder>] <command> ...

  view [<name>]                       the viewer window: search, models (animated) and textures
  search <words>... [--meshes|--textures]
                                      meshes and textures whose object path holds every word
  info <mesh>                         frames, sequences, texture slots, skins, open edges
  render <mesh> -o <file.png> [--frame N | --seq <Name> [--at T]] [--yaw D] [--pitch D] [--dist X]
         [--fov D] [--size WxH] [--skin K] [--mod] [--edges] [--back] [--wire]
                                      a picture of a mesh (--mod: with the mods folder's replacement)
  render --fp <WeaponClass> -o <file.png> [--fov D] [--size WxH] [--frame N | --seq ...] [--look-yaw D] ...
                                      the weapon in hand as the game places it (CalcDrawOffset, Hor+ field of
                                      view; --fov is the player's FOV, default 75)
  export <mesh> [--out <folder>]      the mesh as glTF 2.0 (every frame a morph target, every sequence an
                                      animation) with its textures as PNG
  export-obj <mesh> [--frame N | --seq <Name> [--at T]] [--fp <WeaponClass> | --stored] -o <file.obj>
                                      one frame as OBJ (+ .mtl and textures), placed as the game draws it (x
                                      forward, y right, z up turned to OBJ's y up); --fp: in the player's view;
                                      --stored: in the glTF export's space (no RotOrigin); vertex N is the mesh's N
  export-texture <texture>... [--out <folder>]
                                      textures as PNG at the place a mod's replacement goes:
                                      <folder>/Textures/<Package>/<Group>/<Name>.png
  import <file.gltf|.glb> --mod <mod folder> [--mesh <Package.Name>] [--no-bind]
                                      an edited model back into a mesh mod: <mod folder>/Meshes/<Package>/<Name>.aemesh
                                      (new vertices that do not move follow the nearest original face unless --no-bind)

<mesh>/<texture>: an object path (DeusExItems.GEPGun) or a name found once (GEPGun).
The game folder: --game, else the current folder or the one the program is in, else DEUSEX_DIR.
Output goes to --out (default: ModKit in the current folder).";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let game = take_value(&mut args, "--game").map(PathBuf::from).or_else(find_game);
    let Some(game) = game else { fail("no Deus Ex folder found: pass --game <folder> (the one with System/)") };
    let cmd = if args.is_empty() { "view".to_string() } else { args.remove(0) };
    let r = match cmd.as_str() {
        "view" => viewer::run(&game, &mut args),
        "search" => search(&game, &mut args),
        "info" => info(&game, &mut args),
        "render" => render_cmd(&game, &mut args),
        "export" => export(&game, &mut args),
        "export-texture" | "export-textures" => export_textures(&game, &mut args),
        "export-obj" => export_obj(&game, &mut args),
        "import" => import(&game, &mut args),
        _ => Err(format!("unknown command {cmd}\n\n{USAGE}")),
    };
    if let Err(e) = r {
        fail(&e);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("deusex-modkit: {msg}");
    std::process::exit(1);
}

fn is_game(p: &Path) -> bool {
    p.join("System").is_dir() && (p.join("Maps").is_dir() || p.join("Textures").is_dir())
}

fn find_game() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let mut c: Vec<PathBuf> = Vec::new();
    for p in [cwd, exe].into_iter().flatten() {
        // the program may sit in the game folder or in its System folder
        c.push(p.clone());
        if let Some(up) = p.parent() {
            c.push(up.to_path_buf());
        }
    }
    c.extend(std::env::var_os("DEUSEX_DIR").map(PathBuf::from));
    c.into_iter().find(|p| is_game(p))
}

pub fn take_flag(args: &mut Vec<String>, f: &str) -> bool {
    let n = args.len();
    args.retain(|a| a != f);
    args.len() != n
}

pub fn take_value(args: &mut Vec<String>, f: &str) -> Option<String> {
    let i = args.iter().position(|a| a == f)?;
    if i + 1 >= args.len() {
        fail(&format!("{f} needs a value"));
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Some(v)
}

fn take_num<T: std::str::FromStr>(args: &mut Vec<String>, f: &str) -> Result<Option<T>, String> {
    match take_value(args, f) {
        Some(v) => v.parse().map(Some).map_err(|_| format!("{f}: bad value {v}")),
        None => Ok(None),
    }
}

fn out_dir(args: &mut Vec<String>) -> PathBuf {
    take_value(args, "--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ModKit"))
}

fn one_arg(args: &mut Vec<String>, what: &str) -> Result<String, String> {
    if args.is_empty() {
        return Err(format!("which {what}?\n\n{USAGE}"));
    }
    Ok(args.remove(0))
}

fn search(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let kind = if take_flag(args, "--meshes") { Some(Kind::Mesh) } else if take_flag(args, "--textures") { Some(Kind::Texture) } else { None };
    let cat = Catalog::open(game)?;
    let hits = cat.search(&args.join(" "), kind);
    for e in &hits {
        println!("{:<8} {}", e.class, e.path);
    }
    eprintln!("{} found", hits.len());
    Ok(())
}

/// Edges used by one face only: where the model is open.
pub fn open_edges(m: &LoadedMesh) -> Vec<(usize, usize)> {
    let mut n: std::collections::HashMap<(usize, usize), u32> = std::collections::HashMap::new();
    for f in &m.lod.faces {
        let v = f.wedge.map(|w| m.lod.wedges[w as usize].vertex as usize);
        for k in 0..3 {
            let (a, b) = (v[k], v[(k + 1) % 3]);
            *n.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut e: Vec<(usize, usize)> = n.into_iter().filter(|&(_, c)| c == 1).map(|(e, _)| e).collect();
    e.sort();
    e
}

fn info(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let with_mod = take_flag(args, "--mod");
    let name = one_arg(args, "mesh")?;
    let mut cat = Catalog::open(game)?;
    let path = cat.find(&name, Kind::Mesh)?.path.clone();
    let m = cat.mesh(&path, with_mod)?;
    let l = &m.lod;
    println!("{} ({})", m.path, if m.plain { "Mesh" } else { "LodMesh" });
    println!("  {} frames, {} vertices a frame ({} special), {} faces, {} wedges", l.mesh.anim_frames, l.mesh.frame_verts, l.special_verts(), l.faces.len(), l.wedges.len());
    println!("  scale {:?}, origin {:?}, RotOrigin {:?}", l.mesh.scale, l.mesh.origin, l.mesh.rot_origin);
    for (i, s) in m.slots.iter().enumerate() {
        println!("  slot {i}: {}", s.as_deref().unwrap_or("-"));
    }
    for (i, mat) in l.materials.iter().enumerate() {
        println!("  material {i}: flags {:#x}, slot {}", mat[0], mat[1]);
    }
    for (name, start, n, rate) in &m.seqs {
        println!("  sequence {name}: frames {start}..{} at {rate}/s", start + n);
    }
    for s in cat.skins_for(&m.path) {
        let multi: Vec<String> = s.multi.iter().enumerate().filter_map(|(i, t)| t.as_ref().map(|t| format!("{i}={t}"))).collect();
        println!("  skins of {}: Skin {} MultiSkins [{}]", s.source, s.skin.as_deref().unwrap_or("-"), multi.join(", "));
    }
    println!("  {} open edges", open_edges(&m).len());
    Ok(())
}

/// The pictures of every slot of `m` under skin set `skin` (an index into `Catalog::skins_for`; `None` for the mesh's
/// own textures).
pub fn slot_pictures(cat: &mut Catalog, m: &LoadedMesh, skin: Option<usize>) -> (Vec<Option<Rc<Picture>>>, Vec<Option<String>>, Option<String>) {
    let sets = cat.skins_for(&m.path);
    let set = skin.and_then(|k| sets.get(k));
    let n = m.slots.len().max(m.lod.materials.iter().map(|x| x[1] as usize + 1).max().unwrap_or(0)).max(1);
    let names: Vec<Option<String>> = (0..n).map(|i| Catalog::slot_texture(m, set, i)).collect();
    let pics = names.iter().map(|p| p.as_ref().and_then(|p| cat.picture(p).ok()).map(Rc::new)).collect();
    (pics, names, set.map(|s| s.source.clone()))
}

/// The frames to show for `--frame` / `--seq --at`.
fn pick_frame(m: &LoadedMesh, frame: Option<usize>, seq: Option<&str>, at: f32) -> Result<(usize, usize, f32), String> {
    if let Some(s) = seq {
        let (_, start, n, _) = m.seqs.iter().find(|x| x.0.eq_ignore_ascii_case(s)).ok_or_else(|| format!("no sequence {s}"))?;
        let t = (at.clamp(0.0, 1.0) * *n as f32).min(*n as f32 - 1.0);
        let f = start + t as usize;
        return Ok((f, (f + 1).min(start + n - 1), t.fract()));
    }
    let f = frame.unwrap_or(0).min(m.lod.mesh.anim_frames.max(1) as usize - 1);
    Ok((f, f, 0.0))
}

fn render_cmd(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let out = take_value(args, "-o").ok_or("render needs -o <file.png>")?;
    let frame = take_num::<usize>(args, "--frame")?;
    let seq = take_value(args, "--seq");
    let at = take_num::<f32>(args, "--at")?.unwrap_or(0.0);
    let yaw = take_num::<f32>(args, "--yaw")?.unwrap_or(150.0);
    let pitch = take_num::<f32>(args, "--pitch")?.unwrap_or(-15.0);
    let dist = take_num::<f32>(args, "--dist")?;
    let take_fov = take_num::<f32>(args, "--fov")?;
    let fov = take_fov.unwrap_or(60.0);
    let skin = take_num::<usize>(args, "--skin")?;
    let size = take_value(args, "--size").unwrap_or_else(|| "1280x800".into());
    let (w, h) = size.split_once('x').and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))).ok_or("--size WxH")?;
    let opt = DrawOptions { open_edges: take_flag(args, "--edges"), back_faces: take_flag(args, "--back"), wireframe: take_flag(args, "--wire") };
    let with_mod = take_flag(args, "--mod");
    let fp = take_value(args, "--fp");
    let look_yaw = take_num::<f32>(args, "--look-yaw")?.unwrap_or(0.0);
    let look_pitch = take_num::<f32>(args, "--look-pitch")?.unwrap_or(0.0);
    let mut cat = Catalog::open(game)?;
    let (name, held) = match &fp {
        Some(class) => {
            let (mesh, offset, scale) = catalog::first_person(game, class)?;
            eprintln!("{class}: {mesh} at {offset:?} from the eye, scale {scale}");
            (mesh, Some((offset, scale)))
        }
        None => (one_arg(args, "mesh")?, None),
    };
    let path = cat.find(&name, Kind::Mesh)?.path.clone();
    let m = cat.mesh(&path, with_mod)?;
    let (fa, fb, t) = pick_frame(&m, frame, seq.as_deref(), at)?;
    let mut pos = render::pose(&m, fa, fb, t);
    let cam = if let Some((offset, scale)) = held {
        // the item at its offset from the eye (`catalog::first_person`), turned with the view, scaled by
        // `PlayerViewScale`; the eye looks along +x with the player's FOV (Hor+: the 4:3 angle widens with the frame)
        let player_fov = take_fov.unwrap_or(75.0);
        pos = render::pose_at(&m, fa, fb, t, offset, [0; 3], scale);
        let ht = (player_fov.to_radians() * 0.5).tan() * ((w as f32 / h as f32) / (4.0 / 3.0)).max(1.0);
        Camera { pos: [0.0; 3], yaw: look_yaw, pitch: look_pitch, fov: 2.0 * ht.atan().to_degrees() }
    } else {
        let (lo, hi) = render::bounds(&pos);
        let centre: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let radius = render::dot(render::sub(hi, lo), render::sub(hi, lo)).sqrt() * 0.5;
        Camera::orbit(centre, dist.unwrap_or(radius / (fov.to_radians() * 0.5).tan() * 1.1), yaw, pitch, fov)
    };
    // the first skin set the game uses, unless asked otherwise
    let sets = cat.skins_for(&m.path).len();
    let skin = skin.or((sets > 0).then_some(0));
    let (pics, names, source) = slot_pictures(&mut cat, &m, skin);
    let mut c = Canvas::new(w, h);
    c.clear([28, 32, 44]);
    let drawn = render::draw_mesh(&mut c, &cam, &m, &pos, &pics, opt);
    write_png(Path::new(&out), w as u32, h as u32, &c.rgba)?;
    println!("{out}: {} frame {fa}{} ({drawn} faces drawn), skins {} {:?}", m.path, if t > 0.0 { format!("+{t:.2}") } else { String::new() }, source.as_deref().unwrap_or("(the mesh's own)"), names);
    Ok(())
}

/// A compressed PNG (RGBA8).
pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| format!("{}: {e}", path.display()))
}

/// Where `export-texture` puts a texture: `Textures/<Package>/<Group>/<Name>.png`.
pub fn texture_file(out: &Path, path: &str) -> PathBuf {
    let mut p = out.join("Textures");
    let parts: Vec<&str> = path.split('.').collect();
    for (i, s) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            p = p.join(format!("{s}.png"));
        } else {
            p = p.join(s);
        }
    }
    p
}

fn export_textures(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let out = out_dir(args);
    let cat = Catalog::open(game)?;
    if args.is_empty() {
        return Err("which textures?".into());
    }
    for name in args.iter() {
        let e = cat.find(name, Kind::Texture)?;
        let p = cat.picture(&e.path)?;
        let file = texture_file(&out, &e.path);
        write_png(&file, p.w, p.h, &p.rgba)?;
        println!("{}", file.display());
    }
    Ok(())
}

fn export(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let out = out_dir(args);
    let skin = take_num::<usize>(args, "--skin")?;
    let with_mod = take_flag(args, "--mod");
    let name = one_arg(args, "mesh")?;
    let mut cat = Catalog::open(game)?;
    let path = cat.find(&name, Kind::Mesh)?.path.clone();
    let m = cat.mesh(&path, with_mod)?;
    let sets = cat.skins_for(&m.path).len();
    let (pics, names, _) = slot_pictures(&mut cat, &m, skin.or((sets > 0).then_some(0)));
    let file = gltf::export(&m, &pics, &names, &out)?;
    println!("{}", file.display());
    Ok(())
}

fn import(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let mod_dir = take_value(args, "--mod").map(PathBuf::from).ok_or("import needs --mod <mod folder>")?;
    let mesh = take_value(args, "--mesh");
    let bind = !take_flag(args, "--no-bind");
    let file = PathBuf::from(one_arg(args, "glTF file")?);
    let cat = Catalog::open(game)?;
    let (target, report) = gltf::import(&cat, &file, mesh.as_deref(), bind, &mod_dir)?;
    println!("{}: {report}", target.display());
    Ok(())
}

fn export_obj(game: &Path, args: &mut Vec<String>) -> Result<(), String> {
    let out = PathBuf::from(take_value(args, "-o").ok_or("export-obj needs -o <file.obj>")?);
    let frame = take_num::<usize>(args, "--frame")?;
    let seq = take_value(args, "--seq");
    let at = take_num::<f32>(args, "--at")?.unwrap_or(0.0);
    let fp = take_value(args, "--fp");
    let with_mod = take_flag(args, "--mod");
    let stored = take_flag(args, "--stored");
    let mut cat = Catalog::open(game)?;
    let (name, held) = match &fp {
        Some(class) => {
            let (mesh, offset, scale) = catalog::first_person(game, class)?;
            (mesh, Some((offset, scale)))
        }
        None => (one_arg(args, "mesh")?, None),
    };
    let path = cat.find(&name, Kind::Mesh)?.path.clone();
    let m = cat.mesh(&path, with_mod)?;
    let (fa, fb, t) = pick_frame(&m, frame, seq.as_deref(), at)?;
    let pos = match held {
        Some((o, s)) => render::pose_at(&m, fa, fb, t, o, [0; 3], s),
        // the glTF export's space: (stored - Origin) * Scale, no RotOrigin
        None if stored => ue_assets::mesh_mod::raw_frame(&m.lod, fa).iter().map(|p| std::array::from_fn(|k| (p[k] - m.lod.mesh.origin[k]) * m.lod.mesh.scale[k])).collect(),
        None => render::pose(&m, fa, fb, t),
    };
    let sets = cat.skins_for(&m.path).len();
    let (pics, _, _) = slot_pictures(&mut cat, &m, (sets > 0).then_some(0));
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh").to_string();
    let dir = out.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut obj = format!("# {} frame {fa} (deusex-modkit)\nmtllib {stem}.mtl\n", m.path);
    let mut mtl = String::new();
    for p in &pos {
        obj += &format!("v {} {} {}\n", p[0], p[2], p[1]);
    }
    let l = &m.lod;
    for w in &l.wedges {
        obj += &format!("vt {} {}\n", w.uv[0] as f32 / 256.0, 1.0 - w.uv[1] as f32 / 256.0);
    }
    for (mi, mat) in l.materials.iter().enumerate() {
        let tex = pics.get(mat[1] as usize).cloned().flatten();
        mtl += &format!("newmtl m{mi}_slot{}\nKd 1 1 1\n", mat[1]);
        if let Some(p) = tex {
            let file = format!("{stem}_slot{}.png", mat[1]);
            write_png(&dir.join(&file), p.w, p.h, &p.rgba)?;
            mtl += &format!("map_Kd {file}\n");
        }
        obj += &format!("usemtl m{mi}_slot{}\n", mat[1]);
        for f in l.faces.iter().filter(|f| f.material as usize == mi) {
            // the y/z swap mirrors: the corners go in reverse to keep the faces outward
            let c: Vec<String> = [0usize, 2, 1].iter().map(|&k| format!("{}/{}", l.wedge_vertex(f.wedge[k] as usize) + 1, f.wedge[k] + 1)).collect();
            obj += &format!("f {}\n", c.join(" "));
        }
    }
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(&out, obj).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::write(dir.join(format!("{stem}.mtl")), mtl).map_err(|e| e.to_string())?;
    println!("{}", out.display());
    Ok(())
}
