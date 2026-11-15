//! Vulkan renderer. Step 1: instance + device (Vulkan 1.3 dynamic rendering), an offscreen colour+depth target and
//! readback to RGBA8, so frames can be checked headlessly (and compared with the CPU renderer `dx-soft`).

/// D3DDrv's texture stage setup (10009e20, asm) with the shipped `UseMipmapping=True`, `UseTrilinear=False`: magnify
/// and minify LINEAR (POINT for `PF_NoSmooth`), mip levels POINT (the nearest level, no blend between levels) and a mip
/// LOD bias of -0.5 on every stage (`LODBIAS` console command only).
pub const MIP_MODE: ash::vk::SamplerMipmapMode = ash::vk::SamplerMipmapMode::NEAREST;
pub const MIP_LOD_BIAS: f32 = -0.5;

pub mod texture;
pub mod world;
pub mod actors;
pub mod lodmesh;
pub mod renderer;
pub mod present;
pub mod ui;

pub use ash;
use ash::vk;
use std::ffi::{c_char, CStr};

pub const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// Depth with a stencil (the stencil marks where mirror surfaces are visible, so reflections are drawn only there).
pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT_S8_UINT;
const DEPTH_ASPECTS: vk::ImageAspectFlags = vk::ImageAspectFlags::from_raw(vk::ImageAspectFlags::DEPTH.as_raw() | vk::ImageAspectFlags::STENCIL.as_raw());

/// Dynamic state of every pipeline that draws into the scene target: viewport, scissor and the stencil test (set by
/// `set_stencil`; `Offscreen::begin` turns it off).
pub const SCENE_DYNAMIC_STATES: [vk::DynamicState; 7] = [
    vk::DynamicState::VIEWPORT,
    vk::DynamicState::SCISSOR,
    vk::DynamicState::STENCIL_TEST_ENABLE,
    vk::DynamicState::STENCIL_OP,
    vk::DynamicState::STENCIL_COMPARE_MASK,
    vk::DynamicState::STENCIL_WRITE_MASK,
    vk::DynamicState::STENCIL_REFERENCE,
];

/// Stencil use while drawing the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stencil {
    Off,
    /// Write this value wherever a fragment passes the depth test.
    Mark(u32),
    /// Draw only where the stencil holds this value.
    Inside(u32),
    /// Where the stencil holds this value and the depth test passes, add one (a child frame's area inside its parent's).
    Incr(u32),
    /// Where the stencil holds this value, take one away (the child frame's area given back to its parent).
    Decr(u32),
}

pub fn set_stencil(cx: &Context, cb: vk::CommandBuffer, mode: Stencil) {
    let face = vk::StencilFaceFlags::FRONT_AND_BACK;
    let (enable, pass, compare, reference) = match mode {
        Stencil::Off => (false, vk::StencilOp::KEEP, vk::CompareOp::ALWAYS, 0),
        Stencil::Mark(r) => (true, vk::StencilOp::REPLACE, vk::CompareOp::ALWAYS, r),
        Stencil::Inside(r) => (true, vk::StencilOp::KEEP, vk::CompareOp::EQUAL, r),
        Stencil::Incr(r) => (true, vk::StencilOp::INCREMENT_AND_CLAMP, vk::CompareOp::EQUAL, r),
        Stencil::Decr(r) => (true, vk::StencilOp::DECREMENT_AND_CLAMP, vk::CompareOp::EQUAL, r),
    };
    unsafe {
        cx.device.cmd_set_stencil_test_enable(cb, enable);
        cx.device.cmd_set_stencil_op(cb, face, vk::StencilOp::KEEP, pass, vk::StencilOp::KEEP, compare);
        cx.device.cmd_set_stencil_compare_mask(cb, face, 0xff);
        cx.device.cmd_set_stencil_write_mask(cb, face, 0xff);
        cx.device.cmd_set_stencil_reference(cb, face, reference);
    }
}

#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<vk::Result> for Error {
    fn from(r: vk::Result) -> Self {
        Error(format!("vulkan: {r}"))
    }
}
impl From<ash::LoadingError> for Error {
    fn from(e: ash::LoadingError) -> Self {
        Error(format!("cannot load the Vulkan loader: {e}"))
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// A buffer with its own memory.
pub struct Buffer {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
}

/// Instance, device, one graphics queue, and a command pool.
pub struct Context {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub pool: vk::CommandPool,
    mem_props: vk::PhysicalDeviceMemoryProperties,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub device_name: String,
    /// `VK_EXT_memory_budget` is enabled (see `video_memory`).
    memory_budget: bool,
    /// Nanoseconds per timestamp tick.
    pub timestamp_period: f32,
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let msg = CStr::from_ptr((*data).p_message).to_string_lossy();
    let level = if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) { "error" } else { "warning" };
    eprintln!("[vulkan {level}] {msg}");
    vk::FALSE
}

impl Context {
    /// `validation`: enable the Khronos validation layer if it is installed (messages go to stderr).
    /// `instance_extensions`: extra extensions, e.g. the ones SDL needs for a window surface.
    pub fn new(validation: bool, instance_extensions: &[*const c_char]) -> Result<Context> {
        unsafe {
            let entry = ash::Entry::load()?;
            let layer = c"VK_LAYER_KHRONOS_validation";
            let have_layer = validation
                && entry
                    .enumerate_instance_layer_properties()?
                    .iter()
                    .any(|l| l.layer_name_as_c_str().map(|n| n == layer).unwrap_or(false));
            let mut exts: Vec<*const c_char> = instance_extensions.to_vec();
            if have_layer {
                exts.push(ash::ext::debug_utils::NAME.as_ptr());
            }
            let layers = [layer.as_ptr()];
            let app = vk::ApplicationInfo::default()
                .application_name(c"Deus Ex Aerugo")
                .api_version(vk::make_api_version(0, 1, 3, 0));
            let mut info = vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(&exts);
            if have_layer {
                info = info.enabled_layer_names(&layers);
            }
            let instance = entry.create_instance(&info, None)?;

            let debug = if have_layer {
                let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
                let ci = vk::DebugUtilsMessengerCreateInfoEXT::default()
                    .message_severity(
                        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                    )
                    .message_type(
                        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                            | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                            | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                    )
                    .pfn_user_callback(Some(debug_callback));
                let messenger = loader.create_debug_utils_messenger(&ci, None)?;
                Some((loader, messenger))
            } else {
                None
            };

            // Prefer a discrete GPU.
            let devices = instance.enumerate_physical_devices()?;
            let physical = *devices
                .iter()
                .max_by_key(|&&d| match instance.get_physical_device_properties(d).device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 3,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
                    _ => 1,
                })
                .ok_or_else(|| Error("no Vulkan device".into()))?;
            let props = instance.get_physical_device_properties(physical);
            let device_name = props.device_name_as_c_str().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
            if props.api_version < vk::API_VERSION_1_3 {
                return Err(Error(format!("{device_name}: Vulkan 1.3 is required")));
            }
            let fmt = instance.get_physical_device_format_properties(physical, DEPTH_FORMAT);
            if !fmt.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT) {
                return Err(Error(format!("{device_name}: the depth/stencil format {DEPTH_FORMAT:?} is not supported")));
            }
            let queue_family = instance
                .get_physical_device_queue_family_properties(physical)
                .iter()
                .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .ok_or_else(|| Error("no graphics queue".into()))? as u32;

            let priorities = [1.0f32];
            let queue_info = [vk::DeviceQueueCreateInfo::default().queue_family_index(queue_family).queue_priorities(&priorities)];
            let mut f13 = vk::PhysicalDeviceVulkan13Features::default().dynamic_rendering(true).synchronization2(true).shader_demote_to_helper_invocation(true);
            // the memory budget (what this process uses of each heap) when the driver has it: for measurements
            let available = instance.enumerate_device_extension_properties(physical)?;
            let has = |name: &CStr| available.iter().any(|e| e.extension_name_as_c_str().map(|n| n == name).unwrap_or(false));
            let memory_budget = has(ash::ext::memory_budget::NAME);
            let mut dev_exts: Vec<*const c_char> = Vec::new();
            if !instance_extensions.is_empty() {
                dev_exts.push(ash::khr::swapchain::NAME.as_ptr());
            }
            if memory_budget {
                dev_exts.push(ash::ext::memory_budget::NAME.as_ptr());
            }
            let dev_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_info)
                .enabled_extension_names(&dev_exts)
                .push_next(&mut f13);
            let device = instance.create_device(physical, &dev_info, None)?;
            let queue = device.get_device_queue(queue_family, 0);
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let mem_props = instance.get_physical_device_memory_properties(physical);
            let timestamp_period = props.limits.timestamp_period;
            Ok(Context { entry, instance, physical, device, queue, queue_family, pool, mem_props, debug, device_name, memory_budget, timestamp_period })
        }
    }

    /// Bytes this process uses of the device-local heaps, and their budget (`VK_EXT_memory_budget`); `None` without it.
    pub fn video_memory(&self) -> Option<(u64, u64)> {
        if !self.memory_budget {
            return None;
        }
        let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        let mut props = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
        unsafe { self.instance.get_physical_device_memory_properties2(self.physical, &mut props) };
        let heaps = props.memory_properties.memory_heap_count as usize;
        let local: Vec<usize> = (0..heaps).filter(|&i| props.memory_properties.memory_heaps[i].flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL)).collect();
        Some((local.iter().map(|&i| budget.heap_usage[i]).sum(), local.iter().map(|&i| budget.heap_budget[i]).sum()))
    }

    pub fn memory_type(&self, bits: u32, want: vk::MemoryPropertyFlags) -> Result<u32> {
        (0..self.mem_props.memory_type_count)
            .find(|&i| bits & (1 << i) != 0 && self.mem_props.memory_types[i as usize].property_flags.contains(want))
            .ok_or_else(|| Error("no suitable memory type".into()))
    }

    pub fn create_buffer(&self, size: u64, usage: vk::BufferUsageFlags, props: vk::MemoryPropertyFlags) -> Result<Buffer> {
        unsafe {
            let buffer = self
                .device
                .create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE), None)?;
            let req = self.device.get_buffer_memory_requirements(buffer);
            let memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(self.memory_type(req.memory_type_bits, props)?),
                None,
            )?;
            self.device.bind_buffer_memory(buffer, memory, 0)?;
            Ok(Buffer { buffer, memory, size })
        }
    }

    /// Host-visible buffer filled with `data`.
    pub fn upload<T: Copy>(&self, data: &[T], usage: vk::BufferUsageFlags) -> Result<Buffer> {
        let size = std::mem::size_of_val(data) as u64;
        let buf = self.create_buffer(
            size.max(4),
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        unsafe {
            let p = self.device.map_memory(buf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;
            std::ptr::copy_nonoverlapping(data.as_ptr() as *const u8, p, size as usize);
            self.device.unmap_memory(buf.memory);
        }
        Ok(buf)
    }

    pub fn destroy_buffer(&self, b: Buffer) {
        unsafe {
            self.device.destroy_buffer(b.buffer, None);
            self.device.free_memory(b.memory, None);
        }
    }

    pub fn shader_module(&self, spv: &[u8]) -> Result<vk::ShaderModule> {
        assert!(spv.len() % 4 == 0);
        let words: Vec<u32> = spv.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        unsafe { Ok(self.device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)?) }
    }

    /// Record and submit one-shot commands, wait for completion.
    pub fn one_shot(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<()> {
        unsafe {
            let cb = self.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default().command_pool(self.pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1),
            )?[0];
            self.device.begin_command_buffer(cb, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
            record(cb);
            self.device.end_command_buffer(cb)?;
            let fence = self.device.create_fence(&vk::FenceCreateInfo::default(), None)?;
            let cbs = [cb];
            self.device.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cbs)], fence)?;
            self.device.wait_for_fences(&[fence], true, u64::MAX)?;
            self.device.destroy_fence(fence, None);
            self.device.free_command_buffers(self.pool, &cbs);
        }
        Ok(())
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            if let Some((loader, m)) = self.debug.take() {
                loader.destroy_debug_utils_messenger(m, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// Offscreen colour + depth target. Colour can be read back to RGBA8.
pub struct Offscreen {
    pub width: u32,
    pub height: u32,
    pub color: vk::Image,
    pub color_view: vk::ImageView,
    color_mem: vk::DeviceMemory,
    pub depth: vk::Image,
    pub depth_view: vk::ImageView,
    depth_mem: vk::DeviceMemory,
}

impl Offscreen {
    pub fn new(cx: &Context, width: u32, height: u32) -> Result<Offscreen> {
        let make = |format, usage, aspect| -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
            unsafe {
                let img = cx.device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D { width, height, depth: 1 })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(usage)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )?;
                let req = cx.device.get_image_memory_requirements(img);
                let mem = cx.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(cx.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?),
                    None,
                )?;
                cx.device.bind_image_memory(img, mem, 0)?;
                let view = cx.device.create_image_view(
                    &vk::ImageViewCreateInfo::default().image(img).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(
                        vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1),
                    ),
                    None,
                )?;
                Ok((img, mem, view))
            }
        };
        let (color, color_mem, color_view) =
            make(COLOR_FORMAT, vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED, vk::ImageAspectFlags::COLOR)?;
        let (depth, depth_mem, depth_view) =
            make(DEPTH_FORMAT, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, DEPTH_ASPECTS)?;
        Ok(Offscreen { width, height, color, color_view, color_mem, depth, depth_view, depth_mem })
    }

    pub fn extent(&self) -> vk::Extent2D {
        vk::Extent2D { width: self.width, height: self.height }
    }

    /// Transitions to attachment layouts and starts dynamic rendering, clearing colour and depth.
    pub fn begin(&self, cx: &Context, cb: vk::CommandBuffer, clear: [f32; 4]) {
        let barrier = |image, aspect, new, dst_stage, dst_access| {
            vk::ImageMemoryBarrier2::default()
                .image(image)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(new)
                .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
                .dst_stage_mask(dst_stage)
                .dst_access_mask(dst_access)
                .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1))
        };
        let barriers = [
            barrier(
                self.color,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ),
            barrier(
                self.depth,
                DEPTH_ASPECTS,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
        ];
        let color_att = [vk::RenderingAttachmentInfo::default()
            .image_view(self.color_view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue { color: vk::ClearColorValue { float32: clear } })];
        let depth_att = vk::RenderingAttachmentInfo::default()
            .image_view(self.depth_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } });
        unsafe {
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&barriers));
            let area = vk::Rect2D { offset: vk::Offset2D::default(), extent: self.extent() };
            cx.device.cmd_begin_rendering(
                cb,
                &vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&color_att).depth_attachment(&depth_att).stencil_attachment(&depth_att),
            );
            // Flipped viewport (negative height) so +Y is up in clip space like in GL conventions.
            cx.device.cmd_set_viewport(
                cb,
                0,
                &[vk::Viewport { x: 0.0, y: self.height as f32, width: self.width as f32, height: -(self.height as f32), min_depth: 0.0, max_depth: 1.0 }],
            );
            cx.device.cmd_set_scissor(cb, 0, &[area]);
        }
        set_stencil(cx, cb, Stencil::Off);
    }

    /// Clears the depth buffer inside the current rendering scope (between the sky pass and the world pass).
    pub fn clear_depth(&self, cx: &Context, cb: vk::CommandBuffer) {
        self.clear_depth_in(cx, cb, None);
    }

    /// `clear_depth` inside `area` only (`None`: the whole target).
    pub fn clear_depth_in(&self, cx: &Context, cb: vk::CommandBuffer, area: Option<vk::Rect2D>) {
        let att = [vk::ClearAttachment::default()
            .aspect_mask(vk::ImageAspectFlags::DEPTH)
            .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } })];
        let rect = [vk::ClearRect::default().rect(area.unwrap_or(vk::Rect2D { offset: vk::Offset2D::default(), extent: self.extent() })).layer_count(1)];
        unsafe { cx.device.cmd_clear_attachments(cb, &att, &rect) };
    }

    /// Ends rendering and makes the colour image readable by shaders (for the final pass into a swapchain image).
    pub fn end_for_sampling(&self, cx: &Context, cb: vk::CommandBuffer) {
        unsafe {
            cx.device.cmd_end_rendering(cb);
            let barrier = [vk::ImageMemoryBarrier2::default()
                .image(self.color)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1))];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&barrier));
        }
    }

    /// Ends rendering and copies the colour image into `dst` (width*height*4 bytes).
    pub fn end_and_copy(&self, cx: &Context, cb: vk::CommandBuffer, dst: &Buffer) {
        unsafe {
            cx.device.cmd_end_rendering(cb);
            let barrier = [vk::ImageMemoryBarrier2::default()
                .image(self.color)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1))];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&barrier));
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            cx.device.cmd_copy_image_to_buffer(cb, self.color, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst.buffer, &[region]);
        }
    }

    /// Copies the colour image (left in `layout` by the last frame) into `dst` and leaves it in `layout` again: the last
    /// frame read back on demand (`GenerateSnapshot`).
    pub fn copy_back(&self, cx: &Context, cb: vk::CommandBuffer, dst: &Buffer, layout: vk::ImageLayout) {
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        unsafe {
            let to_src = [vk::ImageMemoryBarrier2::default()
                .image(self.color)
                .old_layout(layout)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_src));
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            cx.device.cmd_copy_image_to_buffer(cb, self.color, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, dst.buffer, &[region]);
            let back = [vk::ImageMemoryBarrier2::default()
                .image(self.color)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(layout)
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&back));
        }
    }

    /// A host-visible buffer sized for the colour image.
    pub fn readback_buffer(&self, cx: &Context) -> Result<Buffer> {
        cx.create_buffer(
            self.width as u64 * self.height as u64 * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
    }

    pub fn read_pixels(&self, cx: &Context, buf: &Buffer) -> Result<Vec<u8>> {
        let n = (self.width * self.height * 4) as usize;
        let mut out = vec![0u8; n];
        unsafe {
            let p = cx.device.map_memory(buf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *const u8;
            std::ptr::copy_nonoverlapping(p, out.as_mut_ptr(), n);
            cx.device.unmap_memory(buf.memory);
        }
        Ok(out)
    }

    pub fn destroy(self, cx: &Context) {
        unsafe {
            cx.device.destroy_image_view(self.color_view, None);
            cx.device.destroy_image(self.color, None);
            cx.device.free_memory(self.color_mem, None);
            cx.device.destroy_image_view(self.depth_view, None);
            cx.device.destroy_image(self.depth, None);
            cx.device.free_memory(self.depth_mem, None);
        }
    }
}

pub const TRI_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tri.vert.spv"));
pub const TRI_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tri.frag.spv"));

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ColorVertex {
    pub pos: [f32; 3],
    pub color: [f32; 3],
}

/// Pipeline for coloured triangles with a 4x4 matrix push constant, depth test on. Used to prove the whole path.
pub struct TrianglePipeline {
    pub layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
}

impl TrianglePipeline {
    pub fn new(cx: &Context) -> Result<TrianglePipeline> {
        unsafe {
            let vs = cx.shader_module(TRI_VERT)?;
            let fs = cx.shader_module(TRI_FRAG)?;
            let push = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX).size(64)];
            let layout = cx.device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push), None)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"main"),
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(c"main"),
            ];
            let bindings = [vk::VertexInputBindingDescription::default().stride(24).input_rate(vk::VertexInputRate::VERTEX)];
            let attrs = [
                vk::VertexInputAttributeDescription::default().location(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
                vk::VertexInputAttributeDescription::default().location(1).format(vk::Format::R32G32B32_SFLOAT).offset(12),
            ];
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&bindings).vertex_attribute_descriptions(&attrs);
            let input_asm = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
            let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let ds = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(true).depth_compare_op(vk::CompareOp::LESS);
            let blend_att = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_att);
            let dyn_states = crate::SCENE_DYNAMIC_STATES;
            let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyn_states);
            let color_formats = [COLOR_FORMAT];
            let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&color_formats).depth_attachment_format(DEPTH_FORMAT).stencil_attachment_format(DEPTH_FORMAT);
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
            let pipeline = cx.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None).map_err(|(_, e)| e)?[0];
            cx.device.destroy_shader_module(vs, None);
            cx.device.destroy_shader_module(fs, None);
            Ok(TrianglePipeline { layout, pipeline })
        }
    }

    pub fn destroy(self, cx: &Context) {
        unsafe {
            cx.device.destroy_pipeline(self.pipeline, None);
            cx.device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

/// Renders vertices (a triangle list) with `view_proj` (column-major) into a fresh offscreen target and returns RGBA8 pixels.
pub fn render_triangles(cx: &Context, width: u32, height: u32, verts: &[ColorVertex], view_proj: &[f32; 16]) -> Result<Vec<u8>> {
    let target = Offscreen::new(cx, width, height)?;
    let pipe = TrianglePipeline::new(cx)?;
    let vb = cx.upload(verts, vk::BufferUsageFlags::VERTEX_BUFFER)?;
    let rb = target.readback_buffer(cx)?;
    cx.one_shot(|cb| unsafe {
        target.begin(cx, cb, [0.05, 0.05, 0.1, 1.0]);
        cx.device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipe.pipeline);
        let bytes: &[u8] = std::slice::from_raw_parts(view_proj.as_ptr() as *const u8, 64);
        cx.device.cmd_push_constants(cb, pipe.layout, vk::ShaderStageFlags::VERTEX, 0, bytes);
        cx.device.cmd_bind_vertex_buffers(cb, 0, &[vb.buffer], &[0]);
        cx.device.cmd_draw(cb, verts.len() as u32, 1, 0, 0);
        target.end_and_copy(cx, cb, &rb);
    })?;
    let pixels = target.read_pixels(cx, &rb)?;
    cx.destroy_buffer(rb);
    cx.destroy_buffer(vb);
    pipe.destroy(cx);
    target.destroy(cx);
    Ok(pixels)
}

/// The engine's global brightness ("gamma") setting, `Brightness` in `[WinDrv.WindowsClient]` of `DeusEx.ini` (0.6 as shipped).
/// The original's hardware renderers (`D3DDrv`, `OpenGlDrv`) load a gamma ramp `ramp[i] = (i / 255) ^ (1 / (2.5 * Brightness))`
/// for the whole screen (read from the drivers' disassembly; constants 2.5, 1/255 and 65535). This is that exponent.
pub fn brightness_exponent(brightness: f32) -> f32 {
    1.0 / (2.5 * brightness.max(0.01))
}

/// Applies the gamma ramp of `brightness_exponent` to RGBA8 pixels in place (alpha untouched).
pub fn apply_brightness(rgba: &mut [u8], brightness: f32) {
    let e = brightness_exponent(brightness) as f64;
    let lut: Vec<u8> = (0..256).map(|i| (255.0 * (i as f64 / 255.0).powf(e)).round().clamp(0.0, 255.0) as u8).collect();
    for px in rgba.chunks_exact_mut(4) {
        for c in &mut px[..3] {
            *c = lut[*c as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_follows_the_drivers_gamma_ramp() {
        let src: Vec<u8> = (0..=255u8).flat_map(|i| [i, i, i, 255]).collect();
        // 0.4 is neutral: 1 / (2.5 * 0.4) = 1
        let mut a = src.clone();
        apply_brightness(&mut a, 0.4);
        assert_eq!(a, src);
        let mut b = src.clone();
        apply_brightness(&mut b, 0.6);
        // 1 / 1.5 = 0.6667: 64/255 -> 0.2510^0.6667 = 0.3979 -> 101
        assert_eq!(b[64 * 4], 101);
        assert_eq!(b[0], 0);
        assert_eq!(b[255 * 4], 255);
        assert!(b.chunks(4).zip(b.chunks(4).skip(1)).all(|(p, q)| q[0] >= p[0]));
        let mut c = src.clone();
        apply_brightness(&mut c, 0.1);
        // exponent 4: 128/255 -> 0.502^4 = 0.0635 -> 16
        assert_eq!(c[128 * 4], 16);
    }
}
