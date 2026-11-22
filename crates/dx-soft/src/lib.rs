//! CPU reference renderer: textured world BSP of a map, flat shaded, no lightmaps.
//! It exists to check geometry assembly and texture mapping before the Vulkan renderer; it is not fast.

pub mod mesh;
mod actors;
mod procedural;

pub mod replace;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use ue_assets::files::{vector_prop, GameFiles, Loaded};
use ue_assets::level::Level;
use ue_assets::model::Model;
use ue_assets::texture::{Palette, Texture};
use ue_package::ObjectRef;

/// Surface flags (`PF_*`) that matter here.
const PF_INVISIBLE: u32 = 0x1;
const PF_MASKED: u32 = 0x2;
const PF_FAKEBACKDROP: u32 = 0x80;
const PF_TWOSIDED: u32 = 0x100;
const PF_PORTAL: u32 = 0x0400_0000;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub pos: [f32; 3],
    /// Unreal rotator units (65536 per turn).
    pub pitch: i32,
    pub yaw: i32,
    /// Horizontal field of view in degrees.
    pub fov: f32,
    /// Roll around the view direction (rotator units; leaning, view shake).
    pub roll: i32,
}

impl Camera {
    /// Forward, right and up unit vectors, as Unreal's `GetAxes` of (pitch, yaw, roll).
    pub fn axes(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        // `FSceneNode::ComputeRenderCoords`: `GMath.ViewCoords /= Rotation` (right, down, forward) with the engine's
        // sine table
        let c = ue_bsp::scene::view_coords(self.pos, [self.pitch, self.yaw, self.roll]);
        (c.z, c.x, [-c.y[0], -c.y[1], -c.y[2]])
    }
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A frame being drawn: colour and depth (distance along the view axis), so that actors can be drawn into the world.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub color: Vec<u8>,
    pub depth: Vec<f32>,
}

impl Frame {
    pub fn into_image(self) -> Image {
        Image { width: self.width, height: self.height, rgba: self.color }
    }
}

/// Everything needed to draw one actor's mesh.
#[derive(Debug, Clone, Default)]
pub struct ActorDraw {
    /// Object path of the `Mesh` (for example `DeusExCharacters.GM_Trench`).
    pub mesh: String,
    /// Animation frames to blend (absolute frame numbers of the mesh) and the blend factor toward `frame_b`.
    pub frame_a: usize,
    pub frame_b: usize,
    pub blend: f32,
    pub location: [f32; 3],
    /// Unreal rotator units.
    pub rotation: [i32; 3],
    /// `PrePivot` (`GetFrame`: the mesh stands at `location + pre_pivot`).
    pub pre_pivot: [f32; 3],
    /// An explicit placement replacing `location`, `rotation` and the actor's pivot: origin and the world images of
    /// the mesh's x, y, z axes (a third-person item on its pawn's weapon triangle); `pre_pivot` applies inside it.
    pub frame: Option<[[f32; 3]; 4]>,
    pub scale: f32,
    /// Texture object paths per mesh texture index (`MultiSkins`, `Skin`); `None` keeps the mesh's own texture.
    pub skins: Vec<Option<String>>,
    /// The actor's `Style` (0 none, 1 normal, 2 masked, 3 translucent, 4 modulated): 2..=4 override the mesh's own material flags.
    pub style: u8,
    /// `ScaleGlow`: brightness of the surface, opacity of translucent and modulated ones.
    pub glow: f32,
    /// `bUnlit`: no shading.
    pub unlit: bool,
    /// Texture object path of a `DT_Sprite` actor: a camera-facing quad (then `mesh` is empty).
    pub sprite: Option<String>,
    /// Surface normal and roll (rotator units) when the sprite is a decal lying on a surface instead of facing the camera.
    /// A decal: the half-axes of its square (right, up), already sized.
    pub decal: Option<([f32; 3], [f32; 3])>,
    /// A decal's clipped pieces with texel coordinates (see `ue_vm::animation::MeshDraw::decal_polys`).
    pub decal_polys: Vec<Vec<([f32; 3], [f32; 2])>>,
    /// First-person weapon: drawn last on a cleared depth buffer.
    pub overlay: bool,
    /// How the mesh is lit (ambient, scale and the lights that reach it); `None` = no light information (drawn unlit).
    pub lighting: Option<MeshLighting>,
    /// The lights around an unlit mesh, for the ray-traced shadows it casts only (it is drawn unlit).
    pub shadow_lighting: Option<MeshLighting>,
    /// A corpse (`Carcass`): it casts no ray-traced shadow.
    pub carcass: bool,
    /// Blend channel poses: weighted absolute frames whose difference to frame 0 is added to the vertices.
    pub blends: Vec<(usize, f32)>,
    /// Place of the actor's sprite in the main view's sprite list (`DrawFrame` draws in this order); `u32::MAX` when
    /// there is no visibility result (then the renderer orders by distance).
    pub rank: u32,
    /// `DrawLodMesh`'s inputs: `Fatness`, `AmbientGlow`, `LODBias`, the draw's poly flags (Render 10b26230), the
    /// environment map and the texture that replaces all of the mesh's (see `ue_vm::animation::MeshDraw`).
    pub fatness: u8,
    pub ambient_glow: u8,
    pub lod_bias: f32,
    pub flags: u32,
    pub env_texture: Option<String>,
    pub all_textures: Option<String>,
}

/// The lighting of one mesh actor: each vertex gets `min(1, sum(lights) * scale + ambient)` (see `docs/actor-lighting.md`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshLighting {
    pub ambient: [f32; 3],
    pub scale: f32,
    /// World position, colour and radius of each light.
    pub lights: Vec<([f32; 3], [f32; 3], f32)>,
    /// The fog lights (`SetupForActor`'s volumetric entries, farthest first): world position, `VolumeRadius`,
    /// `VolumeBrightness`, `VolumeFog`, brightness of the moment and colour. Empty without `PF_RenderFog`.
    pub fog_lights: Vec<([f32; 3], u8, u8, u8, f32, [f32; 3])>,
}

/// Half-width tangent of the view for a horizontal field of view `fov` (degrees) meant for a 4:3 screen; wider screens
/// see more at the sides with the same vertical view ("Hor+"). Same rule as the Vulkan renderer.
pub fn hor_plus_tan(fov: f32, aspect: f32) -> f32 {
    (fov.to_radians() * 0.5).tan() * (aspect / (4.0 / 3.0)).max(1.0)
}

/// Camera axes (forward, right, up) for the rotator conventions of the engine.
pub(crate) fn camera_basis(cam: &Camera) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
    let (sp, cp) = ang(cam.pitch).sin_cos();
    let (sy, cy) = ang(cam.yaw).sin_cos();
    ([cp * cy, cp * sy, sp], [-sy, cy, 0.0], [-sp * cy, -sp * sy, cp])
}

pub struct Tex {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A map opened for rendering.
pub struct MapView {
    /// The lightmaps `ue_bsp::fill` made for the faces it added to `model` (shadowed once the lights are known).
    pub filled_light_maps: std::ops::Range<usize>,
    pub(crate) files: GameFiles,
    map: Rc<Loaded>,
    pub model: Model,
    pub level: Level,
    textures: HashMap<(String, usize), Option<Rc<Tex>>>,
    pub(crate) texture_paths: HashMap<String, Option<Rc<Tex>>>,
    pub(crate) meshes: HashMap<String, Option<Rc<mesh::MeshData>>>,
    /// Simulation state of the procedural textures drawn so far (`procedural.rs`).
    pub(crate) procedurals: HashMap<(String, usize), Option<procedural::Procedural>>,
    pub(crate) fractal: dx_fractal::Shared,
    /// Textures made while the game runs (`GenerateSnapshot`), by path: size and RGBA8 pixels. Drawn through the
    /// procedural path (their pixels can change), keyed `(path, usize::MAX)`.
    pub(crate) runtime: HashMap<String, (u32, u32, Vec<u8>)>,
}

impl MapView {
    pub fn open(game: &Path, map_name: &str) -> Result<MapView, String> {
        let files = GameFiles::new(game);
        let map = files.package(map_name).ok_or_else(|| format!("map {map_name} not found"))?;
        let level_export = (0..map.pkg.exports.len())
            .find(|&i| map.pkg.export_class_name(i) == "Level")
            .ok_or("no Level object")?;
        let level = Level::load(&map.pkg, level_export).map_err(|e| e.to_string())?;
        let model_export = match ObjectRef::from_index(level.model) {
            ObjectRef::Export(i) => i,
            _ => return Err("level model is not an export".into()),
        };
        let mut model = Model::load(&map.pkg, model_export).map_err(|e| e.to_string())?;
        // Aerugo repair: walls the shipped map left without a face get one (as the VM's copy of the level, so both
        // number the nodes and surfaces alike)
        let (_, filled_light_maps) = ue_bsp::fill::fill_level(&mut model, &map.pkg);
        Ok(MapView { filled_light_maps, files, map, model, level, textures: HashMap::new(), texture_paths: HashMap::new(), meshes: HashMap::new(), procedurals: HashMap::new(), fractal: dx_fractal::Shared::new(), runtime: HashMap::new() })
    }

    /// Location and rotation of the first actor of class `class` (for example `PlayerStart`).
    pub fn find_actor(&self, class: &str) -> Option<([f32; 3], [f32; 3])> {
        for &a in &self.level.actors {
            if let ObjectRef::Export(i) = ObjectRef::from_index(a) {
                if self.map.pkg.export_class_name(i) == class {
                    let loc = vector_prop(&self.map.pkg, i, "Location")?;
                    let rot = vector_prop(&self.map.pkg, i, "Rotation").unwrap_or([0.0; 3]);
                    return Some((loc, rot));
                }
            }
        }
        None
    }

    fn texture(&mut self, raw: i32) -> Option<Rc<Tex>> {
        let (pkg, export) = self.files.resolve(&self.map, raw)?;
        let key = (pkg.name.clone(), export);
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let tex = (|| {
            let t = Texture::load(&pkg.pkg, export).ok()?;
            let pal = if t.palette == 0 && t.format != ue_assets::texture::Format::P8 { Palette::none() } else {
                let (ppkg, pexp) = self.files.resolve(&pkg, t.palette)?;
                Palette::load(&ppkg.pkg, pexp).ok()?
            };
            let rgba = t.mip_rgba(0, &pal).ok()?;
            Some(Rc::new(Tex { w: t.mips[0].u_size, h: t.mips[0].v_size, rgba }))
        })();
        self.textures.insert(key, tex.clone());
        tex
    }

    /// Another `Model` object of the map package (a mover's brush).
    pub fn model_at(&self, export: usize) -> Option<Model> {
        Model::load(&self.map.pkg, export).ok()
    }

    /// The editor polygons (`Polys`) of the brush model at `export` of the map package (a mover's brush).
    pub fn brush_polys(&self, export: usize) -> Option<Vec<ue_assets::model::Poly>> {
        let m = self.model_at(export)?;
        let (pkg, e) = self.files.resolve(&self.map, m.polys)?;
        ue_assets::model::Polys::load(&pkg.pkg, e).ok().map(|p| p.polys)
    }

    /// Identity (package name, export index) of a surface's texture reference, for caching by other renderers.
    pub fn texture_key(&self, raw: i32) -> Option<(String, usize)> {
        let (pkg, export) = self.files.resolve(&self.map, raw)?;
        Some((pkg.name.clone(), export))
    }

    /// A surface texture's `DetailTexture` (Render passes the texture's +0x58 to the device): its object path and its
    /// size in texels times its `DrawScale` (`USize * UScale`, `VSize * VScale`). `None` when it has none.
    pub fn surface_detail(&self, raw: i32) -> Option<(String, [f32; 2])> {
        let (pkg, export) = self.files.resolve(&self.map, raw)?;
        let t = Texture::load_any(&pkg.pkg, export).ok()?;
        if t.detail_texture == 0 {
            return None;
        }
        let (dp, de) = self.files.resolve(&pkg, t.detail_texture)?;
        let d = Texture::load_any(&dp.pkg, de).ok()?;
        let path = format!("{}.{}", dp.name, dp.pkg.object_path(ue_package::ObjectRef::Export(de))?);
        Some((path, [d.u_size as f32 * d.draw_scale, d.v_size as f32 * d.draw_scale]))
    }

    /// The frames after a surface texture in its `AnimNext` chain (keys as `texture_key`), in order, stopping at the
    /// end of the chain or where it comes back to the first texture.
    pub fn anim_chain(&self, raw: i32) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let Some((mut pkg, mut export)) = self.files.resolve(&self.map, raw) else { return out };
        let head = (pkg.name.clone(), export);
        while out.len() < 256 {
            let Ok(t) = Texture::load_any(&pkg.pkg, export) else { break };
            if t.anim_next == 0 {
                break;
            }
            let Some((p, e)) = self.files.resolve(&pkg, t.anim_next) else { break };
            if (p.name.clone(), e) == head {
                break;
            }
            out.push((p.name.clone(), e));
            pkg = p;
            export = e;
        }
        out
    }

    /// Every mip level of a texture given by its key (`texture_key`, `anim_chain`).
    pub fn texture_mips_by_key(&self, key: &(String, usize)) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        let pkg = self.files.package(&key.0)?;
        self.mips_of(&pkg, key.1)
    }

    /// The key of a texture object path (`Package.Group.Name`).
    pub fn texture_key_of_path(&self, path: &str) -> Option<(String, usize)> {
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let export = pkg.find(rest)?;
        Some((pkg.name.clone(), export))
    }

    /// Every mip level of a surface's texture as RGBA8 `(width, height, pixels)`, largest first.
    pub fn texture_mips(&self, raw: i32) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        let (pkg, export) = self.files.resolve(&self.map, raw)?;
        self.mips_of(&pkg, export)
    }

    /// Same as `texture_mips`, for a texture object path (`Package.Group.Name`).
    pub fn texture_mips_by_path(&self, path: &str) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let export = pkg.find(rest)?;
        self.mips_of(&pkg, export)
    }

    /// Same as `texture_mips_by_path`, with palette index 0 transparent when the drawing asks for masking (`force`) or the
    /// texture is flagged masked itself (the canvas adds the texture's `PF_Masked` to the drawing's flags).
    /// Sets (or replaces) a texture made at run time: `w` x `h` RGBA8 pixels under `path`.
    pub fn set_runtime_texture(&mut self, path: &str, w: u32, h: u32, rgba: Vec<u8>) {
        self.runtime.insert(path.to_string(), (w, h, rgba));
    }

    pub fn texture_mips_by_path_masked(&self, path: &str, force: bool) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        if let Some((w, h, rgba)) = self.runtime.get(path) {
            return Some(vec![(*w, *h, rgba.clone())]);
        }
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let export = pkg.find(rest)?;
        if let Some(m) = self.replacement(&pkg, export) {
            return Some(m);
        }
        let t = Texture::load(&pkg.pkg, export).ok()?;
        let pal = if t.palette == 0 && t.format != ue_assets::texture::Format::P8 { Palette::none() } else {
            let (ppkg, pexp) = self.files.resolve(&pkg, t.palette)?;
            Palette::load(&ppkg.pkg, pexp).ok()?
        };
        let masked = force || t.masked;
        (0..t.mips.len()).map(|i| Some((t.mips[i].u_size, t.mips[i].v_size, t.mip_rgba_with(i, &pal, masked).ok()?))).collect()
    }

    /// A detail texture as D3DDrv's detail pass uploads it (`DrawComplexSurface` 1000488c, outside the editor): its
    /// palette's entry 0 set to opaque white first. Every mip level as RGBA8.
    pub fn detail_texture_mips(&self, path: &str) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let export = pkg.find(rest)?;
        let t = Texture::load(&pkg.pkg, export).ok()?;
        let mut pal = if t.palette == 0 && t.format != ue_assets::texture::Format::P8 { Palette::none() } else {
            let (ppkg, pexp) = self.files.resolve(&pkg, t.palette)?;
            Palette::load(&ppkg.pkg, pexp).ok()?
        };
        if let Some(c) = pal.colors.get_mut(0) {
            *c = [255, 255, 255, 255];
        }
        (0..t.mips.len()).map(|i| Some((t.mips[i].u_size, t.mips[i].v_size, t.mip_rgba_with(i, &pal, false).ok()?))).collect()
    }

    /// The `PolyFlags` of a surface's texture (map reference `raw`): its surface bools packed in flag order, with a fire
    /// texture's `bMasked` cleared as the engine does when it loads one. 0 when it cannot be read.
    pub fn texture_poly_flags(&self, raw: i32) -> u32 {
        (|| {
            let (pkg, export) = self.files.resolve(&self.map, raw)?;
            let t = Texture::load_any(&pkg.pkg, export).ok()?;
            let fire = pkg.pkg.export_class_name(export) == "FireTexture";
            Some(if fire { t.poly_flags & !PF_MASKED } else { t.poly_flags })
        })()
        .unwrap_or(0)
    }

    /// The texture's own `bMasked` (`PF_Masked`; the engine clears a fire texture's when it loads).
    pub fn texture_masked_by_path(&self, path: &str) -> bool {
        (|| {
            let (pkg_name, rest) = path.split_once('.')?;
            let pkg = self.files.package(pkg_name)?;
            let export = pkg.find(rest)?;
            let t = Texture::load_any(&pkg.pkg, export).ok()?;
            Some(t.masked && pkg.pkg.export_class_name(export) != "FireTexture")
        })()
        .unwrap_or(false)
    }

    /// The blend a texture asks for (0 none, 2 masked, 3 translucent, 4 modulated, the actor `Style` numbers).
    pub fn texture_style_by_path(&self, path: &str) -> u8 {
        (|| {
            let (pkg_name, rest) = path.split_once('.')?;
            let pkg = self.files.package(pkg_name)?;
            let export = pkg.find(rest)?;
            let t = Texture::load_any(&pkg.pkg, export).ok()?;
            // the engine clears a fire texture's masked flag when it loads
            let masked = t.masked && pkg.pkg.export_class_name(export) != "FireTexture";
            Some(if t.modulated {
                4
            } else if t.translucent {
                3
            } else if masked {
                2
            } else {
                0
            })
        })()
        .unwrap_or(0)
    }

    fn mips_of(&self, pkg: &Rc<Loaded>, export: usize) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        if let Some(m) = self.replacement(pkg, export) {
            return Some(m);
        }
        let t = Texture::load(&pkg.pkg, export).ok()?;
        // a wet texture's own pixels are generated at run time (the water simulation is not ported yet): show the
        // picture it distorts, undistorted
        if t.source_texture != 0 && pkg.pkg.export_class_name(export) == "WetTexture" {
            let (spkg, sexp) = self.files.resolve(pkg, t.source_texture)?;
            if !Rc::ptr_eq(&spkg, pkg) || sexp != export {
                return self.mips_of(&spkg, sexp);
            }
        }
        let pal = if t.palette == 0 && t.format != ue_assets::texture::Format::P8 { Palette::none() } else {
            let (ppkg, pexp) = self.files.resolve(pkg, t.palette)?;
            Palette::load(&ppkg.pkg, pexp).ok()?
        };
        (0..t.mips.len()).map(|i| Some((t.mips[i].u_size, t.mips[i].v_size, t.mip_rgba(i, &pal).ok()?))).collect()
    }

    /// A mod's replacement picture for a texture (`replace::replacement_mips`).
    fn replacement(&self, pkg: &Rc<Loaded>, export: usize) -> Option<Vec<(u32, u32, Vec<u8>)>> {
        if ue_package::mods::current().textures.is_empty() {
            return None;
        }
        let path = format!("{}.{}", pkg.name, pkg.pkg.object_path(ObjectRef::Export(export))?);
        replace::replacement_mips(&path)
    }

    /// The size the game knows a texture by (its first mip level in the package), whatever picture a mod puts in its
    /// place: what sprites and the canvas measure, and what world texture coordinates count in.
    pub fn texture_size_by_path(&self, path: &str) -> Option<(u32, u32)> {
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let t = Texture::load_any(&pkg.pkg, pkg.find(rest)?).ok()?;
        Some((t.u_size, t.v_size))
    }

    /// The size of a texture given by its key (`texture_key`, `anim_chain`) as the game knows it.
    pub fn texture_size_by_key(&self, key: &(String, usize)) -> Option<(u32, u32)> {
        let pkg = self.files.package(&key.0)?;
        let t = Texture::load_any(&pkg.pkg, key.1).ok()?;
        Some((t.u_size, t.v_size))
    }

    /// The size of a surface texture as the game knows it (see `texture_size_by_path`).
    pub fn texture_size(&self, raw: i32) -> Option<(u32, u32)> {
        let (pkg, export) = self.files.resolve(&self.map, raw)?;
        let t = Texture::load_any(&pkg.pkg, export).ok()?;
        Some((t.u_size, t.v_size))
    }

    /// Texture by object path (`Package.Group.Name`), cached.
    pub(crate) fn texture_by_path(&mut self, path: &str) -> Option<Rc<Tex>> {
        if let Some(t) = self.texture_paths.get(path) {
            return t.clone();
        }
        let tex = (|| {
            let (pkg_name, rest) = path.split_once('.')?;
            let pkg = self.files.package(pkg_name)?;
            let export = pkg.find(rest)?;
            let t = Texture::load(&pkg.pkg, export).ok()?;
            let pal = if t.palette == 0 && t.format != ue_assets::texture::Format::P8 { Palette::none() } else {
                let (ppkg, pexp) = self.files.resolve(&pkg, t.palette)?;
                Palette::load(&ppkg.pkg, pexp).ok()?
            };
            let rgba = t.mip_rgba(0, &pal).ok()?;
            Some(Rc::new(Tex { w: t.mips[0].u_size, h: t.mips[0].v_size, rgba }))
        })();
        self.texture_paths.insert(path.to_string(), tex.clone());
        tex
    }

    pub fn render(&mut self, cam: &Camera, width: u32, height: u32) -> Image {
        self.render_frame(cam, width, height).into_image()
    }

    /// Renders the world and returns the frame with its depth buffer, ready for actors to be drawn into.
    pub fn render_frame(&mut self, cam: &Camera, width: u32, height: u32) -> Frame {
        let (w, h) = (width as usize, height as usize);
        let mut color = vec![0u8; w * h * 4];
        for px in color.chunks_mut(4) {
            px.copy_from_slice(&[24, 28, 40, 255]);
        }
        let mut depth = vec![f32::INFINITY; w * h];

        let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
        let (sp, cp) = ang(cam.pitch).sin_cos();
        let (sy, cy) = ang(cam.yaw).sin_cos();
        let fwd = [cp * cy, cp * sy, sp];
        let right = [-sy, cy, 0.0];
        let up = [-sp * cy, -sp * sy, cp];
        let focal = (width as f32 * 0.5) / hor_plus_tan(cam.fov, width as f32 / height.max(1) as f32);
        let (cx, cy_) = (width as f32 * 0.5, height as f32 * 0.5);
        let near = 1.0f32;

        let faces = self.model.faces();
        let mut tex_flags: HashMap<i32, u32> = HashMap::new();
        for face in &faces {
            let mut surf = self.model.surfs[face.surf].clone();
            // the surface's flags with its texture's (as the walk hands them to the device)
            surf.poly_flags |= *tex_flags.entry(surf.texture).or_insert_with(|| self.texture_poly_flags(surf.texture));
            if surf.poly_flags & (PF_INVISIBLE | PF_PORTAL | PF_FAKEBACKDROP) != 0 {
                continue;
            }
            let normal = self.model.vectors.get(surf.v_normal as usize).copied().unwrap_or([0.0, 0.0, 1.0]);
            if surf.poly_flags & PF_TWOSIDED == 0 && dot(normal, sub(cam.pos, face.vertices[0])) <= 0.0 {
                continue;
            }
            let Some(tex) = self.texture(surf.texture) else { continue };
            let base = self.model.points.get(surf.p_base as usize).copied().unwrap_or([0.0; 3]);
            let tu = self.model.vectors.get(surf.v_texture_u as usize).copied().unwrap_or([1.0, 0.0, 0.0]);
            let tv = self.model.vectors.get(surf.v_texture_v as usize).copied().unwrap_or([0.0, 1.0, 0.0]);
            let masked = surf.poly_flags & PF_MASKED != 0;
            // simple directional shading so that surface orientation is readable without lightmaps
            let shade = 0.55 + 0.45 * dot(normal, [0.3, 0.5, 0.8]).abs() / (dot(normal, normal).sqrt().max(1e-6) * 0.99);

            // camera-space polygon, clipped against the near plane
            struct V {
                cam: [f32; 3],
                u: f32,
                v: f32,
            }
            let to_v = |p: [f32; 3]| {
                let d = sub(p, cam.pos);
                let rel = sub(p, base);
                V {
                    cam: [dot(d, right), dot(d, up), dot(d, fwd)],
                    u: dot(rel, tu) + surf.pan_u as f32,
                    v: dot(rel, tv) + surf.pan_v as f32,
                }
            };
            let poly: Vec<V> = face.vertices.iter().map(|&p| to_v(p)).collect();
            let mut clipped: Vec<V> = Vec::new();
            for i in 0..poly.len() {
                let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
                let (ain, bin) = (a.cam[2] >= near, b.cam[2] >= near);
                if ain {
                    clipped.push(V { cam: a.cam, u: a.u, v: a.v });
                }
                if ain != bin {
                    let t = (near - a.cam[2]) / (b.cam[2] - a.cam[2]);
                    let l = |x: f32, y: f32| x + (y - x) * t;
                    clipped.push(V {
                        cam: [l(a.cam[0], b.cam[0]), l(a.cam[1], b.cam[1]), near],
                        u: l(a.u, b.u),
                        v: l(a.v, b.v),
                    });
                }
            }
            if clipped.len() < 3 {
                continue;
            }
            // screen space: x right, y down; keep 1/w, u/w, v/w for perspective-correct interpolation
            let scr: Vec<[f32; 5]> = clipped
                .iter()
                .map(|p| {
                    let iw = 1.0 / p.cam[2];
                    [cx + p.cam[0] * iw * focal, cy_ - p.cam[1] * iw * focal, iw, p.u * iw, p.v * iw]
                })
                .collect();
            for k in 1..scr.len() - 1 {
                raster_triangle(&scr[0], &scr[k], &scr[k + 1], w, h, &mut |x, y, iw, u, v| {
                    let z = 1.0 / iw;
                    let idx = y * w + x;
                    if z >= depth[idx] {
                        return;
                    }
                    let (tx, ty) = ((u.floor() as i64).rem_euclid(tex.w as i64) as usize, (v.floor() as i64).rem_euclid(tex.h as i64) as usize);
                    let t = &tex.rgba[(ty * tex.w as usize + tx) * 4..][..4];
                    if masked && t[3] == 0 {
                        return;
                    }
                    depth[idx] = z;
                    for c in 0..3 {
                        color[idx * 4 + c] = (t[c] as f32 * shade).min(255.0) as u8;
                    }
                });
            }
        }
        Frame { width, height, color, depth }
    }
}

/// Fills a triangle whose vertices are `[x, y, 1/w, u/w, v/w]`; calls `put(x, y, 1/w, u, v)` per covered pixel.
pub fn raster_triangle(a: &[f32; 5], b: &[f32; 5], c: &[f32; 5], w: usize, h: usize, put: &mut dyn FnMut(usize, usize, f32, f32, f32)) {
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
    let max_x = (a[0].max(b[0]).max(c[0]).ceil().min(w as f32 - 1.0)).max(-1.0);
    let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
    let max_y = (a[1].max(b[1]).max(c[1]).ceil().min(h as f32 - 1.0)).max(-1.0);
    if max_x < 0.0 || max_y < 0.0 {
        return;
    }
    let inv = 1.0 / area;
    for y in min_y..=max_y as usize {
        for x in min_x..=max_x as usize {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) * inv;
            let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) * inv;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let iw = w0 * a[2] + w1 * b[2] + w2 * c[2];
            let u = (w0 * a[3] + w1 * b[3] + w2 * c[3]) / iw;
            let v = (w0 * a[4] + w1 * b[4] + w2 * c[4]) / iw;
            put(x, y, iw, u, v);
        }
    }
}

/// Minimal PNG writer (RGBA8, uncompressed deflate blocks) for eyeballing decoded images.
pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend((body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        out.extend(&c);
        out.extend(crc(&c).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend((b.len() as u16).to_le_bytes());
        z.extend((!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut s2) = (1u32, 0u32);
    for &b in &raw {
        a = (a + b as u32) % 65521;
        s2 = (s2 + a) % 65521;
    }
    z.extend(((s2 << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

