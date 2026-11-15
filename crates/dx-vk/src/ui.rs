//! The 2D pass: draws the draw list of the window system (textured, tinted rectangles) over the scene.
use crate::texture::{GpuTexture, Staging};
use crate::{Context, Result, DEPTH_FORMAT};
use ash::vk;
use dx_soft::MapView;
use std::collections::HashMap;

pub const UI_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.vert.spv"));
pub const UI_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.frag.spv"));

const PF_MASKED: u32 = 0x2;
const PF_TRANSLUCENT: u32 = 0x4;
const PF_MODULATED: u32 = 0x40;
const PF_NO_SMOOTH: u32 = 0x800;
const MAX_TEXTURES: u32 = 1024;

/// One rectangle to draw: where, which texels of which texture, tint and the original's poly flags.
#[derive(Clone, Debug)]
pub struct UiQuad {
    /// Texture object path (`Package.Group.Name`).
    pub texture: String,
    pub rect: [f32; 4],
    /// Texels: u, v, width, height.
    pub uv: [f32; 4],
    pub color: [f32; 3],
    pub poly_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    viewport: [f32; 4],
}

struct Tex {
    _texture: GpuTexture,
    size: (f32, f32),
    set_linear: vk::DescriptorSet,
    set_nearest: vk::DescriptorSet,
    /// A procedural texture: its key and the staging memory for its per-frame pixels.
    dynamic: Option<((String, usize), Staging)>,
}

/// The flags the shipped render device draws a tile with (`UD3DRenderDevice::DrawTile`, D3DDrv 10005ba0): bit 24 off,
/// and a modulated tile becomes modulated and masked only (translucent and not-smoothed dropped; blending then picks
/// translucent before modulated, `SetBlending` 10008270, which leaves modulated tiles as they are).
fn tile_flags(f: u32) -> u32 {
    let f = f & !0x0100_0000;
    if f & PF_MODULATED != 0 {
        PF_MODULATED | PF_MASKED
    } else {
        f
    }
}

pub struct UiRenderer {
    /// By texture path and whether the drawing is masked (palette index 0 decodes differently).
    textures: HashMap<(String, bool), Option<Tex>>,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    linear: vk::Sampler,
    nearest: vk::Sampler,
    layout: vk::PipelineLayout,
    opaque: vk::Pipeline,
    translucent: vk::Pipeline,
    modulated: vk::Pipeline,
    /// The screen flash: source ONE, destination times a blend constant (the flash's alpha), like the drivers' `EndFlash`.
    flash_pipeline: vk::Pipeline,
    white: (GpuTexture, vk::DescriptorSet),
    /// Set before drawing: the engine's flash for this frame (`FlashFog` colour, `FlashScale` already halved and
    /// clamped like the engine passes it), or `None`.
    pub flash: Option<([f32; 3], [f32; 3])>,
    pub drawn: usize,
}

impl UiRenderer {
    pub fn new(cx: &Context) -> Result<UiRenderer> {
        unsafe {
            let sampler = |filter: vk::Filter| {
                cx.device.create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(filter)
                        .min_filter(filter)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT)
                        .max_lod(0.0),
                    None,
                )
            };
            let (linear, nearest) = (sampler(vk::Filter::LINEAR)?, sampler(vk::Filter::NEAREST)?);
            let b = [vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&b), None)?;
            let sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(MAX_TEXTURES * 2 + 1)];
            let pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(MAX_TEXTURES * 2 + 1).pool_sizes(&sizes), None)?;
            let push = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT).size(std::mem::size_of::<Push>() as u32)];
            let layouts = [set_layout];
            let layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts).push_constant_ranges(&push), None)?;
            let vs = cx.shader_module(UI_VERT)?;
            let fs = cx.shader_module(UI_FRAG)?;
            let make = |blend: Option<(vk::BlendFactor, vk::BlendFactor)>, blend_constants: bool| -> Result<vk::Pipeline> {
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main"),
                ];
                let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
                let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
                let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples());
                let ds = vk::PipelineDepthStencilStateCreateInfo::default();
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
                let mut dyn_states = crate::SCENE_DYNAMIC_STATES.to_vec();
                if blend_constants {
                    dyn_states.push(vk::DynamicState::BLEND_CONSTANTS);
                }
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
            let opaque = make(None, false)?;
            let translucent = make(Some((vk::BlendFactor::ONE, vk::BlendFactor::ONE_MINUS_SRC_COLOR)), false)?;
            let modulated = make(Some((vk::BlendFactor::DST_COLOR, vk::BlendFactor::SRC_COLOR)), false)?;
            let flash_pipeline = make(Some((vk::BlendFactor::ONE, vk::BlendFactor::CONSTANT_COLOR)), true)?;
            cx.device.destroy_shader_module(vs, None);
            cx.device.destroy_shader_module(fs, None);
            let white_tex = GpuTexture::new(cx, &[(1, 1, vec![255, 255, 255, 255])])?;
            let layouts = [set_layout];
            let white_set = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))?[0];
            let info = [vk::DescriptorImageInfo::default().sampler(nearest).image_view(white_tex.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            cx.device.update_descriptor_sets(&[vk::WriteDescriptorSet::default().dst_set(white_set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info)], &[]);
            Ok(UiRenderer { textures: HashMap::new(), set_layout, pool, linear, nearest, layout, opaque, translucent, modulated, flash_pipeline, white: (white_tex, white_set), flash: None, drawn: 0 })
        }
    }

    fn load(&self, cx: &Context, view: &MapView, path: &str, masked: bool) -> Result<Option<Tex>> {
        // a procedural texture is refreshed every frame (`update_procedurals`)
        let procedural = view.procedural_key_by_path(path).and_then(|k| Some((view.procedural_initial(&k)?, k)));
        let mut dynamic = None;
        let mips = if let Some(((w, h, rgba), key)) = procedural {
            dynamic = Some((key, Staging::new(cx, w, h)?));
            vec![(w, h, rgba)]
        } else {
            let Some(mips) = view.texture_mips_by_path_masked(path, masked) else { return Ok(None) };
            mips
        };
        let first = &mips[..1];
        // the size the game measures the texture by (a mod's replacement picture may be bigger)
        let size = view.texture_size_by_path(path).filter(|_| dynamic.is_none()).map(|(w, h)| (w as f32, h as f32)).unwrap_or((first[0].0 as f32, first[0].1 as f32));
        let texture = GpuTexture::new(cx, first)?;
        unsafe {
            let layouts = [self.set_layout, self.set_layout];
            let sets = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.pool).set_layouts(&layouts))?;
            for (set, sampler) in [(sets[0], self.linear), (sets[1], self.nearest)] {
                let info = [vk::DescriptorImageInfo::default().sampler(sampler).image_view(texture.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let write = [vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info)];
                cx.device.update_descriptor_sets(&write, &[]);
            }
            Ok(Some(Tex { _texture: texture, size, set_linear: sets[0], set_nearest: sets[1], dynamic }))
        }
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

    /// Uploads the textures the quads need (call before recording the frame).
    pub fn prepare(&mut self, cx: &Context, view: &MapView, quads: &[UiQuad]) -> Result<()> {
        for q in quads {
            let key = (q.texture.clone(), tile_flags(q.poly_flags) & PF_MASKED != 0);
            if !self.textures.contains_key(&key) && self.textures.len() < MAX_TEXTURES as usize {
                let t = self.load(cx, view, &q.texture, key.1)?;
                self.textures.insert(key, t);
            }
        }
        Ok(())
    }

    /// Records the quads inside an open dynamic rendering pass of a `width` x `height` target.
    pub fn draw(&mut self, cx: &Context, cb: vk::CommandBuffer, width: u32, height: u32, quads: &[UiQuad]) {
        self.draw_clipped(cx, cb, width, height, quads, vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } });
    }

    /// `draw`, only inside `clip` (target pixels).
    pub fn draw_clipped(&mut self, cx: &Context, cb: vk::CommandBuffer, width: u32, height: u32, quads: &[UiQuad], clip: vk::Rect2D) {
        unsafe {
            let d = &cx.device;
            d.cmd_set_viewport(cb, 0, &[vk::Viewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }]);
            d.cmd_set_scissor(cb, 0, &[clip]);
            let mut bound: Option<vk::Pipeline> = None;
            for q in quads {
                let flags = tile_flags(q.poly_flags);
                let Some(Some(tex)) = self.textures.get(&(q.texture.clone(), flags & PF_MASKED != 0)) else { continue };
                let pipe = if flags & PF_MODULATED != 0 {
                    self.modulated
                } else if flags & PF_TRANSLUCENT != 0 {
                    self.translucent
                } else {
                    self.opaque
                };
                if bound != Some(pipe) {
                    d.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipe);
                    bound = Some(pipe);
                }
                let set = if flags & PF_NO_SMOOTH != 0 { tex.set_nearest } else { tex.set_linear };
                // a modulated tile is drawn white (the device sets the vertex colour to 0xFFFFFFFF)
                let color = if flags & PF_MODULATED != 0 { [1.0; 3] } else { q.color };
                d.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[set], &[]);
                let (tw, th) = tex.size;
                let push = Push {
                    rect: q.rect,
                    uv: [q.uv[0] / tw, q.uv[1] / th, (q.uv[0] + q.uv[2]) / tw, (q.uv[1] + q.uv[3]) / th],
                    color: [color[0], color[1], color[2], if flags & PF_MASKED != 0 { 1.0 } else { 0.0 }],
                    viewport: [width as f32, height as f32, 0.0, 0.0],
                };
                let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                d.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                d.cmd_draw(cb, 6, 1, 0, 0);
                self.drawn += 1;
            }
        }
    }

    /// The screen flash over the whole `width` x `height` target (the Direct3D driver's `EndFlash`): nothing when the
    /// scale is exactly 0.5 and the fog black; otherwise `screen = fog + screen * min(2 * scale.x, 1)`, both quantised to
    /// bytes the way the driver builds its vertex colour.
    pub fn draw_flash(&mut self, cx: &Context, cb: vk::CommandBuffer, width: u32, height: u32) {
        let Some((fog, scale)) = self.flash else { return };
        if scale == [0.5; 3] && fog == [0.0; 3] {
            return;
        }
        let byte = |v: f32| ((v * 256.0 - 0.5).round_ties_even()).clamp(0.0, 255.0) / 255.0;
        let alpha = byte((scale[0] * 2.0).min(1.0));
        unsafe {
            let d = &cx.device;
            d.cmd_set_viewport(cb, 0, &[vk::Viewport { x: 0.0, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }]);
            d.cmd_set_scissor(cb, 0, &[vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } }]);
            d.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.flash_pipeline);
            d.cmd_set_blend_constants(cb, &[alpha, alpha, alpha, alpha]);
            d.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.white.1], &[]);
            let push = Push {
                rect: [0.0, 0.0, width as f32, height as f32],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: [byte(fog[0]), byte(fog[1]), byte(fog[2]), 1.0],
                viewport: [width as f32, height as f32, 0.0, 0.0],
            };
            let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
            d.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            d.cmd_draw(cb, 6, 1, 0, 0);
        }
    }

    pub fn destroy(self, cx: &Context) {
        self.white.0.destroy(cx);
        for (_, t) in self.textures {
            if let Some(t) = t {
                t._texture.destroy(cx);
                if let Some((_, s)) = t.dynamic {
                    s.destroy(cx);
                }
            }
        }
        unsafe {
            cx.device.destroy_pipeline(self.opaque, None);
            cx.device.destroy_pipeline(self.translucent, None);
            cx.device.destroy_pipeline(self.modulated, None);
            cx.device.destroy_pipeline(self.flash_pipeline, None);
            cx.device.destroy_pipeline_layout(self.layout, None);
            cx.device.destroy_descriptor_pool(self.pool, None);
            cx.device.destroy_descriptor_set_layout(self.set_layout, None);
            cx.device.destroy_sampler(self.linear, None);
            cx.device.destroy_sampler(self.nearest, None);
        }
    }
}
