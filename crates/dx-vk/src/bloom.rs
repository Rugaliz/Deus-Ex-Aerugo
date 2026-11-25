//! Bloom (`Quality::bloom`, an Aerugo option over the original's look): after the main view (with the first-person
//! weapon) is drawn and before the window system draws over it, the bright parts of the picture are taken at half
//! size above a soft threshold, blurred down a chain of smaller images and back up, and the glow is added onto the
//! picture. The scene stops for it between `Offscreen::pause` and `resume` (the target keeps its samples then).

use crate::{Context, Offscreen, Result, DEPTH_FORMAT};
use ash::vk;

const BLOOM_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bloom.frag.spv"));
const N: usize = crate::FRAMES_IN_FLIGHT;
const FORMAT: vk::Format = vk::Format::B10G11R11_UFLOAT_PACK32;
/// Levels of the chain (half size down to 1/64), fewer when the picture is small.
const LEVELS: usize = 6;

/// The glow's look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloomParams {
    /// Brightness (the largest channel, 0..1) where the glow starts.
    pub threshold: f32,
    /// Width of the soft start below the threshold.
    pub knee: f32,
    /// How much of the glow is added.
    pub strength: f32,
}

impl Default for BloomParams {
    fn default() -> BloomParams {
        // `DX_BLOOM=threshold,knee,strength` for tuning
        let v: Vec<f32> = std::env::var("DX_BLOOM").ok().map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
        BloomParams { threshold: v.first().copied().unwrap_or(0.75), knee: v.get(1).copied().unwrap_or(0.25), strength: v.get(2).copied().unwrap_or(0.35) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    texel: [f32; 2],
    threshold: f32,
    knee: f32,
    strength: f32,
    mode: i32,
}

struct Image {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    width: u32,
    height: u32,
}

pub struct Bloom {
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// Each frame slot's sets: the picture, then each level (as sources).
    sets: [Vec<vk::DescriptorSet>; N],
    layout: vk::PipelineLayout,
    /// One level from another (no blending), one level added onto a bigger one, the glow added onto the picture.
    down: vk::Pipeline,
    up: vk::Pipeline,
    apply: vk::Pipeline,
    sampler: vk::Sampler,
    /// The chain for a target size.
    images: Option<(u32, u32, Vec<Image>)>,
    pub params: BloomParams,
    slot: usize,
}

impl Bloom {
    pub fn new(cx: &Context) -> Result<Bloom> {
        unsafe {
            let frag = vk::ShaderStageFlags::FRAGMENT;
            let b = [vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(1).stage_flags(frag)];
            let set_layout = cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&b), None)?;
            let per_slot = LEVELS + 1;
            let sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count((per_slot * N) as u32)];
            let pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets((per_slot * N) as u32).pool_sizes(&sizes), None)?;
            let layouts = vec![set_layout; per_slot];
            let mut sets: [Vec<vk::DescriptorSet>; N] = Default::default();
            for s in sets.iter_mut() {
                *s = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))?;
            }
            let sl = [set_layout];
            let pr = [vk::PushConstantRange::default().stage_flags(frag).size(std::mem::size_of::<Push>() as u32)];
            let layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&sl).push_constant_ranges(&pr), None)?;
            let vs = cx.shader_module(crate::present::POST_VERT)?;
            let fs = cx.shader_module(BLOOM_FRAG)?;
            let make = |add: bool, scene: bool| -> Result<vk::Pipeline> {
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main"),
                ];
                let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
                let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
                let samples = if scene { cx.samples() } else { vk::SampleCountFlags::TYPE_1 };
                let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
                let ds = vk::PipelineDepthStencilStateCreateInfo::default();
                let mut att = vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA);
                if add {
                    // added; the picture's alpha kept
                    att = att
                        .blend_enable(true)
                        .src_color_blend_factor(vk::BlendFactor::ONE)
                        .dst_color_blend_factor(vk::BlendFactor::ONE)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
                let atts = [att];
                let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&atts);
                let scene_states = crate::SCENE_DYNAMIC_STATES;
                let plain_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(if scene { &scene_states[..] } else { &plain_states[..] });
                let color_formats = [if scene { cx.color_format() } else { FORMAT }];
                let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
                if scene {
                    rendering = rendering.depth_attachment_format(DEPTH_FORMAT).stencil_attachment_format(DEPTH_FORMAT);
                }
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&input_asm)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&ms)
                    .depth_stencil_state(&ds)
                    .color_blend_state(&blend)
                    .dynamic_state(&dynamic)
                    .layout(layout)
                    .push_next(&mut rendering);
                Ok(cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0])
            };
            let (down, up, apply) = (make(false, false)?, make(true, false)?, make(true, true)?);
            cx.device.destroy_shader_module(vs, None);
            cx.device.destroy_shader_module(fs, None);
            let sampler = cx.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            Ok(Bloom { set_layout, pool, sets, layout, down, up, apply, sampler, images: None, params: BloomParams::default(), slot: 0 })
        }
    }

    fn ensure_images(&mut self, cx: &Context, width: u32, height: u32) -> Result<()> {
        if self.images.as_ref().is_some_and(|(w, h, _)| (*w, *h) == (width, height)) {
            return Ok(());
        }
        if self.images.is_some() {
            unsafe { cx.device.device_wait_idle()? };
        }
        self.destroy_images(cx);
        let mut chain = Vec::new();
        let (mut w, mut h) = (width.div_ceil(2).max(1), height.div_ceil(2).max(1));
        for _ in 0..LEVELS {
            unsafe {
                let image = cx.device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(FORMAT)
                        .extent(vk::Extent3D { width: w, height: h, depth: 1 })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )?;
                let req = cx.device.get_image_memory_requirements(image);
                let memory = cx.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(cx.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?),
                    None,
                )?;
                cx.device.bind_image_memory(image, memory, 0)?;
                let view = cx.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(FORMAT)
                        .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)),
                    None,
                )?;
                chain.push(Image { image, memory, view, width: w, height: h });
            }
            if w <= 4 || h <= 4 {
                break;
            }
            (w, h) = (w.div_ceil(2), h.div_ceil(2));
        }
        self.images = Some((width, height, chain));
        Ok(())
    }

    fn destroy_images(&mut self, cx: &Context) {
        if let Some((_, _, chain)) = self.images.take() {
            for i in chain {
                unsafe {
                    cx.device.destroy_image_view(i.view, None);
                    cx.device.destroy_image(i.image, None);
                    cx.device.free_memory(i.memory, None);
                }
            }
        }
    }

    /// The glow of the picture in `target` (outside the scene's rendering: between `Offscreen::pause` and `resume`).
    pub fn run(&mut self, cx: &Context, cb: vk::CommandBuffer, target: &Offscreen) -> Result<()> {
        self.slot = cx.frame_slot();
        self.ensure_images(cx, target.width, target.height)?;
        let (_, _, chain) = self.images.as_ref().unwrap();
        let sets = &self.sets[self.slot];
        let color_range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        unsafe {
            // the sets: the picture, then each level
            let mut infos = vec![[vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(target.color_view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]];
            infos.extend(chain.iter().map(|i| [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(i.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]));
            let writes: Vec<vk::WriteDescriptorSet> = infos.iter().zip(sets).map(|(info, &set)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(info)).collect();
            cx.device.update_descriptor_sets(&writes, &[]);

            let barrier = |image: vk::Image, old, new, src_stage, src_access, dst_stage, dst_access| {
                vk::ImageMemoryBarrier2::default().image(image).old_layout(old).new_layout(new).src_stage_mask(src_stage).src_access_mask(src_access).dst_stage_mask(dst_stage).dst_access_mask(dst_access).subresource_range(color_range)
            };
            let (attach, frag) = (vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, vk::PipelineStageFlags2::FRAGMENT_SHADER);
            let (write, read, sampled) = (vk::AccessFlags2::COLOR_ATTACHMENT_WRITE, vk::AccessFlags2::COLOR_ATTACHMENT_READ, vk::AccessFlags2::SHADER_SAMPLED_READ);
            // the picture becomes a source
            let to_read = [barrier(target.color, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, attach, write, frag, sampled)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_read));

            let p = self.params;
            let pass = |dst: &Image, set: vk::DescriptorSet, texel: [f32; 2], mode: i32, load: bool| {
                let old = if load { vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL } else { vk::ImageLayout::UNDEFINED };
                let to_attach = [barrier(dst.image, old, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, frag, sampled, attach, write | read)];
                cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_attach));
                let area = vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: dst.width, height: dst.height } };
                let att = [vk::RenderingAttachmentInfo::default()
                    .image_view(dst.view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(if load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::DONT_CARE })
                    .store_op(vk::AttachmentStoreOp::STORE)];
                cx.device.cmd_begin_rendering(cb, &vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&att));
                cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: 0.0, y: 0.0, width: dst.width as f32, height: dst.height as f32, min_depth: 0.0, max_depth: 1.0 }]);
                cx.device.cmd_set_scissor(cb, 0, &[area]);
                cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, if load { self.up } else { self.down });
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[set], &[]);
                let push = Push { texel, threshold: p.threshold, knee: p.knee.max(1e-3), strength: p.strength, mode };
                cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::FRAGMENT, 0, std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>()));
                cx.device.cmd_draw(cb, 3, 1, 0, 0);
                cx.device.cmd_end_rendering(cb);
                let done = [barrier(dst.image, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, attach, write, frag, sampled)];
                cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&done));
            };
            // down: the picture's bright part, then each level from the one before
            pass(&chain[0], sets[0], [1.0 / target.width as f32, 1.0 / target.height as f32], 0, false);
            for k in 1..chain.len() {
                let src = &chain[k - 1];
                pass(&chain[k], sets[k], [1.0 / src.width as f32, 1.0 / src.height as f32], 1, false);
            }
            // up: each level added onto the next bigger one
            for k in (1..chain.len()).rev() {
                let src = &chain[k];
                pass(&chain[k - 1], sets[k + 1], [1.0 / src.width as f32, 1.0 / src.height as f32], 2, true);
            }
            // the picture is drawn into again
            let back = [barrier(target.color, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, frag, sampled, attach, write | read)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&back));
        }
        Ok(())
    }

    /// Adds the glow onto the picture (inside the scene's rendering, after `Offscreen::resume`; viewport and scissor
    /// set by the caller).
    pub fn apply(&self, cx: &Context, cb: vk::CommandBuffer) {
        let Some((w, h, _)) = self.images.as_ref() else { return };
        let push = Push { texel: [1.0 / *w as f32, 1.0 / *h as f32], threshold: 0.0, knee: 1.0, strength: self.params.strength, mode: 3 };
        unsafe {
            crate::set_stencil(cx, cb, crate::Stencil::Off);
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.apply);
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.sets[self.slot][1]], &[]);
            cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::FRAGMENT, 0, std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>()));
            cx.device.cmd_draw(cb, 3, 1, 0, 0);
        }
    }

    pub fn destroy(mut self, cx: &Context) {
        self.destroy_images(cx);
        unsafe {
            for p in [self.down, self.up, self.apply] {
                cx.device.destroy_pipeline(p, None);
            }
            cx.device.destroy_pipeline_layout(self.layout, None);
            cx.device.destroy_descriptor_pool(self.pool, None);
            cx.device.destroy_descriptor_set_layout(self.set_layout, None);
            cx.device.destroy_sampler(self.sampler, None);
        }
    }
}
