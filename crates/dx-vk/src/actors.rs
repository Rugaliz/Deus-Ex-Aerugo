//! Actors with Vulkan. Meshes go through `URender::DrawLodMesh` on the CPU (`crate::lodmesh`: level of detail, normals,
//! facing, `Fatness`, vertex light), its triangles land in a per-frame vertex buffer and are drawn in face order as
//! D3DDrv draws them (`DrawGouraudPolygon`: texture times vertex colour; `SetBlending`'s blend per face). Sprites and
//! decals are drawn from push constants and the per-frame slot buffer.
use crate::lodmesh::{self, MeshActor, MeshLights, MeshView, PF_ENVIRONMENT, PF_MASKED, PF_MODULATED, PF_NO_SMOOTH, PF_TRANSLUCENT};
use crate::texture::{GpuTexture, Staging};
use crate::{Buffer, Context, Result, DEPTH_FORMAT};
use ash::vk;
use dx_soft::mesh::MeshData;
use dx_soft::{ActorDraw, Camera, MapView};
use std::collections::HashMap;
use std::rc::Rc;

const LODMESH_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lodmesh.vert.spv"));
const LODMESH_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lodmesh.frag.spv"));
const DETAIL_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/detail.frag.spv"));
const SPRITE_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprite.vert.spv"));
const DECAL_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/decal.vert.spv"));
const DECAL_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/decal.frag.spv"));
const SPRITE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sprite.frag.spv"));

/// One corner of a mesh triangle as the device gets it: world position, texture coordinates, colour.
#[repr(C)]
#[derive(Clone, Copy)]
struct LodVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 3],
    /// D3D's specular colour: the vertex fog, added after texturing.
    fog: [f32; 3],
    /// The unit normal, and the first vec4 of the actor's light list in the slot buffer when this corner is lit per
    /// pixel (`Quality::pixel_lighting`), else `NO_LIGHTS` (the vertex colour as it is).
    normal: [f32; 3],
    light: u32,
}

/// `LodVertex::light` of a corner drawn with its own colour.
const NO_LIGHTS: u32 = u32::MAX;

/// Copies `v` into the per-frame slot buffer at `cursor` (in slots); returns its first vec4's index, `None` when the
/// frame's slots are used up.
fn push_slots(cursor: &mut usize, ptr: *mut [f32; 4], v: &[[f32; 4]]) -> Option<u32> {
    let slots = v.len().div_ceil(SLOT_VEC4S);
    if *cursor + slots > MAX_SLOTS {
        return None;
    }
    let first = *cursor * SLOT_VEC4S;
    *cursor += slots;
    // SAFETY: the mapped slot buffer holds MAX_SLOTS * SLOT_VEC4S vec4s; this range is inside it
    unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), ptr.add(first), v.len()) };
    Some(first as u32)
}

/// One layer of a surface's detail texture as D3DDrv draws it (`DrawComplexSurface`'s detail pass): a triangle fan of
/// world positions, detail texture coordinates (in units of the texture) and vertex alphas (0..1), and the texture.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailPoly {
    /// The BSP node whose polygon this is (the third list draws a node's layers right after it).
    pub node: u32,
    pub texture: String,
    pub points: Vec<([f32; 3], [f32; 2], f32)>,
}

/// Push constants of a run of mesh triangles: view-projection, (alpha test, unused...).
#[repr(C)]
#[derive(Clone, Copy)]
struct LodPush {
    vp: [f32; 16],
    params: [u32; 4],
    /// A mirror frame's plane (its `NearClip` in world terms): mesh points on the reflected camera's side are not drawn.
    clip: [f32; 4],
}

/// Mesh triangle corners one frame can hold (the buffer is rewritten every frame; one frame is in flight).
const MAX_LOD_VERTICES: usize = 1 << 21;

/// Float4s per slot of the per-frame slot buffer (decal pieces fill consecutive slots).
const SLOT_VEC4S: usize = 54;
const MAX_SLOTS: usize = 2048;

/// Push constants of a decal piece: view-projection, (first slot vec4 as bits, unused, texture width, height).
#[repr(C)]
#[derive(Clone, Copy)]
struct DecalPush {
    vp: [f32; 16],
    params: [f32; 4],
}

/// Push constants of a sprite: view-projection, centre, half-width and half-height axes, (glow, kind, alpha test).
#[repr(C)]
#[derive(Clone, Copy)]
struct SpritePush {
    vp: [f32; 16],
    center: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    params: [f32; 4],
}

/// How the device blends a face (`SetBlending`: translucent before modulated).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Opaque,
    Translucent,
    Modulated,
}

/// A mesh as `DrawLodMesh` reads it.
struct CpuMesh {
    data: Rc<MeshData>,
}

struct Tex {
    _texture: GpuTexture,
    set: vk::DescriptorSet,
    /// The same texture point-sampled (`PF_NoSmooth`: D3D's `POINT` filters).
    set_nearest: vk::DescriptorSet,
    /// Size of the first mip in pixels (sprites are as big as their texture times `DrawScale`).
    size: (u32, u32),
    /// Blend the texture itself asks for (see `MapView::texture_style_by_path`).
    style: u8,
    /// The texture's own `PF_Masked` (what `DrawLodMesh` adds to a face's flags).
    masked: bool,
    /// A procedural texture: its key and the staging memory for its per-frame pixels.
    dynamic: Option<((String, usize), Staging)>,
}

/// Textures by object path and how they are uploaded: `TEX_PLAIN`, `TEX_MASKED` (palette index 0 transparent for a
/// masked draw) or `TEX_DETAIL` (palette index 0 white, the detail pass's upload).
type TexKey = (String, u8);
const TEX_PLAIN: u8 = 0;
const TEX_MASKED: u8 = 1;
const TEX_DETAIL: u8 = 2;

pub struct ActorRenderer {
    meshes: HashMap<String, Option<CpuMesh>>,
    textures: HashMap<TexKey, Option<Tex>>,
    tex_set_layout: vk::DescriptorSetLayout,
    tex_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    sampler_nearest: vk::Sampler,
    lod_layout: vk::PipelineLayout,
    lod_opaque: vk::Pipeline,
    lod_translucent: vk::Pipeline,
    lod_modulated: vk::Pipeline,
    /// The detail texture pass (modulated, depth tested less-or-equal, biased toward the eye; D3D `ZBIAS` 15).
    detail_pipeline: vk::Pipeline,
    lod_buf: Buffer,
    lod_ptr: *mut LodVertex,
    lod_cursor: usize,
    sprite_layout: vk::PipelineLayout,
    sprite_opaque: vk::Pipeline,
    sprite_translucent: vk::Pipeline,
    sprite_modulated: vk::Pipeline,
    decal_layout: vk::PipelineLayout,
    decal_pipeline: vk::Pipeline,
    light_set_layout: vk::DescriptorSetLayout,
    light_pool: vk::DescriptorPool,
    light_buf: Buffer,
    light_set: vk::DescriptorSet,
    light_ptr: *mut [f32; 4],
    light_cursor: usize,
    /// The main frame's detail texture layers (D3D's detail pass of the solid surfaces), set by the session per frame.
    pub details: Vec<DetailPoly>,
    /// The layers of its translucent and modulated third-list surfaces, by node (drawn right after each).
    pub details_blended: std::collections::HashMap<u32, Vec<DetailPoly>>,
    /// The frame's width in pixels (`FX`, the level of detail's screen factor); set before drawing into a frame.
    pub frame_x: f32,
    /// The plane of the mirror frame being drawn (all zero outside mirror frames): `DrawLodMesh` clips mesh polygons at
    /// the frame's `NearClip` (Render 10b0bab0), so meshes are drawn only beyond it. Sprites are not clipped.
    pub clip: [f32; 4],
    /// Actors drawn by the last `draw`.
    pub drawn: usize,
}

const MAX_TEXTURES: u32 = 8192;

/// Axes of a mesh-space frame in the world for a rotator (units of 1/65536 turn): `v >> rot` is `v.x*x + v.y*y + v.z*z`.
/// As the engine turns things: `GMath.UnitCoords *= Rotation` (Core's yaw, pitch, roll frames with the sine table).
pub fn rotation_axes(r: [i32; 3]) -> [[f32; 3]; 3] {
    let mut c = ue_bsp::Coords::UNIT;
    c.mul_rotator(r, ue_assets::light::sin_tab);
    std::array::from_fn(|i| [c.x[i], c.y[i], c.z[i]])
}

/// `ULodMesh::GetFrame`'s coordinates (Engine.dll 10355360) as a world placement: the view's coordinates (here the
/// world's) with `Location + PrePivot` taken off the origin, then `*= Rotation`, `*= Mesh.RotOrigin`, `*= Mesh.Scale *
/// DrawScale` (no sheer); a frame vertex minus the mesh's `Origin` goes to the world through `transform_point`. A
/// third-person item (`frame`) starts from its weapon triangle's frame instead, its own location cancelled and its
/// rotation zeroed (`DrawActorSprite`), so only its `PrePivot`, `RotOrigin` and scale act inside the frame.
pub fn mesh_coords(d: &ActorDraw, mesh: &ue_assets::mesh::Mesh) -> ue_bsp::Coords {
    let sin = ue_assets::light::sin_tab;
    let mut c = match d.frame {
        Some([o, x, y, z]) => {
            // `transform_point(q) = o + q.x x + q.y y + q.z z`: rows of the axes' matrix, origin `-(o . x, o . y, o . z)`
            let back = ue_bsp::Coords { origin: [0.0; 3], x, y, z };
            ue_bsp::Coords { origin: back.transform_vector([-o[0], -o[1], -o[2]]), x: [x[0], y[0], z[0]], y: [x[1], y[1], z[1]], z: [x[2], y[2], z[2]] }
        }
        None => ue_bsp::Coords::UNIT,
    };
    match d.frame {
        Some(_) => c.translate(d.pre_pivot),
        None => {
            c.translate([d.location[0] + d.pre_pivot[0], d.location[1] + d.pre_pivot[1], d.location[2] + d.pre_pivot[2]]);
            c.mul_rotator(d.rotation, sin);
        }
    }
    c.mul_rotator(mesh.rot_origin, sin);
    c.mul_scale([mesh.scale[0] * d.scale, mesh.scale[1] * d.scale, mesh.scale[2] * d.scale], 0.0, 0);
    c
}

/// A mesh actor's pose in the world: the frames blended (`frame_a` toward `frame_b`), the blend channels added as
/// `weight * (frame - frame 0)`, then placed by `mesh_coords`.
pub fn posed_vertices(lod: &ue_assets::mesh::LodMesh, d: &ActorDraw) -> Vec<[f32; 3]> {
    let frames = lod.mesh.anim_frames.max(1) as usize;
    let fa = lod.frame_local(d.frame_a.min(frames - 1));
    let mut pose: Vec<[f32; 3]> = if d.blend > 0.0 {
        let fb = lod.frame_local(d.frame_b.min(frames - 1));
        fa.iter().zip(&fb).map(|(a, b)| std::array::from_fn(|k| (b[k] - a[k]) * d.blend + a[k])).collect()
    } else {
        fa
    };
    if !d.blends.is_empty() {
        let f0 = lod.frame_local(0);
        for &(f, w) in &d.blends {
            let fk = lod.frame_local(f.min(frames - 1));
            for (p, (a, b)) in pose.iter_mut().zip(fk.iter().zip(&f0)) {
                for k in 0..3 {
                    p[k] += w * (a[k] - b[k]);
                }
            }
        }
    }
    let c = mesh_coords(d, &lod.mesh);
    pose.iter().map(|&q| c.transform_point(q)).collect()
}

/// Column-major `a * b`.
pub fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut o = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

/// Model matrix (column-major) of an actor: mesh space -> world, with uniform scale.
pub fn model_matrix(d: &ActorDraw) -> [f32; 16] {
    let ax = rotation_axes(d.rotation);
    let mut m = [0.0; 16];
    for (c, a) in ax.iter().enumerate() {
        for r in 0..3 {
            m[c * 4 + r] = a[r] * d.scale;
        }
    }
    m[12..15].copy_from_slice(&d.location);
    m[15] = 1.0;
    m
}

impl ActorRenderer {
    pub fn new(cx: &Context) -> Result<ActorRenderer> {
        unsafe {
            let sampler = cx.device.create_sampler(
                &cx.texture_sampler_info(false),
                None,
            )?;
            let sampler_nearest = cx.device.create_sampler(
                &cx.texture_sampler_info(true),
                None,
            )?;
            let tex_b = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let tex_set_layout = cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&tex_b), None)?;
            let tex_sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(MAX_TEXTURES)];
            let tex_pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(MAX_TEXTURES).pool_sizes(&tex_sizes), None)?;
            // the mesh triangles of a frame (rewritten every frame; one frame is in flight)
            let lod_buf = cx.create_buffer(
                (MAX_LOD_VERTICES * std::mem::size_of::<LodVertex>()) as u64,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            let lod_ptr = cx.device.map_memory(lod_buf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut LodVertex;

            // per-draw lighting (rewritten every frame; one frame is in flight)
            let light_b = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
            let light_set_layout = cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&light_b), None)?;
            let light_sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1)];
            let light_pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&light_sizes), None)?;
            let light_buf = cx.create_buffer(
                (MAX_SLOTS * SLOT_VEC4S * 16) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            let light_ptr = cx.device.map_memory(light_buf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut [f32; 4];
            let light_layouts = [light_set_layout];
            let light_set = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(light_pool).set_layouts(&light_layouts))?[0];
            let light_info = [vk::DescriptorBufferInfo::default().buffer(light_buf.buffer).range(vk::WHOLE_SIZE)];
            cx.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default().dst_set(light_set).dst_binding(0).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&light_info)],
                &[],
            );
            let lod_push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(std::mem::size_of::<LodPush>() as u32)];
            // set 1: the slot buffer (light lists for per-pixel lighting)
            let lod_sets = [tex_set_layout, light_set_layout];
            let lod_layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&lod_sets).push_constant_ranges(&lod_push), None)?;
            let vs = cx.shader_module(LODMESH_VERT)?;
            let fs = cx.shader_module(LODMESH_FRAG)?;
            let sprite_push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(std::mem::size_of::<SpritePush>() as u32)];
            let sprite_set_layouts = [tex_set_layout];
            let sprite_layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&sprite_set_layouts).push_constant_ranges(&sprite_push), None)?;
            let svs = cx.shader_module(SPRITE_VERT)?;
            let sfs = cx.shader_module(SPRITE_FRAG)?;
            let q = cx.quality();
            let spec_map = crate::SCENE_SPEC;
            let (data, data_a2c) = (crate::scene_spec_data(q, false), crate::scene_spec_data(q, true));
            let spec = vk::SpecializationInfo::default().map_entries(&spec_map).data(&data);
            let spec_a2c = vk::SpecializationInfo::default().map_entries(&spec_map).data(&data_a2c);
            let make = |sprite: bool, blend: Option<(vk::BlendFactor, vk::BlendFactor)>| -> Result<vk::Pipeline> {
                let (vs, fs, layout) = if sprite { (svs, sfs, sprite_layout) } else { (vs, fs, lod_layout) };
                // the picture options as specialization constants; opaque draws with multisampling also draw masked
                // texels by alpha-to-coverage (`A2C` in the shaders)
                let a2c = blend.is_none() && q.msaa > 1;
                let frag = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main").specialization_info(if a2c { &spec_a2c } else { &spec });
                let stages = [vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"), frag];
                let bindings = [vk::VertexInputBindingDescription::default().stride(std::mem::size_of::<LodVertex>() as u32).input_rate(vk::VertexInputRate::VERTEX)];
                let attrs = [
                    vk::VertexInputAttributeDescription::default().location(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
                    vk::VertexInputAttributeDescription::default().location(1).format(vk::Format::R32G32_SFLOAT).offset(12),
                    vk::VertexInputAttributeDescription::default().location(2).format(vk::Format::R32G32B32_SFLOAT).offset(20),
                    vk::VertexInputAttributeDescription::default().location(3).format(vk::Format::R32G32B32_SFLOAT).offset(32),
                    vk::VertexInputAttributeDescription::default().location(4).format(vk::Format::R32G32B32_SFLOAT).offset(44),
                    vk::VertexInputAttributeDescription::default().location(5).format(vk::Format::R32_UINT).offset(56),
                ];
                let vertex_input = if sprite {
                    vk::PipelineVertexInputStateCreateInfo::default()
                } else {
                    vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&bindings).vertex_attribute_descriptions(&attrs)
                };
                let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
                let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples()).alpha_to_coverage_enable(a2c);
                let ds = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(blend.is_none()).depth_compare_op(vk::CompareOp::LESS);
                let mut att = vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA);
                if let Some((src, dst)) = blend {
                    att = att
                        .blend_enable(true)
                        .src_color_blend_factor(src)
                        .dst_color_blend_factor(dst)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
                let atts = [att];
                let blend_state = vk::PipelineColorBlendStateCreateInfo::default().attachments(&atts);
                let dyn_states = crate::SCENE_DYNAMIC_STATES;
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
                let color_formats = [cx.color_format()];
                let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats).depth_attachment_format(DEPTH_FORMAT).stencil_attachment_format(DEPTH_FORMAT);
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&input_asm)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&ms)
                    .depth_stencil_state(&ds)
                    .color_blend_state(&blend_state)
                    .dynamic_state(&dynamic)
                    .layout(layout)
                    .push_next(&mut rendering);
                Ok(cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0])
            };
            // `SetBlending`: opaque writes depth; translucent ONE / INVSRCCOLOR, modulated DESTCOLOR / SRCCOLOR (texture *
            // scene * 2, so mid-grey leaves it unchanged), both without depth writes
            let lod_opaque = make(false, None)?;
            let lod_translucent = make(false, Some((vk::BlendFactor::ONE, vk::BlendFactor::ONE_MINUS_SRC_COLOR)))?;
            let lod_modulated = make(false, Some((vk::BlendFactor::DST_COLOR, vk::BlendFactor::SRC_COLOR)))?;
            let detail_fs = cx.shader_module(DETAIL_FRAG)?;
            let detail_pipeline = {
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(detail_fs).name(c"main"),
                ];
                let bindings = [vk::VertexInputBindingDescription::default().stride(std::mem::size_of::<LodVertex>() as u32).input_rate(vk::VertexInputRate::VERTEX)];
                let attrs = [
                    vk::VertexInputAttributeDescription::default().location(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
                    vk::VertexInputAttributeDescription::default().location(1).format(vk::Format::R32G32_SFLOAT).offset(12),
                    vk::VertexInputAttributeDescription::default().location(2).format(vk::Format::R32G32B32_SFLOAT).offset(20),
                    vk::VertexInputAttributeDescription::default().location(3).format(vk::Format::R32G32B32_SFLOAT).offset(32),
                    vk::VertexInputAttributeDescription::default().location(4).format(vk::Format::R32G32B32_SFLOAT).offset(44),
                    vk::VertexInputAttributeDescription::default().location(5).format(vk::Format::R32_UINT).offset(56),
                ];
                let vertex_input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&bindings).vertex_attribute_descriptions(&attrs);
                let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default()
                    .polygon_mode(vk::PolygonMode::FILL)
                    .cull_mode(vk::CullModeFlags::NONE)
                    .line_width(1.0)
                    .depth_bias_enable(true)
                    .depth_bias_constant_factor(-15.0)
                    .depth_bias_slope_factor(-1.0);
                let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples());
                let ds = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(false).depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
                let att = vk::PipelineColorBlendAttachmentState::default()
                    .color_write_mask(vk::ColorComponentFlags::RGBA)
                    .blend_enable(true)
                    .src_color_blend_factor(vk::BlendFactor::DST_COLOR)
                    .dst_color_blend_factor(vk::BlendFactor::SRC_COLOR)
                    .color_blend_op(vk::BlendOp::ADD)
                    .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                    .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                    .alpha_blend_op(vk::BlendOp::ADD);
                let atts = [att];
                let blend_state = vk::PipelineColorBlendStateCreateInfo::default().attachments(&atts);
                let dyn_states = crate::SCENE_DYNAMIC_STATES;
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
                let color_formats = [cx.color_format()];
                let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats).depth_attachment_format(DEPTH_FORMAT).stencil_attachment_format(DEPTH_FORMAT);
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&input_asm)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&ms)
                    .depth_stencil_state(&ds)
                    .color_blend_state(&blend_state)
                    .dynamic_state(&dynamic)
                    .layout(lod_layout)
                    .push_next(&mut rendering);
                cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0]
            };
            cx.device.destroy_shader_module(detail_fs, None);
            let sprite_opaque = make(true, None)?;
            let sprite_translucent = make(true, Some((vk::BlendFactor::ONE, vk::BlendFactor::ONE_MINUS_SRC_COLOR)))?;
            let sprite_modulated = make(true, Some((vk::BlendFactor::DST_COLOR, vk::BlendFactor::SRC_COLOR)))?;
            // decal pieces: texture (set 0) and the slot buffer (set 1), modulated, depth tested only
            let decal_push = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT).size(std::mem::size_of::<DecalPush>() as u32)];
            let decal_sets = [tex_set_layout, light_set_layout];
            let decal_layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&decal_sets).push_constant_ranges(&decal_push), None)?;
            let dvs = cx.shader_module(DECAL_VERT)?;
            let dfs = cx.shader_module(DECAL_FRAG)?;
            let decal_pipeline = {
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(dvs).name(c"main"),
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(dfs).name(c"main"),
                ];
                let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
                let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
                let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples());
                let ds = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(false).depth_compare_op(vk::CompareOp::LESS);
                let att = vk::PipelineColorBlendAttachmentState::default()
                    .color_write_mask(vk::ColorComponentFlags::RGBA)
                    .blend_enable(true)
                    .src_color_blend_factor(vk::BlendFactor::DST_COLOR)
                    .dst_color_blend_factor(vk::BlendFactor::SRC_COLOR)
                    .color_blend_op(vk::BlendOp::ADD)
                    .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                    .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                    .alpha_blend_op(vk::BlendOp::ADD);
                let atts = [att];
                let blend_state = vk::PipelineColorBlendStateCreateInfo::default().attachments(&atts);
                let dyn_states = crate::SCENE_DYNAMIC_STATES;
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
                let color_formats = [cx.color_format()];
                let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats).depth_attachment_format(DEPTH_FORMAT).stencil_attachment_format(DEPTH_FORMAT);
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&input_asm)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&ms)
                    .depth_stencil_state(&ds)
                    .color_blend_state(&blend_state)
                    .dynamic_state(&dynamic)
                    .layout(decal_layout)
                    .push_next(&mut rendering);
                cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0]
            };
            for m in [vs, fs, svs, sfs, dvs, dfs] {
                cx.device.destroy_shader_module(m, None);
            }
            Ok(ActorRenderer {
                light_set_layout,
                light_pool,
                light_buf,
                light_set,
                light_ptr,
                light_cursor: 0,
                meshes: HashMap::new(),
                textures: HashMap::new(),
                tex_set_layout,
                tex_pool,
                sampler,
                sampler_nearest,
                lod_layout,
                lod_opaque,
                lod_translucent,
                lod_modulated,
                detail_pipeline,
                lod_buf,
                lod_ptr,
                lod_cursor: 0,
                sprite_layout,
                sprite_opaque,
                sprite_translucent,
                sprite_modulated,
                decal_layout,
                decal_pipeline,
                details: Vec::new(),
                details_blended: HashMap::new(),
                frame_x: 640.0,
                clip: [0.0; 4],
                drawn: 0,
            })
        }
    }

    fn load_texture(&self, cx: &Context, view: &MapView, path: &str, kind: u8) -> Result<Option<Tex>> {
        // a procedural texture: one level at its own size, refreshed every frame (`update_procedurals`)
        let procedural = view.procedural_key_by_path(path).and_then(|k| Some((view.procedural_initial(&k)?, k)));
        let mut dynamic = None;
        let mips = if let Some(((w, h, rgba), key)) = procedural {
            dynamic = Some((key, Staging::new(cx, w, h)?));
            vec![(w, h, rgba)]
        } else if kind == TEX_MASKED {
            // a masked draw: palette index 0 transparent whatever the texture's own flag (the device's masked upload)
            let Some(mips) = view.texture_mips_by_path_masked(path, true) else { return Ok(None) };
            mips
        } else if kind == TEX_DETAIL {
            let Some(mips) = view.detail_texture_mips(path) else { return Ok(None) };
            mips
        } else {
            let Some(mips) = view.texture_mips_by_path(path) else { return Ok(None) };
            mips
        };
        let mips: Vec<_> = mips.into_iter().take_while(|(w, h, px)| px.len() == (*w * *h * 4) as usize).collect();
        if mips.is_empty() {
            return Ok(None);
        }
        let texture = GpuTexture::new(cx, &mips)?;
        let style = view.texture_style_by_path(path);
        unsafe {
            let layouts = [self.tex_set_layout, self.tex_set_layout];
            let sets = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.tex_pool).set_layouts(&layouts))?;
            for (set, sampler) in [(sets[0], self.sampler), (sets[1], self.sampler_nearest)] {
                let info = [vk::DescriptorImageInfo::default().sampler(sampler).image_view(texture.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                cx.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info)],
                    &[],
                );
            }
            Ok(Some(Tex { _texture: texture, set: sets[0], set_nearest: sets[1], size: view.texture_size_by_path(path).unwrap_or((mips[0].0, mips[0].1)), style, masked: view.texture_masked_by_path(path), dynamic }))
        }
    }

    fn ensure_texture(&mut self, cx: &Context, view: &MapView, path: &str, kind: u8) -> Result<()> {
        let key = (path.to_string(), kind);
        if !self.textures.contains_key(&key) {
            let t = self.load_texture(cx, view, path, kind)?;
            self.textures.insert(key, t);
        }
        Ok(())
    }

    /// The texture of mesh texture slot `slot` (`GetTexture`): the replacement of every texture (`Matrix`), else the
    /// actor's skin, else the mesh's own.
    fn slot_texture<'a>(mesh: &'a CpuMesh, d: &'a ActorDraw, slot: usize) -> Option<&'a str> {
        if let Some(t) = d.all_textures.as_deref() {
            return Some(t);
        }
        d.skins.get(slot).and_then(|s| s.as_deref()).or_else(|| mesh.data.own_texture_paths.get(slot).and_then(|p| p.as_deref()))
    }

    /// The environment map of a draw (`DrawLodMesh`/`DrawMesh`): the replacement of every texture (`Matrix`), else the
    /// actor's `Texture`, zone's or level's map, else the last mesh texture there is. `None`: nothing is drawn.
    fn env_texture<'a>(mesh: &'a CpuMesh, d: &'a ActorDraw) -> Option<&'a str> {
        if let Some(t) = d.all_textures.as_deref() {
            return Some(t);
        }
        d.env_texture.as_deref().or_else(|| (0..mesh.data.lod.mesh.textures.len()).rev().find_map(|i| Self::slot_texture(mesh, d, i)))
    }

    /// The texture a face of texture slot `slot` with flags `flags` (material or triangle | draw) is drawn with: the
    /// environment map for `PF_Environment` faces and faces without a texture.
    fn face_texture<'a>(mesh: &'a CpuMesh, d: &'a ActorDraw, slot: usize, flags: u32) -> Option<&'a str> {
        let own = Self::slot_texture(mesh, d, slot);
        if flags & PF_ENVIRONMENT != 0 || own.is_none() {
            Self::env_texture(mesh, d)
        } else {
            own
        }
    }

    /// The device side of a face's flags (`SetBlending`): the texture's own `PF_Masked` added, translucency wins over
    /// modulation and drops the mask. Returns the blend and whether the face is alpha-tested (uploaded masked).
    fn face_blend(flags: u32, texture_masked: bool) -> (Kind, bool) {
        let flags = flags | if texture_masked { PF_MASKED } else { 0 };
        if flags & PF_TRANSLUCENT != 0 {
            (Kind::Translucent, false)
        } else if flags & PF_MODULATED != 0 {
            (Kind::Modulated, flags & PF_MASKED != 0)
        } else {
            (Kind::Opaque, flags & PF_MASKED != 0)
        }
    }

    /// Loads every mesh and texture the actors need. Call before recording the frame (uploads submit their own commands).
    pub fn prepare(&mut self, cx: &Context, view: &mut MapView, actors: &[ActorDraw]) -> Result<()> {
        for d in actors {
            if let Some(path) = &d.sprite {
                self.ensure_texture(cx, view, path, TEX_PLAIN)?;
                continue;
            }
            if !self.meshes.contains_key(&d.mesh) {
                let m = view.mesh_data(&d.mesh).map(|data| CpuMesh { data });
                self.meshes.insert(d.mesh.clone(), m);
            }
            let Some(Some(mesh)) = self.meshes.get(&d.mesh) else { continue };
            let mut wanted: Vec<(String, u32)> = Vec::new();
            // the faces' flags and slots: the materials of a LodMesh, the triangles of a plain mesh
            let faces: Vec<(u32, usize)> = if mesh.data.plain {
                let mut v: Vec<(u32, usize)> = mesh.data.lod.mesh.tris.iter().map(|t| (t.poly_flags, t.texture_index as usize)).collect();
                v.sort_unstable();
                v.dedup();
                v
            } else {
                mesh.data.lod.materials.iter().map(|m| (m[0], m[1] as usize)).collect()
            };
            for (f, slot) in faces {
                let flags = f | d.flags;
                if let Some(p) = Self::face_texture(mesh, d, slot, flags) {
                    wanted.push((p.to_string(), flags));
                }
            }
            for (path, flags) in wanted {
                self.ensure_texture(cx, view, &path, TEX_PLAIN)?;
                let own = self.textures.get(&(path.clone(), TEX_PLAIN)).and_then(|t| t.as_ref()).map(|t| t.masked).unwrap_or(false);
                let (_, masked) = Self::face_blend(flags, own);
                if masked {
                    self.ensure_texture(cx, view, &path, TEX_MASKED)?;
                }
            }
        }
        Ok(())
    }

    /// Records the actors inside an active rendering scope: opaque and masked first, then translucent and modulated
    /// surfaces and sprites from far to near. `prepare` must have been called with the same actors.
    pub fn draw(&mut self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, actors: &[ActorDraw]) {
        self.draw_with(cx, cb, cam, aspect, actors, None);
    }

    /// `draw` with an extra world transform before the view (a mirror's reflection).
    pub fn draw_with(&mut self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, actors: &[ActorDraw], pre: Option<[f32; 16]>) {
        let dist = |d: &ActorDraw| (0..3).map(|k| (d.location[k] - cam.pos[k]).powi(2)).sum::<f32>();
        let mut far_to_near: Vec<&ActorDraw> = actors.iter().collect();
        far_to_near.sort_by(|a, b| dist(b).total_cmp(&dist(a)));
        let steps: Vec<(u8, &ActorDraw)> = (0..2).flat_map(|pass| far_to_near.iter().map(move |d| (pass, *d))).collect();
        self.drawn = 0;
        self.draw_steps(cx, cb, cam, aspect, &steps, pre);
    }

    /// The actors one after the other in the given order, each whole (a mesh's faces in face order, whatever their
    /// blend), as `DrawFrame` draws its sprite list: the caller orders them (`DrawFrame`'s phases and the walk's sprite
    /// list). Adds to `drawn`.
    pub fn draw_in_order(&mut self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, actors: &[&ActorDraw]) {
        let steps: Vec<(u8, &ActorDraw)> = actors.iter().map(|d| (2u8, *d)).collect();
        self.draw_steps(cx, cb, cam, aspect, &steps, None);
    }

    /// Draws `(pass, actor)` steps: pass 0 the opaque parts, pass 1 the translucent and modulated ones, pass 2 all.
    fn draw_steps(&mut self, cx: &Context, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, steps: &[(u8, &ActorDraw)], pre: Option<[f32; 16]>) {
        let vp = match pre {
            Some(m) => mat_mul(&crate::world::view_proj(cam, aspect, 1.0, 65536.0), &m),
            None => crate::world::view_proj(cam, aspect, 1.0, 65536.0),
        };
        // camera axes for sprites; under a reflection the axes reflected too, so that once reflected the sprite faces
        // the screen unflipped (a mirror frame draws it screen-aligned at its projected centre)
        // (the axes through the inverse of the map's linear part; for a reflection that is the reflection itself)
        let (mut cam_fwd, mut cam_right, mut cam_up) = cam.axes();
        let mut eye = cam.pos;
        if let Some(m) = pre {
            let inv = crate::world::affine_inverse(&m);
            let lin = |v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|r| inv[r] * v[0] + inv[4 + r] * v[1] + inv[8 + r] * v[2]) };
            cam_fwd = lin(cam_fwd);
            cam_right = lin(cam_right);
            cam_up = lin(cam_up);
            eye = crate::world::affine_point(&inv, cam.pos);
        }
        // `DrawLodMesh` works in the frame's own (unreflected) terms: the eye and view axis through the inverse map
        let mesh_view = MeshView { eye, forward: cam_fwd, fov_deg: cam.fov, frame_x: self.frame_x };
        {
            for &(pass, d) in steps {
                if let Some(path) = &d.sprite {
                    let Some(Some(tex)) = self.textures.get(&(path.clone(), TEX_PLAIN)) else { continue };
                    let mut style = if d.style >= 2 { d.style } else { tex.style };
                    // decals are always drawn modulated (`DrawFrame` passes PF_Modulated alone; D3D then uses white)
                    if d.decal.is_some() {
                        style = 4;
                        if pass == 0 {
                            continue;
                        }
                        if !d.decal_polys.is_empty() {
                            let (set, size) = (tex.set, tex.size);
                            self.draw_decal_pieces(cx, cb, &vp, set, size, &d.decal_polys);
                            continue;
                        }
                    }
                    let (kind, opaque) = match style {
                        3 => (1.0, false),
                        4 => (2.0, false),
                        _ => (0.0, true),
                    };
                    if pass != 2 && (pass == 0) != opaque {
                        continue;
                    }
                    let half = [tex.size.0 as f32 * d.scale * 0.5, tex.size.1 as f32 * d.scale * 0.5];
                    // a decal lies on its surface (half-axes from `AttachDecal`'s square), a sprite faces the camera
                    let (right, up) = match d.decal {
                        Some((r, u)) => (r, u),
                        None => {
                            let (r, u) = (cam_right, cam_up);
                            ([r[0] * half[0], r[1] * half[0], r[2] * half[0]], [u[0] * half[1], u[1] * half[1], u[2] * half[1]])
                        }
                    };
                    let push = SpritePush {
                        vp,
                        center: [d.location[0], d.location[1], d.location[2], 1.0],
                        right: [right[0], right[1], right[2], 0.0],
                        up: [up[0], up[1], up[2], 0.0],
                        params: [if d.decal.is_some() { 1.0 } else { d.glow }, kind, 1.0, 0.0],
                    };
                    unsafe {
                        let pipeline = match style {
                            3 => self.sprite_translucent,
                            4 => self.sprite_modulated,
                            _ => self.sprite_opaque,
                        };
                        cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
                        cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.sprite_layout, 0, &[tex.set], &[]);
                        let bytes = std::slice::from_raw_parts(&push as *const SpritePush as *const u8, std::mem::size_of::<SpritePush>());
                        cx.device.cmd_push_constants(cb, self.sprite_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                        cx.device.cmd_draw(cb, 6, 1, 0, 0);
                    }
                    if pass != 1 {
                        self.drawn += 1;
                    }
                    continue;
                }
                if self.draw_mesh(cx, cb, &vp, &mesh_view, pass, d) && pass != 1 {
                    self.drawn += 1;
                }
            }
        }
    }

    /// One mesh actor through `DrawLodMesh`: the pose (two frames blended plus the blend channels, `GetFrame`) in world
    /// space, the triangles of its level of detail, then runs of consecutive triangles with the same texture and blend
    /// (pass 0: the opaque ones, 1: the blended ones, 2: all, in face order). Returns whether anything was drawn.
    fn draw_mesh(&mut self, cx: &Context, cb: vk::CommandBuffer, vp: &[f32; 16], view: &MeshView, pass: u8, d: &ActorDraw) -> bool {
        let Some(Some(mesh)) = self.meshes.get(&d.mesh) else { return false };
        let lod = &mesh.data.lod;
        let world = posed_vertices(lod, d);
        let actor = MeshActor {
            location: d.location,
            draw_scale: d.scale,
            lod_bias: d.lod_bias,
            fatness: d.fatness,
            flags: d.flags,
            glow: lodmesh::glow_colour(d.glow, d.ambient_glow),
        };
        let lights = d.lighting.as_ref().map(|l| MeshLights {
            ambient: l.ambient,
            scale: l.scale,
            lights: l.lights.iter().map(|&(p, c, r)| (p, c, 1.0 / r.max(1.0))).collect(),
            fog: l
                .fog_lights
                .iter()
                .map(|&(p, radius, bright, fog, b, colour)| dx_light::manager::FogLight {
                    view: [p[0] - view.eye[0], p[1] - view.eye[1], p[2] - view.eye[2]],
                    volume_radius: radius,
                    volume_brightness: bright,
                    volume_fog: fog,
                    brightness: b,
                    colour,
                })
                .collect(),
        });
        // nothing at all to draw with: nothing drawn
        if Self::env_texture(mesh, d).is_none() {
            return false;
        }
        let tris = if mesh.data.plain {
            lodmesh::build_plain(&lod.mesh, &world, view, &actor, lights.as_ref())
        } else {
            lodmesh::build(lod, &world, view, &actor, lights.as_ref())
        };
        // per-pixel lighting: the actor's light list in the slot buffer (header: ambient and scale, eye and count;
        // then position and 1/radius, colour for each light)
        let light_list = match (&lights, cx.quality().pixel_lighting) {
            (Some(l), true) => {
                let mut v: Vec<[f32; 4]> = vec![[l.ambient[0], l.ambient[1], l.ambient[2], l.scale], [view.eye[0], view.eye[1], view.eye[2], l.lights.len() as f32]];
                for &(p, c, inv_r) in &l.lights {
                    v.push([p[0], p[1], p[2], inv_r]);
                    v.push([c[0], c[1], c[2], 0.0]);
                }
                push_slots(&mut self.light_cursor, self.light_ptr, &v)
            }
            _ => None,
        };
        // each triangle's texture and blend, then runs of equal ones
        let mut runs: Vec<(vk::DescriptorSet, Kind, bool, usize, usize)> = Vec::new();
        let mut verts: Vec<LodVertex> = Vec::with_capacity(tris.len() * 3);
        for t in &tris {
            let Some(path) = Self::face_texture(mesh, d, t.slot as usize, t.flags) else { continue };
            let own = match self.textures.get(&(path.to_string(), TEX_PLAIN)) {
                Some(Some(tex)) => tex.masked,
                _ => continue,
            };
            let (kind, masked) = Self::face_blend(t.flags, own);
            if (pass == 0 && kind != Kind::Opaque) || (pass == 1 && kind == Kind::Opaque) {
                continue;
            }
            let Some(Some(tex)) = self.textures.get(&(path.to_string(), if masked { TEX_MASKED } else { TEX_PLAIN })) else { continue };
            let set = if t.flags & PF_NO_SMOOTH != 0 { tex.set_nearest } else { tex.set };
            let first = verts.len();
            for c in &t.corners {
                // modulated faces get a white diffuse colour (`DrawGouraudPolygon`)
                let color = if kind == Kind::Modulated { [1.0; 3] } else { c.color };
                let light = match light_list {
                    Some(at) if c.lit && kind != Kind::Modulated => at,
                    _ => NO_LIGHTS,
                };
                verts.push(LodVertex { pos: c.pos, uv: c.uv, color, fog: c.fog, normal: c.normal, light });
            }
            match runs.last_mut() {
                Some(r) if r.0 == set && r.1 == kind && r.2 == masked => r.4 += 3,
                _ => runs.push((set, kind, masked, first, 3)),
            }
        }
        if verts.is_empty() || self.lod_cursor + verts.len() > MAX_LOD_VERTICES {
            return false;
        }
        let base = self.lod_cursor;
        self.lod_cursor += verts.len();
        unsafe {
            std::ptr::copy_nonoverlapping(verts.as_ptr(), self.lod_ptr.add(base), verts.len());
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.lod_buf.buffer], &[0]);
            for (set, kind, masked, first, count) in runs {
                let pipeline = match kind {
                    Kind::Opaque => self.lod_opaque,
                    Kind::Translucent => self.lod_translucent,
                    Kind::Modulated => self.lod_modulated,
                };
                cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.lod_layout, 0, &[set, self.light_set], &[]);
                let push = LodPush { vp: *vp, params: [masked as u32, 0, 0, 0], clip: self.clip };
                let bytes = std::slice::from_raw_parts(&push as *const LodPush as *const u8, std::mem::size_of::<LodPush>());
                cx.device.cmd_push_constants(cb, self.lod_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                cx.device.cmd_draw(cb, count as u32, 1, (base + first) as u32, 0);
            }
        }
        true
    }

    /// Loads the detail textures of a frame's detail polygons (call with `prepare`).
    pub fn prepare_details(&mut self, cx: &Context, view: &MapView) -> Result<()> {
        let paths: Vec<String> = self.details.iter().chain(self.details_blended.values().flatten()).map(|p| p.texture.clone()).collect();
        for p in paths {
            self.ensure_texture(cx, view, &p, TEX_DETAIL)?;
        }
        Ok(())
    }

    /// Loads the detail textures of another frame's detail polygons (a child frame's).
    pub fn prepare_detail_list(&mut self, cx: &Context, view: &MapView, polys: &[DetailPoly]) -> Result<()> {
        for p in polys {
            self.ensure_texture(cx, view, &p.texture, TEX_DETAIL)?;
        }
        Ok(())
    }

    /// The detail texture pass of the surfaces drawn (`DetailPoly` fans), through the view-projection `vp`.
    pub fn draw_details(&mut self, cx: &Context, cb: vk::CommandBuffer, vp: &[f32; 16], polys: &[DetailPoly]) {
        let mut runs: Vec<(vk::DescriptorSet, usize, usize)> = Vec::new();
        let mut verts: Vec<LodVertex> = Vec::new();
        for p in polys {
            let Some(Some(tex)) = self.textures.get(&(p.texture.clone(), TEX_DETAIL)) else { continue };
            let first = verts.len();
            // D3D's triangle fan
            for k in 1..p.points.len().saturating_sub(1) {
                for &(pos, uv, a) in [&p.points[0], &p.points[k], &p.points[k + 1]] {
                    verts.push(LodVertex { pos, uv, color: [a, a, a], fog: [0.0; 3], normal: [0.0; 3], light: NO_LIGHTS });
                }
            }
            let n = verts.len() - first;
            if n == 0 {
                continue;
            }
            match runs.last_mut() {
                Some(r) if r.0 == tex.set => r.2 += n,
                _ => runs.push((tex.set, first, n)),
            }
        }
        if verts.is_empty() || self.lod_cursor + verts.len() > MAX_LOD_VERTICES {
            return;
        }
        let base = self.lod_cursor;
        self.lod_cursor += verts.len();
        unsafe {
            std::ptr::copy_nonoverlapping(verts.as_ptr(), self.lod_ptr.add(base), verts.len());
            cx.device.cmd_bind_vertex_buffers(cb, 0, &[self.lod_buf.buffer], &[0]);
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.detail_pipeline);
            let push = LodPush { vp: *vp, params: [0; 4], clip: [0.0; 4] };
            let bytes = std::slice::from_raw_parts(&push as *const LodPush as *const u8, std::mem::size_of::<LodPush>());
            cx.device.cmd_push_constants(cb, self.lod_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            for (set, first, count) in runs {
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.lod_layout, 0, &[set, self.light_set], &[]);
                cx.device.cmd_draw(cb, count as u32, 1, (base + first) as u32, 0);
            }
        }
    }

    /// Starts a new frame of lighting slots (call once per frame, before the first `draw`).
    pub fn begin_frame(&mut self) {
        self.light_cursor = 0;
        self.lod_cursor = 0;
    }

    /// Writes one actor's lighting into the next slot: model matrix, eye, ambient and scale, lights (position and
    /// inverse radius, colour). `None` when the slots of this frame are used up.
    /// The pieces of a decal (`ClipDecal`'s polygons, fanned into triangles) through the decal pipeline: their vertices
    /// go into consecutive slots of the per-frame slot buffer.
    fn draw_decal_pieces(&mut self, cx: &Context, cb: vk::CommandBuffer, vp: &[f32; 16], set: vk::DescriptorSet, size: (u32, u32), polys: &[Vec<([f32; 3], [f32; 2])>]) {
        let mut verts: Vec<[f32; 4]> = Vec::new();
        for poly in polys {
            for k in 1..poly.len().saturating_sub(1) {
                for &(p, t) in [&poly[0], &poly[k], &poly[k + 1]] {
                    verts.push([p[0], p[1], p[2], 1.0]);
                    verts.push([t[0], t[1], 0.0, 0.0]);
                }
            }
        }
        if verts.is_empty() {
            return;
        }
        let slots = verts.len().div_ceil(SLOT_VEC4S);
        if self.light_cursor + slots > MAX_SLOTS {
            return;
        }
        let first = self.light_cursor * SLOT_VEC4S;
        self.light_cursor += slots;
        unsafe {
            std::ptr::copy_nonoverlapping(verts.as_ptr(), self.light_ptr.add(first), verts.len());
            let push = DecalPush { vp: *vp, params: [f32::from_bits(first as u32), 0.0, size.0.max(1) as f32, size.1.max(1) as f32] };
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.decal_pipeline);
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.decal_layout, 0, &[set, self.light_set], &[]);
            let bytes = std::slice::from_raw_parts(&push as *const DecalPush as *const u8, std::mem::size_of::<DecalPush>());
            cx.device.cmd_push_constants(cb, self.decal_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            cx.device.cmd_draw(cb, (verts.len() / 2) as u32, 1, 0, 0);
        }
        self.drawn += 1;
    }

    /// Steps the procedural textures in use for a frame drawn at `time` and stages their pixels (after the previous
    /// frame finished on the GPU); `record_procedural_uploads` then copies them in.
    pub fn update_procedurals(&mut self, view: &mut MapView, time: f64) {
        for t in self.textures.values_mut().flatten() {
            if let Some((key, staging)) = &mut t.dynamic {
                if let Some((_, _, rgba)) = view.procedural_frame(key, time) {
                    staging.write(rgba);
                }
            }
        }
    }

    pub fn record_procedural_uploads(&mut self, cx: &Context, cb: vk::CommandBuffer) {
        for t in self.textures.values_mut().flatten() {
            if let Some((_, staging)) = &mut t.dynamic {
                staging.record_upload(cx, cb, &t._texture);
            }
        }
    }

    pub fn destroy(self, cx: &Context) {
        unsafe {
            let _ = cx.device.device_wait_idle();
            cx.device.destroy_pipeline_layout(self.decal_layout, None);
            for p in [self.lod_opaque, self.lod_translucent, self.lod_modulated, self.detail_pipeline, self.sprite_opaque, self.sprite_translucent, self.sprite_modulated, self.decal_pipeline] {
                cx.device.destroy_pipeline(p, None);
            }
            cx.device.destroy_pipeline_layout(self.lod_layout, None);
            cx.device.destroy_pipeline_layout(self.sprite_layout, None);
            cx.device.destroy_descriptor_pool(self.tex_pool, None);
            cx.device.destroy_descriptor_set_layout(self.tex_set_layout, None);
            cx.device.destroy_descriptor_pool(self.light_pool, None);
            cx.device.destroy_descriptor_set_layout(self.light_set_layout, None);
            cx.device.unmap_memory(self.light_buf.memory);
            cx.device.unmap_memory(self.lod_buf.memory);
            cx.device.destroy_sampler(self.sampler, None);
            cx.device.destroy_sampler(self.sampler_nearest, None);
        }
        cx.destroy_buffer(self.light_buf);
        cx.destroy_buffer(self.lod_buf);
        for (_, t) in self.textures {
            if let Some(t) = t {
                t._texture.destroy(cx);
                if let Some((_, s)) = t.dynamic {
                    s.destroy(cx);
                }
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// D3DDrv `SetBlending` (10008270): `PF_Translucent` wins over `PF_Modulated` and clears `PF_Masked`; the texture's
    /// own mask is added to the face's flags; opaque and modulated faces keep the alpha test.
    /// `GetFrame`'s placement: a vertex `q` (frame position minus `Origin`) lands at `Location + PrePivot +
    /// R(Rotation) R(RotOrigin) (Scale * DrawScale * q)`, checked against the rotators' closed form (the sine table is
    /// within 2e-4 of it); `rotation_axes` gives the same turn. A third-person item's frame puts `q` at `o + q.x X +
    /// q.y Y + q.z Z` with its `PrePivot` and `RotOrigin` inside the frame and its own location and rotation ignored.
    #[test]
    fn mesh_coords_place_as_get_frame() {
        let closed = |r: [i32; 3]| -> [[f32; 3]; 3] {
            let ang = |u: i32| (u & 0xFFFF) as f32 * std::f32::consts::TAU / 65536.0;
            let (sp, cp) = ang(r[0]).sin_cos();
            let (sy, cy) = ang(r[1]).sin_cos();
            let (sr, cr) = ang(r[2]).sin_cos();
            [[cp * cy, cp * sy, sp], [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp], [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp]]
        };
        let apply = |ax: [[f32; 3]; 3], v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|k| v[0] * ax[0][k] + v[1] * ax[1][k] + v[2] * ax[2][k]) };
        let mesh = ue_assets::mesh::Mesh { scale: [0.5, 0.25, 2.0], rot_origin: [0, 16384, 0], ..Default::default() };
        let rot = [3000, -12000, 700];
        let d = ActorDraw { location: [100.0, 200.0, 300.0], pre_pivot: [0.0, 0.0, -20.0], rotation: rot, scale: 1.5, ..Default::default() };
        let q = [10.0, -4.0, 8.0];
        let got = mesh_coords(&d, &mesh).transform_point(q);
        let s: [f32; 3] = std::array::from_fn(|k| q[k] * mesh.scale[k] * 1.5);
        // RotOrigin (yaw a quarter turn) first, then the actor's rotation, then Location + PrePivot
        let w = apply(closed(rot), apply(closed([0, 16384, 0]), s));
        let want = [100.0 + w[0], 200.0 + w[1], 300.0 - 20.0 + w[2]];
        for k in 0..3 {
            assert!((got[k] - want[k]).abs() < 0.01, "{got:?} {want:?}");
        }
        let ra = rotation_axes(rot);
        let ca = closed(rot);
        for i in 0..3 {
            for k in 0..3 {
                assert!((ra[i][k] - ca[i][k]).abs() < 2e-4);
            }
        }
        // the weapon frame: X, Y, Z orthonormal, origin o; the item's location and rotation do not matter
        let (o, x, y, z) = ([5.0, 6.0, 7.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let item = ActorDraw { location: [999.0; 3], rotation: [1234, 5678, 91], pre_pivot: [1.0, 2.0, 3.0], scale: 1.0, frame: Some([o, x, y, z]), ..Default::default() };
        let plain = ue_assets::mesh::Mesh { scale: [1.0; 3], ..Default::default() };
        let got = mesh_coords(&item, &plain).transform_point([10.0, 0.0, 0.0]);
        // local (10 + 1, 2, 3) -> o + 11 x + 2 y + 3 z
        let want = [5.0 - 2.0, 6.0 + 11.0, 7.0 + 3.0];
        for k in 0..3 {
            assert!((got[k] - want[k]).abs() < 1e-4, "{got:?} {want:?}");
        }
    }

    #[test]
    fn device_blend_of_a_face() {
        assert_eq!(ActorRenderer::face_blend(0x4000, false), (Kind::Opaque, false));
        assert_eq!(ActorRenderer::face_blend(0x4000, true), (Kind::Opaque, true));
        assert_eq!(ActorRenderer::face_blend(0x4002, false), (Kind::Opaque, true));
        assert_eq!(ActorRenderer::face_blend(0x4006, true), (Kind::Translucent, false));
        assert_eq!(ActorRenderer::face_blend(0x4044, false), (Kind::Translucent, false));
        assert_eq!(ActorRenderer::face_blend(0x4040, false), (Kind::Modulated, false));
        assert_eq!(ActorRenderer::face_blend(0x4040, true), (Kind::Modulated, true));
    }
}
