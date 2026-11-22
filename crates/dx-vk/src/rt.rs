//! Ray queries for the ray-traced picture options (`Quality::rt_shadows`, `Quality::rt_ao`): acceleration structures of
//! the world (built once per map), the movers (once each, placed every frame) and the meshes drawn (rebuilt every
//! frame), placed by a top-level structure every frame (`RtFrame`). The rays are traced by the screen-space pass of
//! `rt_post`.
//!
//! Instance masks: `MASK_WORLD` for the level's geometry (whose shadows are already in the lightmaps), `MASK_OBJECT` for
//! movers and meshes (whose shadows the original never drew).

use crate::{Buffer, Context, Result};
use ash::vk;

pub const MASK_WORLD: u8 = 1;
pub const MASK_OBJECT: u8 = 2;

/// The device's ray tracing functions.
pub struct RtSupport {
    pub accel: ash::khr::acceleration_structure::Device,
    scratch_align: u64,
}

impl RtSupport {
    pub fn new(instance: &ash::Instance, device: &ash::Device) -> Result<RtSupport> {
        Ok(RtSupport { accel: ash::khr::acceleration_structure::Device::new(instance, device), scratch_align: 256 })
    }
}

/// An acceleration structure and its storage.
pub struct Accel {
    pub handle: vk::AccelerationStructureKHR,
    buffer: Buffer,
    pub address: u64,
    /// What it was made to hold (primitives or instances), for reuse.
    capacity: u32,
}

impl Accel {
    fn new(cx: &Context, rt: &RtSupport, ty: vk::AccelerationStructureTypeKHR, size: u64, capacity: u32) -> Result<Accel> {
        let (buffer, _) = cx.create_buffer_addressed(size, vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
        unsafe {
            let handle = rt.accel.create_acceleration_structure(&vk::AccelerationStructureCreateInfoKHR::default().buffer(buffer.buffer).size(size).ty(ty), None)?;
            let address = rt.accel.get_acceleration_structure_device_address(&vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(handle));
            Ok(Accel { handle, buffer, address, capacity })
        }
    }

    pub fn destroy(self, cx: &Context) {
        if let Some(rt) = &cx.rt {
            unsafe { rt.accel.destroy_acceleration_structure(self.handle, None) };
        }
        cx.destroy_buffer(self.buffer);
    }
}

/// A geometry of triangles; `opaque`: hits count without asking (else the shader's ray query decides: alpha-tested faces).
fn triangles<'a>(vertices: u64, vertex_count: u32, indices: Option<u64>, opaque: bool) -> vk::AccelerationStructureGeometryKHR<'a> {
    let mut tri = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
        .vertex_format(vk::Format::R32G32B32_SFLOAT)
        .vertex_data(vk::DeviceOrHostAddressConstKHR { device_address: vertices })
        .vertex_stride(12)
        .max_vertex(vertex_count.saturating_sub(1));
    tri = match indices {
        Some(i) => tri.index_type(vk::IndexType::UINT32).index_data(vk::DeviceOrHostAddressConstKHR { device_address: i }),
        None => tri.index_type(vk::IndexType::NONE_KHR),
    };
    vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { triangles: tri })
        .flags(if opaque { vk::GeometryFlagsKHR::OPAQUE } else { vk::GeometryFlagsKHR::empty() })
}

/// A bottom-level structure of fixed triangles (the world, a mover's brush in its own space), built now. `None` without
/// ray queries or triangles.
pub fn static_blas(cx: &Context, positions: &[[f32; 3]], indices: &[u32]) -> Result<Option<Accel>> {
    let Some(rt) = &cx.rt else { return Ok(None) };
    if indices.len() < 3 || positions.is_empty() {
        return Ok(None);
    }
    let input = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR;
    let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    let (vbuf, vaddr) = cx.create_buffer_addressed(std::mem::size_of_val(positions) as u64, input, host)?;
    let (ibuf, iaddr) = cx.create_buffer_addressed(std::mem::size_of_val(indices) as u64, input, host)?;
    unsafe {
        let p = cx.device.map_memory(vbuf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut [f32; 3];
        std::ptr::copy_nonoverlapping(positions.as_ptr(), p, positions.len());
        cx.device.unmap_memory(vbuf.memory);
        let p = cx.device.map_memory(ibuf.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u32;
        std::ptr::copy_nonoverlapping(indices.as_ptr(), p, indices.len());
        cx.device.unmap_memory(ibuf.memory);
    }
    let count = (indices.len() / 3) as u32;
    let geometry = [triangles(vaddr, positions.len() as u32, Some(iaddr), true)];
    let mut info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
        .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(&geometry);
    let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    unsafe { rt.accel.get_acceleration_structure_build_sizes(vk::AccelerationStructureBuildTypeKHR::DEVICE, &info, &[count], &mut sizes) };
    let accel = Accel::new(cx, rt, vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL, sizes.acceleration_structure_size, count)?;
    let (scratch, saddr) = cx.create_buffer_addressed(sizes.build_scratch_size + rt.scratch_align, vk::BufferUsageFlags::STORAGE_BUFFER, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
    info = info.dst_acceleration_structure(accel.handle).scratch_data(vk::DeviceOrHostAddressKHR { device_address: align(saddr, rt.scratch_align) });
    let range = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
    let r = cx.one_shot(|cb| unsafe { rt.accel.cmd_build_acceleration_structures(cb, &[info], &[&range]) });
    cx.destroy_buffer(scratch);
    cx.destroy_buffer(vbuf);
    cx.destroy_buffer(ibuf);
    r?;
    Ok(Some(accel))
}

fn align(x: u64, a: u64) -> u64 {
    x.div_ceil(a) * a
}

/// A host-visible, addressed buffer that grows (made again, larger) when a frame needs more.
struct Growable {
    buffer: Option<(Buffer, u64, crate::Mapped<u8>)>,
    usage: vk::BufferUsageFlags,
}

impl Growable {
    fn new(usage: vk::BufferUsageFlags) -> Growable {
        Growable { buffer: None, usage }
    }

    /// At least `size` bytes; returns whether the buffer was made again (descriptors pointing at it must be rewritten).
    fn ensure(&mut self, cx: &Context, size: u64) -> Result<bool> {
        if self.buffer.as_ref().is_some_and(|(b, _, _)| b.size >= size) {
            return Ok(false);
        }
        if let Some((b, _, _)) = self.buffer.take() {
            cx.destroy_buffer(b);
        }
        let size = size.max(4096).next_power_of_two();
        let (b, addr) = cx.create_buffer_addressed(size, self.usage, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        let p = unsafe { cx.device.map_memory(b.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8 };
        self.buffer = Some((b, addr, crate::Mapped(p)));
        Ok(true)
    }

    fn write<T: Copy>(&mut self, cx: &Context, data: &[T]) -> Result<bool> {
        let made = self.ensure(cx, std::mem::size_of_val(data) as u64)?;
        let (_, _, p) = self.buffer.as_ref().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr() as *const u8, p.0, std::mem::size_of_val(data)) };
        Ok(made)
    }

    fn address(&self) -> u64 {
        self.buffer.as_ref().map(|b| b.1).unwrap_or(0)
    }

    fn destroy(&mut self, cx: &Context) {
        if let Some((b, _, _)) = self.buffer.take() {
            cx.destroy_buffer(b);
        }
    }
}

/// The per-frame acceleration structures.
pub struct RtFrame {
    mesh_verts: Growable,
    mesh_blas: Option<Accel>,
    instances: Growable,
    tlas: Option<Accel>,
    scratch: Option<(Buffer, u64)>,
}

impl RtFrame {
    pub fn new(cx: &Context) -> Option<RtFrame> {
        cx.rt.as_ref()?;
        let input = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR;
        Some(RtFrame { mesh_verts: Growable::new(input), mesh_blas: None, instances: Growable::new(input), tlas: None, scratch: None })
    }

    /// This frame's top-level structure (after `record`).
    pub fn tlas(&self) -> Option<vk::AccelerationStructureKHR> {
        self.tlas.as_ref().map(|a| a.handle)
    }

    fn ensure_scratch(&mut self, cx: &Context, rt: &RtSupport, size: u64) -> Result<u64> {
        let size = size + rt.scratch_align;
        if !self.scratch.as_ref().is_some_and(|(b, _)| b.size >= size) {
            if let Some((b, _)) = self.scratch.take() {
                cx.destroy_buffer(b);
            }
            self.scratch = Some(cx.create_buffer_addressed(size.next_power_of_two(), vk::BufferUsageFlags::STORAGE_BUFFER, vk::MemoryPropertyFlags::DEVICE_LOCAL)?);
        }
        Ok(align(self.scratch.as_ref().unwrap().1, rt.scratch_align))
    }

    /// Records this frame's builds into `cb` (before the scene's rendering starts): the meshes' structure from
    /// `mesh_triangles` (world space, three corners each), two geometries per mesh (`mesh_parts`: first corner and
    /// corner count of its opaque faces, then of its alpha-tested ones: geometry `2 m` and `2 m + 1`), the top-level
    /// structure over the world (`world`),
    /// the movers (`movers`: structure address and placement as a row-major 3x4) and the meshes. Returns whether the
    /// top-level structure was made again (descriptors pointing at it must be rewritten). The GPU must be done with
    /// the previous frame's data (one frame in flight).
    pub fn record(&mut self, cx: &Context, cb: vk::CommandBuffer, world: Option<u64>, movers: &[(u64, [[f32; 4]; 3])], mesh_triangles: &[[f32; 3]], mesh_parts: &[[u32; 4]]) -> Result<bool> {
        let rt = cx.rt.as_ref().expect("ray queries");
        let mut rewrite = false;
        // the meshes: one structure for every mesh drawn this frame (a geometry each), rebuilt
        let mut mesh_addr = None;
        if !mesh_parts.is_empty() {
            self.mesh_verts.write(cx, mesh_triangles)?;
            let base = self.mesh_verts.address();
            let geometry: Vec<vk::AccelerationStructureGeometryKHR> = mesh_parts
                .iter()
                .flat_map(|p| [triangles(base + p[0] as u64 * 12, p[1], None, true), triangles(base + p[2] as u64 * 12, p[3], None, false)])
                .collect();
            let counts: Vec<u32> = mesh_parts.iter().flat_map(|p| [p[1] / 3, p[3] / 3]).collect();
            let mut info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD)
                .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                .geometries(&geometry);
            let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
            unsafe { rt.accel.get_acceleration_structure_build_sizes(vk::AccelerationStructureBuildTypeKHR::DEVICE, &info, &counts, &mut sizes) };
            // `capacity`: the storage size in KiB
            let need = sizes.acceleration_structure_size.div_ceil(1024) as u32;
            if !self.mesh_blas.as_ref().is_some_and(|a| a.capacity >= need) {
                if let Some(a) = self.mesh_blas.take() {
                    a.destroy(cx);
                }
                let cap = (need + need / 2).max(64);
                self.mesh_blas = Some(Accel::new(cx, rt, vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL, cap as u64 * 1024, cap)?);
            }
            let scratch = self.ensure_scratch(cx, rt, sizes.build_scratch_size)?;
            let blas = self.mesh_blas.as_ref().unwrap();
            info = info.dst_acceleration_structure(blas.handle).scratch_data(vk::DeviceOrHostAddressKHR { device_address: scratch });
            let ranges: Vec<vk::AccelerationStructureBuildRangeInfoKHR> = counts.iter().map(|&c| vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(c)).collect();
            unsafe {
                rt.accel.cmd_build_acceleration_structures(cb, &[info], &[&ranges]);
                build_barrier(cx, cb, vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR);
            }
            mesh_addr = Some(blas.address);
        }
        // the instances
        let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let mut inst: Vec<vk::AccelerationStructureInstanceKHR> = Vec::new();
        let mut push = |addr: u64, m: [[f32; 4]; 3], mask: u8| {
            inst.push(vk::AccelerationStructureInstanceKHR {
                transform: vk::TransformMatrixKHR { matrix: [m[0][0], m[0][1], m[0][2], m[0][3], m[1][0], m[1][1], m[1][2], m[1][3], m[2][0], m[2][1], m[2][2], m[2][3]] },
                instance_custom_index_and_mask: vk::Packed24_8::new(0, mask),
                instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(0, vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE.as_raw() as u8),
                acceleration_structure_reference: vk::AccelerationStructureReferenceKHR { device_handle: addr },
            });
        };
        if let Some(w) = world {
            push(w, identity, MASK_WORLD);
        }
        for &(addr, m) in movers {
            push(addr, m, MASK_OBJECT);
        }
        if let Some(m) = mesh_addr {
            push(m, identity, MASK_OBJECT);
        }
        self.instances.write(cx, &inst)?;
        let count = inst.len() as u32;
        let geometry = [vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::INSTANCES)
            .geometry(vk::AccelerationStructureGeometryDataKHR {
                instances: vk::AccelerationStructureGeometryInstancesDataKHR::default().data(vk::DeviceOrHostAddressConstKHR { device_address: self.instances.address() }),
            })];
        let mut info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(&geometry);
        if !self.tlas.as_ref().is_some_and(|a| a.capacity >= count.max(1)) {
            let cap = count.max(16).next_power_of_two();
            let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
            unsafe { rt.accel.get_acceleration_structure_build_sizes(vk::AccelerationStructureBuildTypeKHR::DEVICE, &info, &[cap], &mut sizes) };
            if let Some(a) = self.tlas.take() {
                a.destroy(cx);
            }
            self.tlas = Some(Accel::new(cx, rt, vk::AccelerationStructureTypeKHR::TOP_LEVEL, sizes.acceleration_structure_size, cap)?);
            rewrite = true;
        }
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe { rt.accel.get_acceleration_structure_build_sizes(vk::AccelerationStructureBuildTypeKHR::DEVICE, &info, &[count], &mut sizes) };
        let scratch = self.ensure_scratch(cx, rt, sizes.build_scratch_size)?;
        info = info.dst_acceleration_structure(self.tlas.as_ref().unwrap().handle).scratch_data(vk::DeviceOrHostAddressKHR { device_address: scratch });
        let range = [vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(count)];
        unsafe {
            rt.accel.cmd_build_acceleration_structures(cb, &[info], &[&range]);
            build_barrier(cx, cb, vk::PipelineStageFlags2::FRAGMENT_SHADER);
        }
        // the lights and the surfaces' lists: a table of each level surface's list start (0: none), then the lists
        // (count, then light indices)
        Ok(rewrite)
    }

    pub fn destroy(mut self, cx: &Context) {
        if let Some(a) = self.mesh_blas.take() {
            a.destroy(cx);
        }
        if let Some(a) = self.tlas.take() {
            a.destroy(cx);
        }
        if let Some((b, _)) = self.scratch.take() {
            cx.destroy_buffer(b);
        }
        self.mesh_verts.destroy(cx);
        self.instances.destroy(cx);
    }
}

/// A build's writes made visible to what follows (`dst`: the next build, or the fragment shaders' ray queries).
unsafe fn build_barrier(cx: &Context, cb: vk::CommandBuffer, dst: vk::PipelineStageFlags2) {
    let barrier = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
        .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
        .dst_stage_mask(dst | vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
        .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR | vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)];
    cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().memory_barriers(&barrier));
}

/// A placement as an acceleration structure instance's row-major 3x4 transform.
pub fn instance_transform(p: &crate::world::Placement) -> [[f32; 4]; 3] {
    std::array::from_fn(|r| [p.cols[0][r], p.cols[1][r], p.cols[2][r], p.origin[r]])
}
