//! Showing frames in a window: a swapchain, and a final fullscreen pass that applies the engine's brightness (gamma) and the crosshair
//! while copying the offscreen colour image into the swapchain image. The surface comes from the windowing library (SDL3 in `dx-play`).
use crate::{Context, Offscreen, Result};
use ash::vk;

pub const POST_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/post.vert.spv"));
pub const POST_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/post.frag.spv"));

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    gamma_exp: f32,
    crosshair: f32,
    size: [f32; 2],
}

/// One frame being recorded; get it from `Presenter::begin_frame`, finish with `Presenter::end_frame`.
pub struct Frame {
    pub cb: vk::CommandBuffer,
    image: u32,
}

pub struct Presenter {
    surface_loader: ash::khr::surface::Instance,
    swapchain_loader: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    format: vk::Format,
    pub extent: vk::Extent2D,
    image_available: vk::Semaphore,
    render_finished: Vec<vk::Semaphore>,
    in_flight: vk::Fence,
    cb: vk::CommandBuffer,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pub present_mode: vk::PresentModeKHR,
    /// The vertical sync mode (`VSync`): see `present_modes`.
    pub vsync: VSync,
}

impl Presenter {
    /// `surface` must belong to `cx.instance`. `size` is the window size in pixels.
    pub fn new(cx: &Context, surface: vk::SurfaceKHR, size: (u32, u32), vsync: VSync) -> Result<Presenter> {
        unsafe {
            let entry_instance = &cx.instance;
            let surface_loader = ash::khr::surface::Instance::new(&cx.entry, entry_instance);
            let swapchain_loader = ash::khr::swapchain::Device::new(entry_instance, &cx.device);
            let supported = surface_loader.get_physical_device_surface_support(cx.physical, cx.queue_family, surface)?;
            if !supported {
                return Err(crate::Error("the graphics queue cannot present to this window".into()));
            }
            let formats = surface_loader.get_physical_device_surface_formats(cx.physical, surface)?;
            // our colours are final display values (gamma is applied by us): use a non-sRGB format
            let format = [vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM]
                .into_iter()
                .find(|f| formats.iter().any(|s| s.format == *f))
                .unwrap_or(formats[0].format);

            let sampler = cx.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = cx.device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)?;
            let sizes = [vk::DescriptorPoolSize::default().ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).descriptor_count(1)];
            let pool = cx.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&sizes), None)?;
            let layouts = [set_layout];
            let set = cx.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))?[0];

            let push = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::FRAGMENT).size(std::mem::size_of::<Push>() as u32)];
            let layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts).push_constant_ranges(&push), None)?;
            let vs = cx.shader_module(POST_VERT)?;
            let fs = cx.shader_module(POST_FRAG)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main"),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
            let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
            let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let blend_att = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_att);
            let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
            let color_formats = [format];
            let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_asm)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&ms)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(layout)
                .push_next(&mut rendering);
            let pipeline = cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0];
            cx.device.destroy_shader_module(vs, None);
            cx.device.destroy_shader_module(fs, None);

            let cb = cx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default().command_pool(cx.pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1),
            )?[0];
            let image_available = cx.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
            let in_flight = cx.device.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None)?;
            let mut p = Presenter {
                surface_loader,
                swapchain_loader,
                surface,
                swapchain: vk::SwapchainKHR::null(),
                images: Vec::new(),
                views: Vec::new(),
                format,
                extent: vk::Extent2D { width: size.0, height: size.1 },
                image_available,
                render_finished: Vec::new(),
                in_flight,
                cb,
                set_layout,
                pool,
                set,
                sampler,
                layout,
                pipeline,
                present_mode: vk::PresentModeKHR::FIFO,
                vsync,
            };
            p.create_swapchain(cx, size)?;
            Ok(p)
        }
    }

    fn create_swapchain(&mut self, cx: &Context, size: (u32, u32)) -> Result<()> {
        unsafe {
            cx.device.device_wait_idle()?;
            let caps = self.surface_loader.get_physical_device_surface_capabilities(cx.physical, self.surface)?;
            let extent = if caps.current_extent.width != u32::MAX {
                caps.current_extent
            } else {
                vk::Extent2D {
                    width: size.0.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                    height: size.1.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
                }
            };
            if extent.width == 0 || extent.height == 0 {
                self.extent = extent;
                return Ok(());
            }
            let modes = self.surface_loader.get_physical_device_surface_present_modes(cx.physical, self.surface)?;
            // the mode's first supported present mode (FIFO always exists)
            self.present_mode = self.vsync.present_modes().iter().copied().find(|m| modes.contains(m)).unwrap_or(vk::PresentModeKHR::FIFO);
            let mut count = caps.min_image_count + 1;
            if caps.max_image_count > 0 {
                count = count.min(caps.max_image_count);
            }
            let old = self.swapchain;
            let info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface)
                .min_image_count(count)
                .image_format(self.format)
                .image_color_space(vk::ColorSpaceKHR::SRGB_NONLINEAR)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                .present_mode(self.present_mode)
                .clipped(true)
                .old_swapchain(old);
            self.swapchain = self.swapchain_loader.create_swapchain(&info, None)?;
            for v in self.views.drain(..) {
                cx.device.destroy_image_view(v, None);
            }
            for s in self.render_finished.drain(..) {
                cx.device.destroy_semaphore(s, None);
            }
            if old != vk::SwapchainKHR::null() {
                self.swapchain_loader.destroy_swapchain(old, None);
            }
            self.images = self.swapchain_loader.get_swapchain_images(self.swapchain)?;
            for &img in &self.images {
                self.views.push(cx.device.create_image_view(
                    &vk::ImageViewCreateInfo::default().image(img).view_type(vk::ImageViewType::TYPE_2D).format(self.format).subresource_range(
                        vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1),
                    ),
                    None,
                )?);
                self.render_finished.push(cx.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?);
            }
            self.extent = extent;
            Ok(())
        }
    }

    /// Rebuilds the swapchain for a new window size.
    pub fn resize(&mut self, cx: &Context, size: (u32, u32)) -> Result<()> {
        self.create_swapchain(cx, size)
    }

    /// Changes the vertical sync mode (the swapchain is made again when it changes).
    pub fn set_vsync(&mut self, cx: &Context, mode: VSync) -> Result<()> {
        if mode == self.vsync {
            return Ok(());
        }
        self.vsync = mode;
        let size = (self.extent.width, self.extent.height);
        self.create_swapchain(cx, size)
    }

    /// Waits for the previous frame, acquires the next swapchain image and starts the command buffer. `None`: nothing to draw now
    /// (minimised window, or the swapchain had to be rebuilt: call again).
    pub fn begin_frame(&mut self, cx: &Context, window_size: (u32, u32)) -> Result<Option<Frame>> {
        unsafe {
            if self.extent.width == 0 || self.extent.height == 0 || self.swapchain == vk::SwapchainKHR::null() || self.images.is_empty() {
                self.create_swapchain(cx, window_size)?;
                return Ok(None);
            }
            cx.device.wait_for_fences(&[self.in_flight], true, u64::MAX)?;
            let image = match self.swapchain_loader.acquire_next_image(self.swapchain, u64::MAX, self.image_available, vk::Fence::null()) {
                Ok((i, _suboptimal)) => i,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.create_swapchain(cx, window_size)?;
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            };
            cx.device.reset_fences(&[self.in_flight])?;
            cx.device.reset_command_buffer(self.cb, vk::CommandBufferResetFlags::empty())?;
            cx.device.begin_command_buffer(self.cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
            Ok(Some(Frame { cb: self.cb, image }))
        }
    }

    /// Records the final pass (`scene` is the offscreen target, already ended with `end_for_sampling`), submits and presents.
    /// Returns false when the swapchain is out of date (the next `begin_frame` rebuilds it).
    pub fn end_frame(&mut self, cx: &Context, frame: Frame, scene: &Offscreen, gamma_exp: f32, crosshair: bool) -> Result<bool> {
        unsafe {
            let cb = frame.cb;
            let img = self.images[frame.image as usize];
            let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
            let to_color = [vk::ImageMemoryBarrier2::default()
                .image(img)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_color));
            let info = [vk::DescriptorImageInfo::default().sampler(self.sampler).image_view(scene.color_view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            cx.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(0).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&info)],
                &[],
            );
            // the scene keeps its aspect ratio: scaled to fit, centred, black bars around it
            let att = [vk::RenderingAttachmentInfo::default()
                .image_view(self.views[frame.image as usize])
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .clear_value(vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] } })
                .store_op(vk::AttachmentStoreOp::STORE)];
            let area = vk::Rect2D { offset: vk::Offset2D::default(), extent: self.extent };
            cx.device.cmd_begin_rendering(cb, &vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&att));
            let [x, y, w, h] = fit_rect((scene.width, scene.height), (self.extent.width, self.extent.height));
            cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x, y, width: w, height: h, min_depth: 0.0, max_depth: 1.0 }]);
            cx.device.cmd_set_scissor(cb, 0, &[area]);
            cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            cx.device.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.set], &[]);
            let push = Push { gamma_exp, crosshair: crosshair as u32 as f32, size: [self.extent.width as f32, self.extent.height as f32] };
            let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
            cx.device.cmd_push_constants(cb, self.layout, vk::ShaderStageFlags::FRAGMENT, 0, bytes);
            cx.device.cmd_draw(cb, 3, 1, 0, 0);
            cx.device.cmd_end_rendering(cb);
            let to_present = [vk::ImageMemoryBarrier2::default()
                .image(img)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::BOTTOM_OF_PIPE)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_present));
            cx.device.end_command_buffer(cb)?;
            let wait = [self.image_available];
            let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let cbs = [cb];
            let signal = [self.render_finished[frame.image as usize]];
            cx.device.queue_submit(cx.queue, &[vk::SubmitInfo::default().wait_semaphores(&wait).wait_dst_stage_mask(&stages).command_buffers(&cbs).signal_semaphores(&signal)], self.in_flight)?;
            let swapchains = [self.swapchain];
            let indices = [frame.image];
            match self.swapchain_loader.queue_present(cx.queue, &vk::PresentInfoKHR::default().wait_semaphores(&signal).swapchains(&swapchains).image_indices(&indices)) {
                Ok(suboptimal) => Ok(!suboptimal),
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(false),
                Err(e) => Err(e.into()),
            }
        }
    }

    pub fn destroy(mut self, cx: &Context) {
        unsafe {
            let _ = cx.device.device_wait_idle();
            for v in self.views.drain(..) {
                cx.device.destroy_image_view(v, None);
            }
            for s in self.render_finished.drain(..) {
                cx.device.destroy_semaphore(s, None);
            }
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);
            cx.device.destroy_semaphore(self.image_available, None);
            cx.device.destroy_fence(self.in_flight, None);
            cx.device.destroy_pipeline(self.pipeline, None);
            cx.device.destroy_pipeline_layout(self.layout, None);
            cx.device.destroy_descriptor_pool(self.pool, None);
            cx.device.destroy_descriptor_set_layout(self.set_layout, None);
            cx.device.destroy_sampler(self.sampler, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}

/// Where a `scene`-sized picture goes in a `window`-sized one: as large as fits with the same aspect ratio, centred
/// (`[x, y, width, height]` in window pixels).
pub fn fit_rect(scene: (u32, u32), window: (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (scene.0.max(1) as f32, scene.1.max(1) as f32);
    let (ww, wh) = (window.0 as f32, window.1 as f32);
    let k = (ww / sw).min(wh / sh);
    let (w, h) = ((sw * k).round(), (sh * k).round());
    [((ww - w) / 2.0).floor(), ((wh - h) / 2.0).floor(), w, h]
}

/// Vertical sync as the player chooses it (the port's menu, `AerugoSettings.VSync`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VSync {
    /// Frames shown as soon as they are done: lowest latency, may tear (within a FreeSync/G-Sync range the display
    /// follows the frames anyway).
    Off,
    /// Wait for the vertical blank: no tearing, capped at the refresh rate (with a variable refresh rate display and a
    /// frame limit just below it, no tearing and low latency).
    On,
    /// Wait while frames keep up, show at once (tearing) when one is late.
    Adaptive,
    /// Newest finished frame at each vertical blank: no tearing, not capped (frames that are not shown are dropped).
    Fast,
}

impl VSync {
    /// From the setting's number (0 off, 1 on, 2 adaptive, 3 fast; anything else on).
    pub fn from_setting(v: i32) -> VSync {
        match v {
            0 => VSync::Off,
            2 => VSync::Adaptive,
            3 => VSync::Fast,
            _ => VSync::On,
        }
    }

    /// Present modes in order of preference (drivers and compositors differ in what they offer; FIFO always exists).
    pub fn present_modes(self) -> &'static [vk::PresentModeKHR] {
        use vk::PresentModeKHR as M;
        match self {
            VSync::Off => &[M::IMMEDIATE, M::MAILBOX, M::FIFO],
            VSync::On => &[M::FIFO],
            VSync::Adaptive => &[M::FIFO_RELAXED, M::FIFO],
            VSync::Fast => &[M::MAILBOX, M::IMMEDIATE, M::FIFO],
        }
    }
}

