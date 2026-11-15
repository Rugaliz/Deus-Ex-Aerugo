//! The ray-traced screen passes (`Quality::rt_shadows`, `Quality::rt_ao`), after the main view's world and objects are
//! drawn: from the depth buffer, each visible point is traced once, at half the view's size (`rt_trace.frag`: the
//! shadows of the casters, ambient occlusion), the result blurred along the surfaces (`rt_blur.frag`) and multiplied
//! into the picture at full size (`rt_apply.frag`), mirror areas left out by the stencil.
//!
//! A caster is an object drawn this frame with one of the (at most four) lights that light it most; its shadow is as
//! dark as that light's share of the object's light.

use crate::{Buffer, Context, Offscreen, Result, DEPTH_FORMAT};
use ash::vk;

const TRACE_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rt_trace.frag.spv"));
const BLUR_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rt_blur.frag.spv"));
const APPLY_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rt_apply.frag.spv"));
const DIST_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rt_dist.frag.spv"));
/// Each pixel's distance from the eye (full size).
const DIST_FORMAT: vk::Format = vk::Format::R32_SFLOAT;
const N: usize = crate::FRAMES_IN_FLIGHT;
/// The traced factor and the distance from the eye.
const HALF_FORMAT: vk::Format = vk::Format::R16G16_SFLOAT;

/// One caster as `rt_trace.frag` reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Caster {
    /// The light's position and size (the disc the rays spread over).
    pub light: [f32; 4],
    /// The object's middle and radius.
    pub centre: [f32; 4],
    /// x: how dark its shadow is (0..1), y: how far past the object it reaches.
    pub params: [f32; 4],
}

/// The main view's camera as the screen passes read it (`rt_common.glsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Push {
    pub eye: [f32; 4],
    pub fwd: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub rect: [f32; 4],
    pub params: [u32; 4],
}

impl Push {
    /// For camera `cam` drawn into `rect` (target pixels) of a `width` x `height` target with `world::view_proj(cam,
    /// aspect, 1, 65536)`; `flags`: 1 shadows, 2 ambient occlusion.
    pub fn new(cam: &dx_soft::Camera, aspect: f32, rect: [u32; 4], width: u32, height: u32, casters: u32, flags: u32) -> Push {
        let (fwd, right, up) = cam.axes();
        let sx = 1.0 / crate::world::hor_plus_tan(cam.fov, aspect);
        let sy = sx * aspect;
        let (near, far) = (1.0f32, 65536.0f32);
        let q = far / (far - near);
        Push {
            eye: [cam.pos[0], cam.pos[1], cam.pos[2], q * near],
            fwd: [fwd[0], fwd[1], fwd[2], q],
            right: [right[0] / sx, right[1] / sx, right[2] / sx, 0.0],
            up: [up[0] / sy, up[1] / sy, up[2] / sy, 0.0],
            rect: rect.map(|v| v as f32),
            params: [casters, flags, width, height],
        }
    }
}

struct Image {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

pub struct RtPost {
    /// Trace, blur, apply, distance.
    set_layouts: [vk::DescriptorSetLayout; 4],
    pool: vk::DescriptorPool,
    /// The sets of each frame slot (rewritten every frame).
    sets: [[vk::DescriptorSet; 4]; N],
    layouts: [vk::PipelineLayout; 4],
    pipelines: [vk::Pipeline; 4],
    sampler: vk::Sampler,
    /// The images for a target size: half size (traced, blurred), full size (distance).
    images: Option<(u32, u32, [Image; 3])>,
    /// Each frame slot's casters (buffer, mapping, capacity).
    casters: [Option<(Buffer, *mut Caster, usize)>; N],
    pub caster_count: u32,
    /// The frame slot being recorded (from `set_casters`).
    slot: usize,
}

impl RtPost {
    pub fn new(cx: &Context) -> Result<Option<RtPost>> {
        if cx.rt.is_none() {
            return Ok(None);
        }
        unsafe {
            let frag = vk::ShaderStageFlags::FRAGMENT;
            let image = |b: u32| vk::DescriptorSetLayoutBinding::default().binding(b).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(1).stage_flags(frag);
            let trace_b = [
                image(0),
                vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR).descriptor_count(1).stage_flags(frag),
                vk::DescriptorSetLayoutBinding::default().binding(2).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(frag),
            ];
            let blur_b = [image(0)];
            let apply_b = [image(0), image(1)];
            let dist_b = [image(0), image(1)];
            let mk = |b: &[vk::DescriptorSetLayoutBinding]| cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(b), None);
            let set_layouts = [mk(&trace_b)?, mk(&blur_b)?, mk(&apply_b)?, mk(&dist_b)?];
            let sizes = [
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(6 * N as u32),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR).descriptor_count(N as u32),
                vk::DescriptorPoolSize::default().ty(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(N as u32),
            ];
            let pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(4 * N as u32).pool_sizes(&sizes), None)?;
            let mut sets = [[vk::DescriptorSet::null(); 4]; N];
            for slot in sets.iter_mut() {
                let s = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&set_layouts))?;
                *slot = [s[0], s[1], s[2], s[3]];
            }
            let push_size = |n: usize| [vk::PushConstantRange::default().stage_flags(frag).size(n as u32)];
            let pl = |set: vk::DescriptorSetLayout, size: usize| {
                let sl = [set];
                let pr = push_size(size);
                cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&sl).push_constant_ranges(&pr), None)
            };
            let push = std::mem::size_of::<Push>();
            let layouts = [pl(set_layouts[0], push)?, pl(set_layouts[1], 16)?, pl(set_layouts[2], push)?, pl(set_layouts[3], push)?];
            let vs = cx.shader_module(crate::present::POST_VERT)?;
            let make = |fs_code: &[u8], layout: vk::PipelineLayout, apply: bool, format: vk::Format| -> Result<vk::Pipeline> {
                let fs = cx.shader_module(fs_code)?;
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                    vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main"),
                ];
                let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
                let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
                let samples = if apply { cx.samples() } else { vk::SampleCountFlags::TYPE_1 };
                let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
                let ds = vk::PipelineDepthStencilStateCreateInfo::default();
                let mut att = vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA);
                if apply {
                    // the picture times the factor; its alpha kept
                    att = att
                        .blend_enable(true)
                        .src_color_blend_factor(vk::BlendFactor::DST_COLOR)
                        .dst_color_blend_factor(vk::BlendFactor::ZERO)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ZERO)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                        .alpha_blend_op(vk::BlendOp::ADD);
                } else {
                    att = att.color_write_mask(if format == DIST_FORMAT { vk::ColorComponentFlags::R } else { vk::ColorComponentFlags::R | vk::ColorComponentFlags::G });
                }
                let atts = [att];
                let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&atts);
                let scene_states = crate::SCENE_DYNAMIC_STATES;
                let plain_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
                let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(if apply { &scene_states[..] } else { &plain_states[..] });
                let color_formats = [if apply { cx.color_format() } else { format }];
                let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
                if apply {
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
                let p = cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0];
                cx.device.destroy_shader_module(fs, None);
                Ok(p)
            };
            let pipelines = [
                make(TRACE_FRAG, layouts[0], false, HALF_FORMAT)?,
                make(BLUR_FRAG, layouts[1], false, HALF_FORMAT)?,
                make(APPLY_FRAG, layouts[2], true, HALF_FORMAT)?,
                make(DIST_FRAG, layouts[3], false, DIST_FORMAT)?,
            ];
            cx.device.destroy_shader_module(vs, None);
            let sampler = cx.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            Ok(Some(RtPost { set_layouts, pool, sets, layouts, pipelines, sampler, images: None, casters: [const { None }; N], caster_count: 0, slot: 0 }))
        }
    }

    /// This frame's casters (the frame starts here: its slot is the context's).
    pub fn set_casters(&mut self, cx: &Context, casters: &[Caster]) -> Result<()> {
        self.slot = cx.frame_slot();
        let need = casters.len().max(1);
        let own = &mut self.casters[self.slot];
        if !own.as_ref().is_some_and(|(_, _, cap)| *cap >= need) {
            if let Some((b, _, _)) = own.take() {
                cx.destroy_buffer(b);
            }
            let cap = need.next_power_of_two().max(64);
            let b = cx.create_buffer(
                (cap * std::mem::size_of::<Caster>()) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            let p = unsafe { cx.device.map_memory(b.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut Caster };
            *own = Some((b, p, cap));
        }
        let (_, p, _) = own.as_ref().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(casters.as_ptr(), *p, casters.len()) };
        self.caster_count = casters.len() as u32;
        Ok(())
    }

    fn ensure_images(&mut self, cx: &Context, width: u32, height: u32) -> Result<()> {
        if self.images.as_ref().is_some_and(|(fw, fh, _)| (*fw, *fh) == (width, height)) {
            return Ok(());
        }
        if self.images.is_some() {
            // earlier frames may still use them
            unsafe { cx.device.device_wait_idle()? };
        }
        self.destroy_images(cx);
        let (hw, hh) = (width.div_ceil(2).max(1), height.div_ceil(2).max(1));
        let make = |w: u32, h: u32, format: vk::Format| -> Result<Image> {
            unsafe {
                let image = cx.device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
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
                        .format(format)
                        .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)),
                    None,
                )?;
                Ok(Image { image, memory, view })
            }
        };
        self.images = Some((width, height, [make(hw, hh, HALF_FORMAT)?, make(hw, hh, HALF_FORMAT)?, make(width, height, DIST_FORMAT)?]));
        Ok(())
    }

    fn destroy_images(&mut self, cx: &Context) {
        if let Some((_, _, images)) = self.images.take() {
            for i in images {
                unsafe {
                    cx.device.destroy_image_view(i.view, None);
                    cx.device.destroy_image(i.image, None);
                    cx.device.free_memory(i.memory, None);
                }
            }
        }
    }

    /// Each pixel's distance, then traces and blurs (outside the scene's rendering: between `Offscreen::pause` and `resume`).
    pub fn trace(&mut self, cx: &Context, cb: vk::CommandBuffer, target: &Offscreen, tlas: vk::AccelerationStructureKHR, push: &Push) -> Result<()> {
        let (Some(depth), Some(stencil)) = (target.depth_read, target.stencil_read) else { return Ok(()) };
        self.ensure_images(cx, target.width, target.height)?;
        let (fw, fh, images) = self.images.as_ref().unwrap();
        let (fw, fh) = (*fw, *fh);
        let (w, h) = (fw.div_ceil(2).max(1), fh.div_ceil(2).max(1));
        let casters = self.casters[self.slot].as_ref().map(|c| c.0.buffer).unwrap_or_default();
        let sets = self.sets[self.slot];
        unsafe {
            // the sets (rewritten every frame: the target, the structure or the casters' buffer may be new)
            let depth_info = [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(depth).image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)];
            let traced_info = [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(images[0].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let blurred_info = [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(images[1].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let stencil_info = [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(stencil).image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)];
            let dist_info = [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(images[2].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let casters_info = [vk::DescriptorBufferInfo::default().buffer(casters).range(vk::WHOLE_SIZE)];
            let handles = [tlas];
            let mut accel_info = vk::WriteDescriptorSetAccelerationStructureKHR::default().acceleration_structures(&handles);
            let mut accel_write = vk::WriteDescriptorSet::default().dst_set(sets[0]).dst_binding(1).descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR).push_next(&mut accel_info);
            accel_write.descriptor_count = 1;
            fn image_write(set: vk::DescriptorSet, binding: u32, info: &[vk::DescriptorImageInfo]) -> vk::WriteDescriptorSet<'_> {
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(binding).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(info)
            }
            let writes = [
                image_write(sets[0], 0, &depth_info),
                accel_write,
                vk::WriteDescriptorSet::default().dst_set(sets[0]).dst_binding(2).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&casters_info),
                image_write(sets[1], 0, &traced_info),
                image_write(sets[2], 0, &blurred_info),
                image_write(sets[2], 1, &dist_info),
                image_write(sets[3], 0, &depth_info),
                image_write(sets[3], 1, &stencil_info),
            ];
            cx.device.update_descriptor_sets(&writes, &[]);

            let pass = |img: &Image, (w, h): (u32, u32), pipeline: vk::Pipeline, layout: vk::PipelineLayout, set: vk::DescriptorSet, push: &[u8]| {
                let to_attach = [vk::ImageMemoryBarrier2::default()
                    .image(img.image)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                    .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                    .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1))];
                cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_attach));
                let area = vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: w, height: h } };
                let att = [vk::RenderingAttachmentInfo::default()
                    .image_view(img.view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .store_op(vk::AttachmentStoreOp::STORE)];
                cx.device.cmd_begin_rendering(cb, &vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&att));
                cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: 0.0, y: 0.0, width: w as f32, height: h as f32, min_depth: 0.0, max_depth: 1.0 }]);
                cx.device.cmd_set_scissor(cb, 0, &[area]);
                cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
                cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, layout, 0, &[set], &[]);
                cx.device.cmd_push_constants(cb, layout, vk::ShaderStageFlags::FRAGMENT, 0, push);
                cx.device.cmd_draw(cb, 3, 1, 0, 0);
                cx.device.cmd_end_rendering(cb);
                let to_read = [vk::ImageMemoryBarrier2::default()
                    .image(img.image)
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1))];
                cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_read));
            };
            let push_bytes = std::slice::from_raw_parts(push as *const Push as *const u8, std::mem::size_of::<Push>());
            pass(&images[2], (fw, fh), self.pipelines[3], self.layouts[3], sets[3], push_bytes);
            pass(&images[0], (w, h), self.pipelines[0], self.layouts[0], sets[0], push_bytes);
            let size = [w as f32, h as f32, 0.0, 0.0];
            let size_bytes = std::slice::from_raw_parts(size.as_ptr() as *const u8, 16);
            pass(&images[1], (w, h), self.pipelines[1], self.layouts[1], sets[1], size_bytes);
        }
        Ok(())
    }

    /// Multiplies the blurred factor into the picture (inside the scene's rendering, after `Offscreen::resume`; the
    /// caller sets the stencil test that leaves mirror areas out).
    pub fn apply(&self, cx: &Context, cb: vk::CommandBuffer, push: &Push) {
        unsafe {
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipelines[2]);
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layouts[2], 0, &[self.sets[self.slot][2]], &[]);
            let bytes = std::slice::from_raw_parts(push as *const Push as *const u8, std::mem::size_of::<Push>());
            cx.device.cmd_push_constants(cb, self.layouts[2], vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            cx.device.cmd_draw(cb, 3, 1, 0, 0);
        }
    }

    pub fn destroy(mut self, cx: &Context) {
        self.destroy_images(cx);
        for (b, _, _) in self.casters.iter_mut().filter_map(Option::take) {
            cx.destroy_buffer(b);
        }
        unsafe {
            for p in self.pipelines {
                cx.device.destroy_pipeline(p, None);
            }
            for l in self.layouts {
                cx.device.destroy_pipeline_layout(l, None);
            }
            cx.device.destroy_descriptor_pool(self.pool, None);
            for l in self.set_layouts {
                cx.device.destroy_descriptor_set_layout(l, None);
            }
            cx.device.destroy_sampler(self.sampler, None);
        }
    }
}
