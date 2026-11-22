//! glTF 2.0 export and import of the game's vertex-animated meshes.
//!
//! Export: one mesh node named after the object path; positions of frame 0 in Unreal units (stored units minus
//! `Origin`, times `Scale`; `RotOrigin` is not applied, so the model keeps the axes it was made in), converted to glTF
//! axes (`x, z, y`: y up); one morph target per animation frame (`frame_000`, ... in `extras.targetNames`, target `f`
//! holding frame `f` minus frame 0) and one animation per sequence that steps the target weights; texture
//! coordinates as the game maps them (`uv / 256`, from the top left); one material per texture slot and surface flags
//! (named `slot<N>` plus `-masked`, `-translucent`, `-modulated`, `-twosided` or `-f0x<hex>`), its picture as a PNG
//! next to the file.
//!
//! Import: any glTF 2.0 file (`.gltf` with its buffers, or `.glb`) of the same mesh, as Blender and other tools write
//! it back: the first mesh found (or the one named after the object path), every triangle primitive, the morph targets
//! matched to the frames by name (`frame_NNN`) or else by order, the node transforms applied. Vertices that share
//! their position in every frame are welded into one model vertex; triangles keep their texture coordinates (rounded
//! to the game's 1/256 steps) and get slot and flags from their material's name. New vertices that do not move in any
//! frame (added in an editor after the animation was made) can be bound to the nearest original face, so they follow
//! it through every frame.

use crate::catalog::{Catalog, Kind, LoadedMesh, Picture};
use crate::render::{cross, dot, sub, V3};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use ue_assets::mesh_mod::{GeoTri, Geometry};

const FLAG_NAMES: &[(u32, &str)] = &[(0x2, "masked"), (0x4, "translucent"), (0x40, "modulated"), (0x100, "twosided")];

fn material_name(slot: u8, flags: u32) -> String {
    let mut s = format!("slot{slot}");
    let mut rest = flags;
    for &(f, n) in FLAG_NAMES {
        if flags & f != 0 {
            s += &format!("-{n}");
            rest &= !f;
        }
    }
    if rest != 0 {
        s += &format!("-f0x{rest:x}");
    }
    s
}

/// Slot and flags from a material name written by `material_name` (any prefix before `slot`, e.g. Blender's
/// `slot0-masked.001`, is fine).
fn parse_material(name: &str) -> (u8, u32) {
    let l = name.to_ascii_lowercase();
    let Some(i) = l.find("slot") else { return (0, 0) };
    let rest = &l[i + 4..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let slot = digits.parse().unwrap_or(0);
    let mut flags = 0;
    for part in rest.split(['-', '.', '_']) {
        if let Some(&(f, _)) = FLAG_NAMES.iter().find(|(_, n)| *n == part) {
            flags |= f;
        } else if let Some(h) = part.strip_prefix("f0x") {
            flags |= u32::from_str_radix(h, 16).unwrap_or(0);
        }
    }
    (slot, flags)
}

/// Stored units -> glTF (Unreal units, y up).
fn to_gltf(m: &LoadedMesh, p: V3) -> V3 {
    let s = &m.lod.mesh;
    let u: V3 = std::array::from_fn(|k| (p[k] - s.origin[k]) * s.scale[k]);
    [u[0], u[2], u[1]]
}

/// glTF -> stored units.
fn from_gltf(m: &LoadedMesh, g: V3) -> V3 {
    let s = &m.lod.mesh;
    let u = [g[0], g[2], g[1]];
    std::array::from_fn(|k| u[k] / s.scale[k] + s.origin[k])
}

struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Bin {
    fn push(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn vec3(&mut self, v: &[V3], bounds: bool) -> usize {
        let bytes: Vec<u8> = v.iter().flat_map(|p| p.iter().flat_map(|c| c.to_le_bytes())).collect();
        let view = self.push(&bytes, Some(34962));
        let mut a = json!({"bufferView": view, "componentType": 5126, "count": v.len(), "type": "VEC3"});
        if bounds {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in v {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            if v.is_empty() {
                (lo, hi) = ([0.0; 3], [0.0; 3]);
            }
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn floats(&mut self, v: &[f32], ty: &str, minmax: bool) -> usize {
        let bytes: Vec<u8> = v.iter().flat_map(|c| c.to_le_bytes()).collect();
        let view = self.push(&bytes, None);
        let n = if ty == "VEC2" { v.len() / 2 } else { v.len() };
        let mut a = json!({"bufferView": view, "componentType": 5126, "count": n, "type": ty});
        if minmax {
            a["min"] = json!([v.iter().copied().fold(f32::MAX, f32::min)]);
            a["max"] = json!([v.iter().copied().fold(f32::MIN, f32::max)]);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }
}

/// Writes `<out>/<Package>/<Name>/<Name>.gltf` (+ `.bin` and the slot pictures). Returns the .gltf path.
pub fn export(m: &LoadedMesh, pics: &[Option<Rc<Picture>>], names: &[Option<String>], out: &Path) -> Result<PathBuf, String> {
    let (pkg, name) = m.path.split_once('.').unwrap_or(("", &m.path));
    let short = name.rsplit('.').next().unwrap_or(name);
    let dir = out.join(pkg).join(short);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let g = Geometry::from_lod(&m.lod);
    // glTF vertices: one per distinct (model vertex, uv)
    let mut index: HashMap<(u16, [u8; 2]), u32> = HashMap::new();
    let mut verts: Vec<(u16, [u8; 2])> = Vec::new();
    let mut prims: Vec<((u8, u32), Vec<u32>)> = Vec::new();
    for t in &g.tris {
        let key = (t.slot, t.flags);
        let pi = match prims.iter().position(|p| p.0 == key) {
            Some(i) => i,
            None => {
                prims.push((key, Vec::new()));
                prims.len() - 1
            }
        };
        // the game's front face is clockwise in glTF's axes (the axis swap mirrors): reverse the corners
        for k in [0usize, 2, 1] {
            let id = *index.entry((t.vertex[k], t.uv[k])).or_insert_with(|| {
                verts.push((t.vertex[k], t.uv[k]));
                (verts.len() - 1) as u32
            });
            prims[pi].1.push(id);
        }
    }
    let mut bin = Bin { data: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let base: Vec<V3> = verts.iter().map(|&(v, _)| to_gltf(m, g.frames[0][v as usize])).collect();
    let pos = bin.vec3(&base, true);
    let uv: Vec<f32> = verts.iter().flat_map(|&(_, uv)| [uv[0] as f32 / 256.0, uv[1] as f32 / 256.0]).collect();
    let uv = bin.floats(&uv, "VEC2", false);
    let nf = g.frames.len();
    let targets: Vec<usize> = (0..nf)
        .map(|f| {
            let d: Vec<V3> = verts.iter().zip(&base).map(|(&(v, _), b)| sub(to_gltf(m, g.frames[f][v as usize]), *b)).collect();
            bin.vec3(&d, true)
        })
        .collect();
    let mut materials = Vec::new();
    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut primitives = Vec::new();
    let mut tex_of_slot: HashMap<u8, usize> = HashMap::new();
    for ((slot, flags), idx) in &prims {
        let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = bin.push(&bytes, Some(34963));
        bin.accessors.push(json!({"bufferView": view, "componentType": 5125, "count": idx.len(), "type": "SCALAR"}));
        let indices = bin.accessors.len() - 1;
        let tex = match tex_of_slot.get(slot) {
            Some(&t) => Some(t),
            None => pics.get(*slot as usize).cloned().flatten().map(|p| {
                let file = format!("{short}_slot{slot}.png");
                let _ = crate::write_png(&dir.join(&file), p.w, p.h, &p.rgba);
                let src = names.get(*slot as usize).cloned().flatten().unwrap_or_default();
                images.push(json!({"uri": file, "name": src}));
                textures.push(json!({"source": images.len() - 1, "sampler": 0}));
                tex_of_slot.insert(*slot, textures.len() - 1);
                textures.len() - 1
            }),
        };
        let mut mat = json!({"name": material_name(*slot, *flags), "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0}});
        if let Some(t) = tex {
            mat["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": t});
        }
        if flags & 0x2 != 0 {
            mat["alphaMode"] = json!("MASK");
        }
        if flags & 0x100 != 0 {
            mat["doubleSided"] = json!(true);
        }
        materials.push(mat);
        primitives.push(json!({
            "attributes": {"POSITION": pos, "TEXCOORD_0": uv},
            "indices": indices,
            "material": materials.len() - 1,
            "mode": 4,
            "targets": targets.iter().map(|&t| json!({"POSITION": t})).collect::<Vec<_>>(),
        }));
    }
    let target_names: Vec<String> = (0..nf).map(|f| format!("frame_{f:03}")).collect();
    let mut weights0 = vec![0.0f32; nf];
    weights0[0] = 1.0;
    let mut animations = Vec::new();
    for (sname, start, n, rate) in &m.seqs {
        let rate = if *rate > 0.0 { *rate } else { 10.0 };
        let frames: Vec<usize> = (*start..start + n).filter(|&f| f < nf).collect();
        if frames.is_empty() {
            continue;
        }
        let times: Vec<f32> = (0..frames.len()).map(|i| i as f32 / rate).collect();
        let w: Vec<f32> = frames.iter().flat_map(|&f| (0..nf).map(move |k| if k == f { 1.0 } else { 0.0 })).collect();
        let input = bin.floats(&times, "SCALAR", true);
        let output = bin.floats(&w, "SCALAR", false);
        animations.push(json!({
            "name": sname,
            "samplers": [{"input": input, "output": output, "interpolation": "LINEAR"}],
            "channels": [{"sampler": 0, "target": {"node": 0, "path": "weights"}}],
        }));
    }
    let bin_name = format!("{short}.bin");
    std::fs::write(dir.join(&bin_name), &bin.data).map_err(|e| format!("{}: {e}", dir.join(&bin_name).display()))?;
    let doc = json!({
        "asset": {"version": "2.0", "generator": "deusex-modkit (Deus Ex Aerugo)"},
        "scene": 0,
        "scenes": [{"nodes": [0], "name": m.path}],
        "nodes": [{"mesh": 0, "name": m.path}],
        "meshes": [{"name": m.path, "primitives": primitives, "weights": weights0, "extras": {"targetNames": target_names}}],
        "materials": materials,
        "textures": textures,
        "images": images,
        "samplers": [{"magFilter": 9728, "minFilter": 9728}],
        "animations": animations,
        "buffers": [{"uri": bin_name, "byteLength": bin.data.len()}],
        "bufferViews": bin.views,
        "accessors": bin.accessors,
        "extras": {"aerugo": {"mesh": m.path, "frames": nf, "scale": m.lod.mesh.scale, "origin": m.lod.mesh.origin}},
    });
    let file = dir.join(format!("{short}.gltf"));
    std::fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(file)
}

/// A parsed glTF file with its buffers.
struct Doc {
    json: Value,
    buffers: Vec<Vec<u8>>,
}

fn load(file: &Path) -> Result<Doc, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (json_bytes, glb_bin) = if bytes.starts_with(b"glTF") {
        let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
        let jl = u(12);
        let json = bytes.get(20..20 + jl).ok_or("bad .glb")?.to_vec();
        let bo = 20 + jl;
        let bin = if bytes.len() >= bo + 8 { Some(bytes[bo + 8..bo + 8 + u(bo)].to_vec()) } else { None };
        (json, bin)
    } else {
        (bytes, None)
    };
    let json: Value = serde_json::from_slice(&json_bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    let dir = file.parent().unwrap_or(Path::new("."));
    let mut buffers = Vec::new();
    for (i, b) in json["buffers"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        let data = match b["uri"].as_str() {
            None => glb_bin.clone().ok_or(format!("buffer {i} has no data"))?,
            Some(uri) if uri.starts_with("data:") => {
                let b64 = uri.split_once(",").map(|x| x.1).unwrap_or("");
                base64(b64)?
            }
            Some(uri) => {
                let p = dir.join(percent_decode(uri));
                std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?
            }
        };
        buffers.push(data);
    }
    Ok(Doc { json, buffers })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        acc = acc << 6 | val(c).ok_or("bad base64 data")?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

impl Doc {
    /// The elements of accessor `i` as floats (normalised integers scaled to 0..1), `n` components each.
    fn floats(&self, i: usize) -> Result<(Vec<f32>, usize), String> {
        let a = &self.json["accessors"][i];
        let n = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            t => return Err(format!("accessor {i}: type {t:?}")),
        };
        let count = a["count"].as_u64().unwrap_or(0) as usize;
        let ct = a["componentType"].as_u64().unwrap_or(0);
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let mut out = vec![0.0f32; count * n];
        let read = |out: &mut [f32], view: &Value, offset: usize, idx: Option<&[usize]>| -> Result<(), String> {
            let buf = self.buffers.get(view["buffer"].as_u64().unwrap_or(0) as usize).ok_or("no buffer")?;
            let base = view["byteOffset"].as_u64().unwrap_or(0) as usize + offset;
            let size = match ct {
                5126 | 5125 => 4,
                5123 | 5122 => 2,
                _ => 1,
            };
            let stride = view["byteStride"].as_u64().map(|s| s as usize).unwrap_or(size * n);
            let rows = idx.map_or(out.len() / n, |x| x.len());
            for r in 0..rows {
                for k in 0..n {
                    let o = base + r * stride + k * size;
                    let b = buf.get(o..o + size).ok_or("accessor out of its buffer")?;
                    let v = match ct {
                        5126 => f32::from_le_bytes(b.try_into().unwrap()),
                        5125 => u32::from_le_bytes(b.try_into().unwrap()) as f32,
                        5123 => u16::from_le_bytes(b.try_into().unwrap()) as f32 / if normalized { 65535.0 } else { 1.0 },
                        5122 => i16::from_le_bytes(b.try_into().unwrap()) as f32 / if normalized { 32767.0 } else { 1.0 },
                        5121 => b[0] as f32 / if normalized { 255.0 } else { 1.0 },
                        5120 => b[0] as i8 as f32 / if normalized { 127.0 } else { 1.0 },
                        _ => return Err(format!("component type {ct}")),
                    };
                    let row = idx.map_or(r, |x| x[r]);
                    if let Some(d) = out.get_mut(row * n + k) {
                        *d = v;
                    }
                }
            }
            Ok(())
        };
        if let Some(v) = a["bufferView"].as_u64() {
            read(&mut out, &self.json["bufferViews"][v as usize], a["byteOffset"].as_u64().unwrap_or(0) as usize, None)?;
        }
        if let Some(sp) = a.get("sparse") {
            let cnt = sp["count"].as_u64().unwrap_or(0) as usize;
            let ict = sp["indices"]["componentType"].as_u64().unwrap_or(5125);
            let iv = &self.json["bufferViews"][sp["indices"]["bufferView"].as_u64().unwrap_or(0) as usize];
            let ibuf = &self.buffers[iv["buffer"].as_u64().unwrap_or(0) as usize];
            let ib = iv["byteOffset"].as_u64().unwrap_or(0) as usize + sp["indices"]["byteOffset"].as_u64().unwrap_or(0) as usize;
            let isz = match ict {
                5125 => 4,
                5123 => 2,
                _ => 1,
            };
            let idx: Vec<usize> = (0..cnt)
                .map(|r| {
                    let b = &ibuf[ib + r * isz..ib + r * isz + isz];
                    match isz {
                        4 => u32::from_le_bytes(b.try_into().unwrap()) as usize,
                        2 => u16::from_le_bytes(b.try_into().unwrap()) as usize,
                        _ => b[0] as usize,
                    }
                })
                .collect();
            let vv = &self.json["bufferViews"][sp["values"]["bufferView"].as_u64().unwrap_or(0) as usize];
            read(&mut out, vv, sp["values"]["byteOffset"].as_u64().unwrap_or(0) as usize, Some(&idx))?;
        }
        Ok((out, n))
    }

    /// Node transforms (column-major 4x4) of every node holding a mesh, from the scene's roots.
    fn mesh_nodes(&self) -> Vec<(usize, [f32; 16])> {
        let nodes = self.json["nodes"].as_array().cloned().unwrap_or_default();
        let local = |n: &Value| -> [f32; 16] {
            if let Some(m) = n["matrix"].as_array() {
                return std::array::from_fn(|i| m.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32);
            }
            let v = |k: &str, d: &[f32]| -> Vec<f32> { n[k].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or(d.to_vec()) };
            let (t, r, s) = (v("translation", &[0.0; 3]), v("rotation", &[0.0, 0.0, 0.0, 1.0]), v("scale", &[1.0; 3]));
            let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
            let rot = [
                [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
                [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
                [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
            ];
            let mut m = [0.0f32; 16];
            for c in 0..3 {
                for r in 0..3 {
                    m[c * 4 + r] = rot[c][r] * s[c];
                }
            }
            m[12..15].copy_from_slice(&t);
            m[15] = 1.0;
            m
        };
        let mul = |a: &[f32; 16], b: &[f32; 16]| -> [f32; 16] { std::array::from_fn(|i| (0..4).map(|k| a[k * 4 + i % 4] * b[(i / 4) * 4 + k]).sum()) };
        let mut out = Vec::new();
        let scene = self.json["scene"].as_u64().unwrap_or(0) as usize;
        let roots: Vec<usize> = self.json["scenes"][scene]["nodes"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64().map(|v| v as usize)).collect()).unwrap_or_else(|| (0..nodes.len()).collect());
        let mut stack: Vec<(usize, [f32; 16])> = roots.into_iter().map(|r| (r, std::array::from_fn(|i| if i % 5 == 0 { 1.0 } else { 0.0 }))).collect();
        while let Some((i, parent)) = stack.pop() {
            let Some(n) = nodes.get(i) else { continue };
            let m = mul(&parent, &local(n));
            if n["mesh"].is_u64() {
                out.push((i, m));
            }
            for c in n["children"].as_array().cloned().unwrap_or_default() {
                if let Some(c) = c.as_u64() {
                    stack.push((c as usize, m));
                }
            }
        }
        out.sort_by_key(|x| x.0);
        out
    }
}

/// Reads an edited model and writes it as a mesh mod. Returns the written file and a report.
pub fn import(cat: &Catalog, file: &Path, mesh: Option<&str>, bind: bool, mod_dir: &Path) -> Result<(PathBuf, String), String> {
    let doc = load(file)?;
    let nodes = doc.mesh_nodes();
    let mesh_name = |n: usize| doc.json["meshes"][doc.json["nodes"][n]["mesh"].as_u64().unwrap_or(0) as usize]["name"].as_str().unwrap_or("").to_string();
    let node_name = |n: usize| doc.json["nodes"][n]["name"].as_str().unwrap_or("").to_string();
    // the target mesh: given, else the file's own note, else a mesh or node named after an object path of the game
    let target = match mesh {
        Some(m) => m.to_string(),
        None => doc.json["extras"]["aerugo"]["mesh"]
            .as_str()
            .map(str::to_string)
            .or_else(|| nodes.iter().flat_map(|&(n, _)| [node_name(n), mesh_name(n)]).find(|s| cat.find(strip_suffix(s), Kind::Mesh).is_ok()).map(|s| strip_suffix(&s).to_string()))
            .ok_or("which mesh does this replace? pass --mesh <Package.Name>")?,
    };
    let path = cat.find(&target, Kind::Mesh)?.path.clone();
    let orig = cat.mesh(&path, false)?;
    let frames = orig.lod.mesh.anim_frames.max(1) as usize;
    // the node: the one named after the mesh, else the first
    let &(node, xf) = nodes.iter().find(|&&(n, _)| strip_suffix(&node_name(n)).eq_ignore_ascii_case(&path) || strip_suffix(&mesh_name(n)).eq_ignore_ascii_case(&path)).or(nodes.first()).ok_or("no mesh in the file")?;
    let gmesh = &doc.json["meshes"][doc.json["nodes"][node]["mesh"].as_u64().unwrap() as usize];
    let names: Vec<String> = gmesh["extras"]["targetNames"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
    let apply = |p: V3| -> V3 { std::array::from_fn(|r| xf[r] * p[0] + xf[4 + r] * p[1] + xf[8 + r] * p[2] + xf[12 + r]) };
    // every corner: its position in every frame (stored units) and its uv; welded by the rounded positions
    let mut points: Vec<Vec<V3>> = Vec::new();
    let mut point_of: HashMap<Vec<i32>, u16> = HashMap::new();
    let mut tris: Vec<GeoTri> = Vec::new();
    for prim in gmesh["primitives"].as_array().cloned().unwrap_or_default() {
        if prim["mode"].as_u64().unwrap_or(4) != 4 {
            continue;
        }
        let (pos, _) = doc.floats(prim["attributes"]["POSITION"].as_u64().ok_or("primitive without positions")? as usize)?;
        let nv = pos.len() / 3;
        let uv = match prim["attributes"]["TEXCOORD_0"].as_u64() {
            Some(a) => doc.floats(a as usize)?.0,
            None => vec![0.0; nv * 2],
        };
        let targets = prim["targets"].as_array().cloned().unwrap_or_default();
        // frame f's morph target
        let mut deltas: Vec<Option<Vec<f32>>> = vec![None; frames];
        for (ti, t) in targets.iter().enumerate() {
            let f = names.get(ti).and_then(|n| n.strip_prefix("frame_")).and_then(|d| d.parse::<usize>().ok()).unwrap_or(ti);
            if f < frames {
                if let Some(a) = t["POSITION"].as_u64() {
                    deltas[f] = Some(doc.floats(a as usize)?.0);
                }
            }
        }
        if targets.len() < frames && frames > 1 {
            return Err(format!("{} morph targets, the mesh has {frames} frames (export the shape keys)", targets.len()));
        }
        let corner_point = |v: usize, points: &mut Vec<Vec<V3>>, point_of: &mut HashMap<Vec<i32>, u16>| -> Result<u16, String> {
            let fr: Vec<V3> = (0..frames)
                .map(|f| {
                    let b = [pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]];
                    let d = deltas[f].as_ref().map(|d| [d[v * 3], d[v * 3 + 1], d[v * 3 + 2]]).unwrap_or([0.0; 3]);
                    from_gltf(&orig, apply([b[0] + d[0], b[1] + d[1], b[2] + d[2]]))
                })
                .collect();
            let key: Vec<i32> = fr.iter().flat_map(|p| p.map(|c| c.round() as i32)).collect();
            if let Some(&i) = point_of.get(&key) {
                return Ok(i);
            }
            if points.len() >= 65535 {
                return Err("more than 65535 vertices".into());
            }
            points.push(fr);
            point_of.insert(key, (points.len() - 1) as u16);
            Ok((points.len() - 1) as u16)
        };
        let idx: Vec<usize> = match prim["indices"].as_u64() {
            Some(a) => doc.floats(a as usize)?.0.into_iter().map(|x| x as usize).collect(),
            None => (0..nv).collect(),
        };
        let mname = prim["material"].as_u64().and_then(|m| doc.json["materials"][m as usize]["name"].as_str()).unwrap_or("");
        let (slot, flags) = parse_material(mname);
        for c in idx.chunks_exact(3) {
            // back to the game's corner order
            let corners = [c[0], c[2], c[1]];
            let mut vertex = [0u16; 3];
            let mut tuv = [[0u8; 2]; 3];
            for k in 0..3 {
                vertex[k] = corner_point(corners[k], &mut points, &mut point_of)?;
                tuv[k] = [0, 1].map(|j| (uv[corners[k] * 2 + j] * 256.0).round().clamp(0.0, 255.0) as u8);
            }
            if vertex[0] == vertex[1] || vertex[1] == vertex[2] || vertex[0] == vertex[2] {
                continue;
            }
            tris.push(GeoTri { vertex, uv: tuv, slot, flags });
        }
    }
    if tris.is_empty() {
        return Err("no triangles in the file".into());
    }
    let mut geo = Geometry { frames: (0..frames).map(|f| points.iter().map(|p| p[f]).collect()).collect(), tris };
    let bound = if bind { bind_new_vertices(&mut geo, &Geometry::from_lod(&orig.lod)) } else { 0 };
    let (pkg, name) = path.split_once('.').unwrap();
    let target_file = mod_dir.join("Meshes").join(pkg).join(format!("{}.aemesh", name.replace('.', "/")));
    // the result must fit the mesh before it is written
    geo.apply(&orig.lod)?;
    std::fs::create_dir_all(target_file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&target_file, geo.write()).map_err(|e| format!("{}: {e}", target_file.display()))?;
    let og = Geometry::from_lod(&orig.lod);
    Ok((target_file, format!("{path}: {} vertices (original {}), {} triangles (original {}), {frames} frames; {bound} new vertices bound to the nearest original face", geo.verts(), og.verts(), geo.tris.len(), og.tris.len())))
}

/// Blender adds `.001` to repeated names.
fn strip_suffix(s: &str) -> &str {
    match s.rsplit_once('.') {
        Some((a, b)) if b.len() == 3 && b.bytes().all(|c| c.is_ascii_digit()) => a,
        _ => s,
    }
}

/// Vertices that stay put in every frame, are not original vertices and sit on a mesh whose original moves: each
/// follows the original face nearest to it in frame 0 (kept in that face's frame: two edges and the normal).
/// Returns how many were bound.
pub fn bind_new_vertices(geo: &mut Geometry, orig: &Geometry) -> usize {
    let frames = geo.frames.len();
    if frames < 2 {
        return 0;
    }
    let originals: std::collections::HashSet<Vec<i32>> = (0..orig.verts()).map(|v| (0..frames).flat_map(|f| orig.frames[f][v].map(|c| c.round() as i32)).collect()).collect();
    let faces: Vec<[usize; 3]> = orig.tris.iter().map(|t| t.vertex.map(|v| v as usize)).filter(|v| area(orig, 0, *v) > 1e-3).collect();
    let mut n = 0;
    for v in 0..geo.verts() {
        let key: Vec<i32> = (0..frames).flat_map(|f| geo.frames[f][v].map(|c| c.round() as i32)).collect();
        if originals.contains(&key) || (1..frames).any(|f| geo.frames[f][v] != geo.frames[0][v]) {
            continue;
        }
        let p = geo.frames[0][v];
        let Some(face) = faces.iter().min_by(|a, b| dist_to_tri(p, orig, a).total_cmp(&dist_to_tri(p, orig, b))) else { continue };
        let Some(c) = local_coords(p, frame_axes(orig, 0, face)) else { continue };
        for f in 1..frames {
            let (o, e1, e2, nn) = frame_axes(orig, f, face);
            geo.frames[f][v] = std::array::from_fn(|k| o[k] + c[0] * e1[k] + c[1] * e2[k] + c[2] * nn[k]);
        }
        n += 1;
    }
    n
}

fn area(g: &Geometry, f: usize, t: [usize; 3]) -> f32 {
    let c = cross(sub(g.frames[f][t[1]], g.frames[f][t[0]]), sub(g.frames[f][t[2]], g.frames[f][t[0]]));
    dot(c, c).sqrt()
}

/// A face's origin, edges and normal (scaled to the edges' size, so the frame follows a uniform scaling too).
fn frame_axes(g: &Geometry, f: usize, t: &[usize; 3]) -> (V3, V3, V3, V3) {
    let o = g.frames[f][t[0]];
    let (e1, e2) = (sub(g.frames[f][t[1]], o), sub(g.frames[f][t[2]], o));
    let c = cross(e1, e2);
    let l = dot(c, c).sqrt().max(1e-9);
    let s = l.sqrt() / l;
    (o, e1, e2, c.map(|x| x * s))
}

/// `p - o` in the basis (e1, e2, n).
fn local_coords(p: V3, (o, e1, e2, n): (V3, V3, V3, V3)) -> Option<V3> {
    let d = sub(p, o);
    let det = dot(e1, cross(e2, n));
    if det.abs() < 1e-9 {
        return None;
    }
    Some([dot(d, cross(e2, n)) / det, dot(e1, cross(d, n)) / det, dot(e1, cross(e2, d)) / det])
}

fn dist_to_tri(p: V3, g: &Geometry, t: &[usize; 3]) -> f32 {
    let (a, b, c) = (g.frames[0][t[0]], g.frames[0][t[1]], g.frames[0][t[2]]);
    // closest point on the triangle (Ericson, Real-Time Collision Detection 5.1.5)
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let q = if d1 <= 0.0 && d2 <= 0.0 {
        a
    } else {
        let bp = sub(p, b);
        let (d3, d4) = (dot(ab, bp), dot(ac, bp));
        let cp = sub(p, c);
        let (d5, d6) = (dot(ab, cp), dot(ac, cp));
        let (vc, vb, va) = (d1 * d4 - d3 * d2, d5 * d2 - d1 * d6, d3 * d6 - d5 * d4);
        let at = |x: V3, y: V3, t: f32| -> V3 { std::array::from_fn(|k| x[k] + (y[k] - x[k]) * t) };
        if d3 >= 0.0 && d4 <= d3 {
            b
        } else if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            at(a, b, d1 / (d1 - d3))
        } else if d6 >= 0.0 && d5 <= d6 {
            c
        } else if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            at(a, c, d2 / (d2 - d6))
        } else if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            at(b, c, (d4 - d3) / ((d4 - d3) + (d5 - d6)))
        } else {
            let den = 1.0 / (va + vb + vc);
            let (v, w) = (vb * den, vc * den);
            std::array::from_fn(|k| a[k] + ab[k] * v + ac[k] * w)
        }
    };
    let d = sub(p, q);
    dot(d, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_names_carry_slot_and_flags() {
        for (s, f) in [(0u8, 0u32), (1, 2), (3, 0x104), (2, 0x8000_0002)] {
            assert_eq!(parse_material(&material_name(s, f)), (s, f));
        }
        assert_eq!(parse_material("slot1-masked.001"), (1, 2));
        assert_eq!(parse_material("Material"), (0, 0));
    }

    #[test]
    fn a_still_new_vertex_follows_its_face() {
        // one triangle moving up by 10 a frame and turning; a new vertex above it
        let tri = |f: f32| vec![[0.0, 0.0, 10.0 * f], [10.0, 0.0, 10.0 * f], [0.0, 0.0, 10.0 * f + 10.0]];
        let orig = Geometry { frames: (0..3).map(|f| tri(f as f32)).collect(), tris: vec![GeoTri { vertex: [0, 1, 2], uv: [[0; 2]; 3], slot: 0, flags: 0 }] };
        let mut g = orig.clone();
        for f in &mut g.frames {
            f.push([5.0, 3.0, 5.0]);
        }
        assert_eq!(bind_new_vertices(&mut g, &orig), 1);
        assert_eq!(g.frames[2][3], [5.0, 3.0, 25.0]);
        assert_eq!(g.frames[0][3], [5.0, 3.0, 5.0]);
    }

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
    }

    /// Triangles as (corner positions in every frame, uv) sets, rotated to a canonical first corner (vertex order free).
    fn canonical(g: &Geometry) -> Vec<(Vec<(Vec<[i32; 3]>, [u8; 2])>, u8, u32)> {
        let mut v: Vec<_> = g
            .tris
            .iter()
            .map(|t| {
                let c: Vec<(Vec<[i32; 3]>, [u8; 2])> = (0..3).map(|k| (g.frames.iter().map(|f| f[t.vertex[k] as usize].map(|x| x.round() as i32)).collect(), t.uv[k])).collect();
                let r = (0..3).min_by_key(|&k| c[k].clone()).unwrap();
                ((0..3).map(|k| c[(r + k) % 3].clone()).collect(), t.slot, t.flags)
            })
            .collect();
        v.sort();
        v
    }

    /// On the game's data (`DEUSEX_DIR`): a weapon, a character and a plain mesh exported to glTF and imported back
    /// give the same geometry (positions in every frame, corners in order, uvs, slots and flags).
    #[test]
    fn export_and_import_give_the_mesh_back() {
        let Some(game) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
        let mut cat = Catalog::open(&game).unwrap();
        let dir = std::env::temp_dir().join(format!("aerugo-modkit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plain = cat.entries.iter().find(|e| e.class == "Mesh").map(|e| e.path.clone()).unwrap();
        for name in ["DeusExItems.GEPGun", "DeusExCharacters.GM_Trench", plain.as_str()] {
            let m = cat.mesh(name, false).unwrap();
            let (pics, names, _) = crate::slot_pictures(&mut cat, &m, None);
            let file = export(&m, &pics, &names, &dir.join("export")).unwrap();
            let (out, _) = import(&cat, &file, None, true, &dir.join("mod")).unwrap();
            let back = Geometry::read(&std::fs::read(&out).unwrap()).unwrap();
            assert_eq!(canonical(&back), canonical(&Geometry::from_lod(&m.lod)), "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
