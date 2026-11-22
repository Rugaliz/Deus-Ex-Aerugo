//! Vulkan renderer. Step 1: instance + device (Vulkan 1.3 dynamic rendering), an offscreen colour+depth target and
//! readback to RGBA8, so frames can be checked headlessly (and compared with the CPU renderer `dx-soft`).

/// D3DDrv's texture stage setup (10009e20, asm) with the shipped `UseMipmapping=True`, `UseTrilinear=False`: magnify
/// and minify LINEAR (POINT for `PF_NoSmooth`), mip levels POINT (the nearest level, no blend between levels) and a mip
/// LOD bias of -0.5 on every stage (`LODBIAS` console command only).
pub const MIP_MODE: ash::vk::SamplerMipmapMode = ash::vk::SamplerMipmapMode::NEAREST;
pub const MIP_LOD_BIAS: f32 = -0.5;

/// A pointer into host-visible GPU memory that stays mapped while its buffer lives. Only the buffer's owner writes
/// through it (one thread at a time), so the owner can be handed to another thread with it (RENDER_THREAD.md).
#[derive(Debug, Clone, Copy)]
pub struct Mapped<T>(pub *mut T);

// SAFETY: see above: the mapping outlives every use, and the pointer is only used by whoever owns the buffer
unsafe impl<T> Send for Mapped<T> {}

pub mod texture;
pub mod rt;
pub mod rt_post;
pub mod world;
pub mod actors;
pub mod lodmesh;
pub mod renderer;
pub mod render_thread;
pub mod present;
pub mod ui;

pub use ash;
use ash::vk;
use std::ffi::{c_char, CStr};

/// The scene target's format as the original draws (8 bits per channel); `Quality::deep_color` uses `DEEP_COLOR_FORMAT`.
pub const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// 16 bits per channel, still clamped to 0..1 like the original's frame buffer (blends saturate the same way).
pub const DEEP_COLOR_FORMAT: vk::Format = vk::Format::R16G16B16A16_UNORM;

/// Picture options over the original's look (the port's settings menu). `Quality::ORIGINAL` draws as Deus Ex does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// Texture filtering: 0 the original's (bilinear, nearest mip level, LOD bias -0.5), 1 trilinear, 2..=5
    /// trilinear with 2x, 4x, 8x, 16x anisotropy.
    pub filter: u8,
    /// Multisampling: 1 (off), 2, 4 or 8 samples, with alpha-to-coverage for masked textures.
    pub msaa: u32,
    /// 16 bits per channel in the scene, dithered when shown: no banding in fog and dark gradients.
    pub deep_color: bool,
    /// Lightmaps sampled bicubic (smooth shadow edges) instead of bilinear.
    pub smooth_lightmaps: bool,
    /// Meshes lit per pixel from the actor's light list instead of per vertex.
    pub pixel_lighting: bool,
    /// Ray-traced shadows of characters, objects and doors from the map's lights (on the world and on each other).
    pub rt_shadows: bool,
    /// Ray-traced contact shadows (ambient occlusion) where surfaces and objects meet.
    pub rt_ao: bool,
}

/// Specialization constants of the scene's fragment shaders: 0 `A2C` (masked texels by alpha-to-coverage: the opaque
/// pipelines with multisampling), 1 `DEEP` (16-bit scene: no 8-bit frame-buffer rounding), 2 `SMOOTH_LM` (bicubic
/// lightmaps), 3 `PIXEL_LIGHT` (meshes lit per pixel). A shader declares the ones it uses.
pub const SCENE_SPEC: [vk::SpecializationMapEntry; 4] = [
    vk::SpecializationMapEntry { constant_id: 0, offset: 0, size: 4 },
    vk::SpecializationMapEntry { constant_id: 1, offset: 4, size: 4 },
    vk::SpecializationMapEntry { constant_id: 2, offset: 8, size: 4 },
    vk::SpecializationMapEntry { constant_id: 3, offset: 12, size: 4 },
];

/// The specialization data for `SCENE_SPEC` (`map`) from the picture options; `a2c` for an opaque pipeline.
pub fn scene_spec_data(q: Quality, a2c: bool) -> [u8; 16] {
    let mut d = [0u8; 16];
    for (i, on) in [a2c && q.msaa > 1, q.deep_color, q.smooth_lightmaps, q.pixel_lighting].into_iter().enumerate() {
        d[i * 4..i * 4 + 4].copy_from_slice(&(on as u32).to_ne_bytes());
    }
    d
}

impl Quality {
    /// Whether anything is ray traced (the scene then has acceleration structures and the ray-traced shader variants).
    pub fn ray_traced(&self) -> bool {
        self.rt_shadows || self.rt_ao
    }

    pub const ORIGINAL: Quality = Quality { filter: 0, msaa: 1, deep_color: false, smooth_lightmaps: false, pixel_lighting: false, rt_shadows: false, rt_ao: false };
}

impl Default for Quality {
    fn default() -> Quality {
        Quality::ORIGINAL
    }
}
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
    /// Draw only where the stencil does not hold this value.
    Outside(u32),
}

pub fn set_stencil(cx: &Context, cb: vk::CommandBuffer, mode: Stencil) {
    let face = vk::StencilFaceFlags::FRONT_AND_BACK;
    let (enable, pass, compare, reference) = match mode {
        Stencil::Off => (false, vk::StencilOp::KEEP, vk::CompareOp::ALWAYS, 0),
        Stencil::Mark(r) => (true, vk::StencilOp::REPLACE, vk::CompareOp::ALWAYS, r),
        Stencil::Inside(r) => (true, vk::StencilOp::KEEP, vk::CompareOp::EQUAL, r),
        Stencil::Incr(r) => (true, vk::StencilOp::INCREMENT_AND_CLAMP, vk::CompareOp::EQUAL, r),
        Stencil::Decr(r) => (true, vk::StencilOp::DECREMENT_AND_CLAMP, vk::CompareOp::EQUAL, r),
        Stencil::Outside(r) => (true, vk::StencilOp::KEEP, vk::CompareOp::NOT_EQUAL, r),
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
    /// The picture options in effect (`set_quality`), already limited to what the device can do.
    quality: std::cell::Cell<Quality>,
    /// The device's largest anisotropy (1 without the feature), the sample counts its colour and depth targets both
    /// take, and whether `DEEP_COLOR_FORMAT` can be a blended colour attachment.
    max_anisotropy: f32,
    sample_counts: vk::SampleCountFlags,
    deep_color_ok: bool,
    /// Ray queries and acceleration structures (`VK_KHR_ray_query`), when the device has them.
    pub rt: Option<rt::RtSupport>,
    /// Which of the `FRAMES_IN_FLIGHT` sets of per-frame data the frame being recorded writes (`set_frame_slot`).
    frame_slot: std::cell::Cell<usize>,
    /// Buffers given up while frames may still read them (`retire_buffer`), by the frame slot that gave them up.
    retired: std::cell::RefCell<[Vec<Buffer>; FRAMES_IN_FLIGHT]>,
}

/// Frames the GPU may still be drawing while the next is recorded: data the CPU writes for a frame (mesh triangles,
/// lighting, staged texture pixels, ray tracing input) comes in this many sets, one per frame slot.
pub const FRAMES_IN_FLIGHT: usize = 2;

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

/// Writes the chosen device, its driver and its memory heaps and types to stderr (the log file on Windows), for
/// reports from machines we cannot test on.
unsafe fn log_device(instance: &ash::Instance, physical: vk::PhysicalDevice, props: &vk::PhysicalDeviceProperties, mem: &vk::PhysicalDeviceMemoryProperties) {
    let mut driver = vk::PhysicalDeviceDriverProperties::default();
    let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut driver);
    instance.get_physical_device_properties2(physical, &mut p2);
    let text = |c: std::result::Result<&CStr, std::ffi::FromBytesUntilNulError>| c.map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
    let v = props.api_version;
    eprintln!(
        "[vulkan] {} ({:?}), {} {} ({:?}), Vulkan {}.{}.{}, driver version {:#x}",
        text(props.device_name_as_c_str()),
        props.device_type,
        text(driver.driver_name_as_c_str()),
        text(driver.driver_info_as_c_str()),
        driver.driver_id,
        vk::api_version_major(v),
        vk::api_version_minor(v),
        vk::api_version_patch(v),
        props.driver_version
    );
    for (i, h) in mem.memory_heaps[..mem.memory_heap_count as usize].iter().enumerate() {
        eprintln!("[vulkan] heap {i}: {} MB {:?}", h.size >> 20, h.flags);
    }
    for (i, t) in mem.memory_types[..mem.memory_type_count as usize].iter().enumerate() {
        eprintln!("[vulkan] memory type {i}: heap {} {:?}", t.heap_index, t.property_flags);
    }
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
            // anisotropic filtering, multisampling and the 16-bit target when the device has them (picture options)
            let feats = instance.get_physical_device_features(physical);
            let max_anisotropy = if feats.sampler_anisotropy == vk::TRUE { props.limits.max_sampler_anisotropy } else { 1.0 };
            let sample_counts = props.limits.framebuffer_color_sample_counts & props.limits.framebuffer_depth_sample_counts & props.limits.framebuffer_stencil_sample_counts;
            let deep = instance.get_physical_device_format_properties(physical, DEEP_COLOR_FORMAT).optimal_tiling_features;
            let deep_color_ok = deep.contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR | vk::FormatFeatureFlags::TRANSFER_SRC);
            let enabled_features = vk::PhysicalDeviceFeatures::default().sampler_anisotropy(feats.sampler_anisotropy == vk::TRUE);
            // ray queries (the ray-traced shadow and ambient occlusion options): the extensions and their features
            let rt_exts = [ash::khr::acceleration_structure::NAME, ash::khr::ray_query::NAME, ash::khr::deferred_host_operations::NAME];
            let rt_ok = {
                let available = instance.enumerate_device_extension_properties(physical)?;
                let has = |name: &CStr| available.iter().any(|e| e.extension_name_as_c_str().map(|n| n == name).unwrap_or(false));
                let mut asf = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
                let mut rqf = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
                let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
                let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut asf).push_next(&mut rqf).push_next(&mut v12);
                instance.get_physical_device_features2(physical, &mut f2);
                rt_exts.iter().all(|n| has(n)) && asf.acceleration_structure == vk::TRUE && rqf.ray_query == vk::TRUE && v12.buffer_device_address == vk::TRUE
            } && std::env::var_os("DX_NO_RT").is_none();
            let mut f12 = vk::PhysicalDeviceVulkan12Features::default().buffer_device_address(rt_ok);
            let mut f_as = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default().acceleration_structure(true);
            let mut f_rq = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
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
            if rt_ok {
                dev_exts.extend(rt_exts.iter().map(|n| n.as_ptr()));
            }
            let mut dev_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_info)
                .enabled_extension_names(&dev_exts)
                .enabled_features(&enabled_features)
                .push_next(&mut f13)
                .push_next(&mut f12);
            if rt_ok {
                dev_info = dev_info.push_next(&mut f_as).push_next(&mut f_rq);
            }
            let device = instance.create_device(physical, &dev_info, None)?;
            let queue = device.get_device_queue(queue_family, 0);
            let pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let mem_props = instance.get_physical_device_memory_properties(physical);
            log_device(&instance, physical, &props, &mem_props);
            let timestamp_period = props.limits.timestamp_period;
            let rt = if rt_ok { Some(rt::RtSupport::new(&instance, &device)?) } else { None };
            Ok(Context {
                entry,
                instance,
                physical,
                device,
                queue,
                queue_family,
                pool,
                mem_props,
                debug,
                device_name,
                memory_budget,
                timestamp_period,
                quality: std::cell::Cell::new(Quality::ORIGINAL),
                max_anisotropy,
                sample_counts,
                deep_color_ok,
                rt,
                frame_slot: std::cell::Cell::new(0),
                retired: Default::default(),
            })
        }
    }

    /// The picture options in effect.
    pub fn quality(&self) -> Quality {
        self.quality.get()
    }

    /// The frame slot being recorded (0..`FRAMES_IN_FLIGHT`).
    pub fn frame_slot(&self) -> usize {
        self.frame_slot.get()
    }

    /// Starts recording a frame into slot `slot`: the caller has waited for the frame that last used it (and so for
    /// every frame before: the buffers that frame's slot gave up are freed now).
    pub fn set_frame_slot(&self, slot: usize) {
        let slot = slot % FRAMES_IN_FLIGHT;
        self.frame_slot.set(slot);
        for b in std::mem::take(&mut self.retired.borrow_mut()[slot]) {
            self.destroy_buffer(b);
        }
    }

    /// Frees `b` once no frame in flight can read it any more (when this frame slot comes round again).
    pub fn retire_buffer(&self, b: Buffer) {
        self.retired.borrow_mut()[self.frame_slot.get()].push(b);
    }

    /// Frees every retired buffer: the GPU is idle (after a frame drawn and waited for, with none in flight).
    pub fn free_retired(&self) {
        let all: Vec<Buffer> = self.retired.borrow_mut().iter_mut().flat_map(std::mem::take).collect();
        for b in all {
            self.destroy_buffer(b);
        }
    }

    /// Sets the picture options, limited to what the device can do; returns what is in effect. Samplers, pipelines and
    /// targets read them when they are made, so the renderer remakes them after a change (`Renderer::set_quality`).
    pub fn set_quality(&self, mut q: Quality) -> Quality {
        q.msaa = [8u32, 4, 2, 1].into_iter().find(|&n| n <= q.msaa.max(1) && self.sample_counts.contains(vk::SampleCountFlags::from_raw(n))).unwrap_or(1);
        q.deep_color &= self.deep_color_ok;
        q.rt_shadows &= self.rt.is_some();
        q.rt_ao &= self.rt.is_some();
        q.filter = q.filter.min(5);
        self.quality.set(q);
        q
    }

    /// The scene target's colour format.
    pub fn color_format(&self) -> vk::Format {
        if self.quality.get().deep_color {
            DEEP_COLOR_FORMAT
        } else {
            COLOR_FORMAT
        }
    }

    /// The scene's samples per pixel.
    pub fn samples(&self) -> vk::SampleCountFlags {
        vk::SampleCountFlags::from_raw(self.quality.get().msaa.max(1))
    }

    /// The sampler of textures drawn on the world and meshes: the original's (`MIP_MODE`, `MIP_LOD_BIAS`) or trilinear
    /// with the chosen anisotropy. `nearest`: point sampling (`PF_NoSmooth`) for magnify and minify.
    pub fn texture_sampler_info(&self, nearest: bool) -> vk::SamplerCreateInfo<'static> {
        let q = self.quality.get();
        let filter = if nearest { vk::Filter::NEAREST } else { vk::Filter::LINEAR };
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(filter)
            .min_filter(filter)
            .address_mode_u(vk::SamplerAddressMode::REPEAT)
            .address_mode_v(vk::SamplerAddressMode::REPEAT)
            .address_mode_w(vk::SamplerAddressMode::REPEAT)
            .max_lod(vk::LOD_CLAMP_NONE);
        if q.filter == 0 {
            return info.mipmap_mode(MIP_MODE).mip_lod_bias(MIP_LOD_BIAS);
        }
        let aniso = if q.filter >= 2 { (1u32 << (q.filter - 1)) as f32 } else { 1.0 }.min(self.max_anisotropy);
        info.mipmap_mode(vk::SamplerMipmapMode::LINEAR).anisotropy_enable(aniso > 1.0 && !nearest).max_anisotropy(aniso.max(1.0))
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

    /// A buffer whose device address shaders and acceleration structure builds use (memory allocated with
    /// `DEVICE_ADDRESS`; `SHADER_DEVICE_ADDRESS` added to `usage`).
    pub fn create_buffer_addressed(&self, size: u64, usage: vk::BufferUsageFlags, props: vk::MemoryPropertyFlags) -> Result<(Buffer, u64)> {
        unsafe {
            let buffer = self.device.create_buffer(
                &vk::BufferCreateInfo::default().size(size.max(16)).usage(usage | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS).sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = self.device.get_buffer_memory_requirements(buffer);
            let mut flags = vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
            let memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(self.memory_type(req.memory_type_bits, props)?).push_next(&mut flags),
                None,
            )?;
            self.device.bind_buffer_memory(buffer, memory, 0)?;
            let address = self.device.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buffer));
            Ok((Buffer { buffer, memory, size: size.max(16) }, address))
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
            self.free_retired();
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
    /// The picture: what is shown, read back and copied (with multisampling, the samples resolved into it).
    pub color: vk::Image,
    pub color_view: vk::ImageView,
    color_mem: vk::DeviceMemory,
    pub depth: vk::Image,
    pub depth_view: vk::ImageView,
    depth_mem: vk::DeviceMemory,
    /// With multisampling: the multisampled colour image drawn into (resolved into `color` when rendering ends).
    msaa: Option<(vk::Image, vk::ImageView, vk::DeviceMemory)>,
    /// The colour format (`Context::color_format`) and its bytes per pixel.
    pub format: vk::Format,
    /// With ray tracing: the scene can stop and go on (`pause`, `resume`; its attachments are kept), and its depth is
    /// read in between (`depth_read`: the depth image itself, or with multisampling its first samples resolved into
    /// `depth_resolve`).
    keep: bool,
    depth_resolve: Option<(vk::Image, vk::ImageView, vk::DeviceMemory)>,
    pub depth_read: Option<vk::ImageView>,
    /// The stencil of the same image, as shaders read it.
    pub stencil_read: Option<vk::ImageView>,
}

impl Offscreen {
    pub fn new(cx: &Context, width: u32, height: u32) -> Result<Offscreen> {
        let samples = cx.samples();
        let make = |format, usage, aspect, samples| -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
            unsafe {
                let img = cx.device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D { width, height, depth: 1 })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(samples)
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
        let format = cx.color_format();
        let (color, color_mem, color_view) = make(
            format,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED,
            vk::ImageAspectFlags::COLOR,
            vk::SampleCountFlags::TYPE_1,
        )?;
        let keep = cx.quality().ray_traced() && cx.rt.is_some();
        let multi = samples != vk::SampleCountFlags::TYPE_1;
        let depth_usage = if keep && !multi { vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED } else { vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT };
        let (depth, depth_mem, depth_view) = make(DEPTH_FORMAT, depth_usage, DEPTH_ASPECTS, samples)?;
        let msaa = if multi {
            // kept between `pause` and `resume` with ray tracing (stored), else only resolved
            let usage = if keep { vk::ImageUsageFlags::COLOR_ATTACHMENT } else { vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT };
            let (img, mem, view) = make(format, usage, vk::ImageAspectFlags::COLOR, samples)?;
            Some((img, view, mem))
        } else {
            None
        };
        let depth_resolve = if keep && multi {
            let (img, mem, view) = make(DEPTH_FORMAT, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED, DEPTH_ASPECTS, vk::SampleCountFlags::TYPE_1)?;
            Some((img, view, mem))
        } else {
            None
        };
        // the depth and the stencil as shaders read them (one aspect a view)
        let read_view = |aspect| -> Result<Option<vk::ImageView>> {
            if !keep {
                return Ok(None);
            }
            let img = depth_resolve.as_ref().map(|d| d.0).unwrap_or(depth);
            Ok(Some(unsafe {
                cx.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(img)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(DEPTH_FORMAT)
                        .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1)),
                    None,
                )?
            }))
        };
        let depth_read = read_view(vk::ImageAspectFlags::DEPTH)?;
        let stencil_read = read_view(vk::ImageAspectFlags::STENCIL)?;
        Ok(Offscreen { width, height, color, color_view, color_mem, depth, depth_view, depth_mem, msaa, format, keep, depth_resolve, depth_read, stencil_read })
    }

    /// The image whose depth `depth_read` shows.
    fn depth_read_image(&self) -> vk::Image {
        self.depth_resolve.as_ref().map(|d| d.0).unwrap_or(self.depth)
    }

    /// Bytes per pixel of the colour image.
    fn pixel_bytes(&self) -> usize {
        if self.format == DEEP_COLOR_FORMAT {
            8
        } else {
            4
        }
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
                // after whatever earlier frames still do with it (the window's pass reads the colour)
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(dst_stage)
                .dst_access_mask(dst_access)
                .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1))
        };
        let depth_stages = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
        let mut barriers = vec![
            barrier(self.color, vk::ImageAspectFlags::COLOR, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE),
            barrier(self.depth, DEPTH_ASPECTS, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL, depth_stages, vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE),
        ];
        if let Some((img, _, _)) = self.msaa {
            barriers.push(barrier(img, vk::ImageAspectFlags::COLOR, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE));
        }
        if let Some((img, _, _)) = self.depth_resolve {
            // (resolves write at the colour output stage)
            barriers.push(barrier(
                img,
                DEPTH_ASPECTS,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                depth_stages | vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ));
        }
        unsafe { cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&barriers)) };
        self.start(cx, cb, Some(clear));
    }

    /// Starts dynamic rendering into the attachments (in their attachment layouts): clearing them, or going on with what
    /// they hold (`None`, after `pause`: the frame's last part, whose samples and depth are not kept and whose depth is
    /// not resolved again).
    fn start(&self, cx: &Context, cb: vk::CommandBuffer, clear: Option<[f32; 4]>) {
        let load = if clear.is_some() { vk::AttachmentLoadOp::CLEAR } else { vk::AttachmentLoadOp::LOAD };
        let keep = self.keep && clear.is_some();
        // the part after a pause starts with depth and stencil cleared (what follows the screen passes: the coronas,
        // which test no depth, then a depth clear before the overlays); with multisampling, the samples' depth is
        // never kept (its first samples are resolved for the screen passes)
        let depth_load = if self.keep && clear.is_none() { vk::AttachmentLoadOp::CLEAR } else { load };
        let depth_keep = keep && self.depth_resolve.is_none();
        let mut color_att = vk::RenderingAttachmentInfo::default()
            .image_view(self.color_view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(load)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue { color: vk::ClearColorValue { float32: clear.unwrap_or([0.0; 4]) } });
        // multisampled: drawn into the samples, averaged into the picture when rendering ends (the samples kept when
        // the scene goes on after a pause)
        if let Some((_, view, _)) = self.msaa {
            color_att = color_att
                .image_view(view)
                .store_op(if keep { vk::AttachmentStoreOp::STORE } else { vk::AttachmentStoreOp::DONT_CARE })
                .resolve_mode(vk::ResolveModeFlags::AVERAGE)
                .resolve_image_view(self.color_view)
                .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        }
        let color_att = [color_att];
        let mut depth_att = vk::RenderingAttachmentInfo::default()
            .image_view(self.depth_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .load_op(depth_load)
            .store_op(if depth_keep { vk::AttachmentStoreOp::STORE } else { vk::AttachmentStoreOp::DONT_CARE })
            .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } });
        // with multisampling and ray tracing: the first samples of depth and stencil resolved for the screen passes
        if let Some((_, view, _)) = self.depth_resolve.filter(|_| clear.is_some()) {
            depth_att = depth_att.resolve_mode(vk::ResolveModeFlags::SAMPLE_ZERO).resolve_image_view(view).resolve_image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        }
        unsafe {
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

    /// Stops the scene's rendering (ray tracing only) and makes its depth readable by the screen passes (`depth_read`).
    pub fn pause(&self, cx: &Context, cb: vk::CommandBuffer) {
        let range = vk::ImageSubresourceRange::default().aspect_mask(DEPTH_ASPECTS).level_count(1).layer_count(1);
        let barrier = [vk::ImageMemoryBarrier2::default()
            .image(self.depth_read_image())
            .old_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .src_stage_mask(vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS | vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .subresource_range(range)];
        unsafe {
            cx.device.cmd_end_rendering(cb);
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&barrier));
        }
    }

    /// Goes on with the scene after `pause`: the picture is kept, depth and stencil start cleared.
    pub fn resume(&self, cx: &Context, cb: vk::CommandBuffer) {
        let range = vk::ImageSubresourceRange::default().aspect_mask(DEPTH_ASPECTS).level_count(1).layer_count(1);
        let crange = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let color_dep = |image| {
            vk::ImageMemoryBarrier2::default()
                .image(image)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .subresource_range(crange)
        };
        // (a resolved copy of the depth stays readable: it is not an attachment again this frame)
        let mut barriers = vec![color_dep(self.color)];
        if self.depth_resolve.is_none() {
            barriers.push(vk::ImageMemoryBarrier2::default()
                .image(self.depth_read_image())
                .old_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS)
                .dst_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .subresource_range(range));
        }
        if let Some((img, _, _)) = self.msaa {
            barriers.push(color_dep(img));
        }
        if self.depth_resolve.is_some() {
            // the multisampled depth itself stayed an attachment
            barriers.push(
                vk::ImageMemoryBarrier2::default()
                    .image(self.depth)
                    .old_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                    .src_stage_mask(vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS)
                    .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS)
                    .dst_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                    .subresource_range(range),
            );
        }
        unsafe { cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&barriers)) };
        self.start(cx, cb, None);
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
            self.width as u64 * self.height as u64 * self.pixel_bytes() as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
    }

    /// The read-back picture as RGBA8 (a 16-bit one rounded to 8 bits).
    pub fn read_pixels(&self, cx: &Context, buf: &Buffer) -> Result<Vec<u8>> {
        let n = (self.width * self.height) as usize * self.pixel_bytes();
        let mut raw = vec![0u8; n];
        unsafe {
            let p = cx.device.map_memory(buf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *const u8;
            std::ptr::copy_nonoverlapping(p, raw.as_mut_ptr(), n);
            cx.device.unmap_memory(buf.memory);
        }
        if self.pixel_bytes() == 8 {
            return Ok(raw.chunks_exact(2).map(|c| ((u16::from_le_bytes([c[0], c[1]]) as u32 * 255 + 32767) / 65535) as u8).collect());
        }
        Ok(raw)
    }

    pub fn destroy(self, cx: &Context) {
        unsafe {
            cx.device.destroy_image_view(self.color_view, None);
            cx.device.destroy_image(self.color, None);
            cx.device.free_memory(self.color_mem, None);
            cx.device.destroy_image_view(self.depth_view, None);
            cx.device.destroy_image(self.depth, None);
            cx.device.free_memory(self.depth_mem, None);
            for (img, view, mem) in [self.msaa, self.depth_resolve].into_iter().flatten() {
                cx.device.destroy_image_view(view, None);
                cx.device.destroy_image(img, None);
                cx.device.free_memory(mem, None);
            }
            for v in [self.depth_read, self.stencil_read].into_iter().flatten() {
                cx.device.destroy_image_view(v, None);
            }
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
            let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(cx.samples());
            let ds = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(true).depth_compare_op(vk::CompareOp::LESS);
            let blend_att = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_att);
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
