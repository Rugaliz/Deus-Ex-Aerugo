//! Sampled RGBA8 textures with a mip chain, uploaded through a staging buffer.
use crate::{Context, Result};
use ash::vk;

pub struct GpuTexture {
    pub image: vk::Image,
    pub view: vk::ImageView,
    memory: vk::DeviceMemory,
}

impl GpuTexture {
    /// `mips`: `(width, height, rgba)` per level, largest first.
    pub fn new(cx: &Context, mips: &[(u32, u32, Vec<u8>)]) -> Result<GpuTexture> {
        let (w, h) = (mips[0].0, mips[0].1);
        let levels = mips.len() as u32;
        let mut staging_data: Vec<u8> = Vec::new();
        let mut regions = Vec::new();
        for (i, (mw, mh, px)) in mips.iter().enumerate() {
            regions.push(
                vk::BufferImageCopy::default()
                    .buffer_offset(staging_data.len() as u64)
                    .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).mip_level(i as u32).layer_count(1))
                    .image_extent(vk::Extent3D { width: *mw, height: *mh, depth: 1 }),
            );
            staging_data.extend_from_slice(px);
        }
        let staging = cx.upload(&staging_data, vk::BufferUsageFlags::TRANSFER_SRC)?;
        unsafe {
            let image = cx.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D { width: w, height: h, depth: 1 })
                    .mip_levels(levels)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            let req = cx.device.get_image_memory_requirements(image);
            let memory = cx.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(cx.memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?),
                None,
            )?;
            cx.device.bind_image_memory(image, memory, 0)?;
            let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(levels).layer_count(1);
            cx.one_shot(|cb| {
                let to_dst = [vk::ImageMemoryBarrier2::default()
                    .image(image)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .subresource_range(range)];
                cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_dst));
                cx.device.cmd_copy_buffer_to_image(cb, staging.buffer, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &regions);
                let to_read = [vk::ImageMemoryBarrier2::default()
                    .image(image)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                    .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                    .subresource_range(range)];
                cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_read));
            })?;
            cx.destroy_buffer(staging);
            let view = cx.device.create_image_view(
                &vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(vk::Format::R8G8B8A8_UNORM).subresource_range(range),
                None,
            )?;
            Ok(GpuTexture { image, view, memory })
        }
    }

    pub fn destroy(self, cx: &Context) {
        unsafe {
            cx.device.destroy_image_view(self.view, None);
            cx.device.destroy_image(self.image, None);
            cx.device.free_memory(self.memory, None);
        }
    }
}

/// Host memory for a texture whose pixels change every frame (procedural textures): written by the CPU, then copied
/// into level 0 of the texture by commands recorded into the same frame.
/// Uploads of rectangles into a texture (one level): pixels gathered in a host buffer, then one copy per rectangle.
pub struct RectStaging {
    /// One host buffer for each frame that may be in flight (`FRAMES_IN_FLIGHT`), used in turn: a frame's pixels go
    /// into the next one, so the frame before it can still be copying from its own.
    buffers: Vec<(crate::Buffer, crate::Mapped<u8>)>,
    slot: usize,
    capacity: usize,
    used: usize,
    rects: Vec<vk::BufferImageCopy>,
}

impl RectStaging {
    pub fn new(cx: &Context, capacity: usize) -> Result<RectStaging> {
        let mut buffers = Vec::new();
        for _ in 0..crate::FRAMES_IN_FLIGHT {
            let buffer = cx.create_buffer(capacity as u64, vk::BufferUsageFlags::TRANSFER_SRC, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
            let mapped = unsafe { cx.device.map_memory(buffer.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8 };
            buffers.push((buffer, crate::Mapped(mapped)));
        }
        Ok(RectStaging { buffers, slot: 0, capacity, used: 0, rects: Vec::new() })
    }

    /// Stages `rgba` (`w * h * 4` bytes) for texels `x..x+w`, `y..y+h`; false when the buffer is full for this frame.
    /// Call after the frame's slot was waited for (the frame `FRAMES_IN_FLIGHT` before is done).
    pub fn push(&mut self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) -> bool {
        // a frame's first rectangle: the next buffer
        if self.rects.is_empty() {
            self.slot = (self.slot + 1) % self.buffers.len();
            self.used = 0;
        }
        let n = (w * h * 4) as usize;
        // copy offsets must be multiples of the texel size
        let at = (self.used + 3) & !3;
        if rgba.len() != n || n == 0 || at + n > self.capacity {
            return false;
        }
        unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), self.buffers[self.slot].1.0.add(at), n) };
        self.used = at + n;
        self.rects.push(
            vk::BufferImageCopy::default()
                .buffer_offset(at as u64)
                .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                .image_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 })
                .image_extent(vk::Extent3D { width: w, height: h, depth: 1 }),
        );
        true
    }

    /// Records the staged copies into `texture`; outside a render pass. Clears the staging for the next frame.
    pub fn record_upload(&mut self, cx: &Context, cb: vk::CommandBuffer, texture: &GpuTexture) {
        if self.rects.is_empty() {
            return;
        }
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        unsafe {
            let to_dst = [vk::ImageMemoryBarrier2::default()
                .image(texture.image)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_dst));
            cx.device.cmd_copy_buffer_to_image(cb, self.buffers[self.slot].0.buffer, texture.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &self.rects);
            let to_read = [vk::ImageMemoryBarrier2::default()
                .image(texture.image)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_read));
        }
        self.rects.clear();
        self.used = 0;
    }

    pub fn destroy(self, cx: &Context) {
        for (b, _) in self.buffers {
            unsafe { cx.device.unmap_memory(b.memory) };
            cx.destroy_buffer(b);
        }
    }
}

pub struct Staging {
    /// One host buffer for each frame that may be in flight, used in turn as `RectStaging`'s.
    buffers: Vec<(crate::Buffer, crate::Mapped<u8>)>,
    slot: usize,
    width: u32,
    height: u32,
    dirty: bool,
}

impl Staging {
    pub fn new(cx: &Context, width: u32, height: u32) -> Result<Staging> {
        let size = (width * height * 4) as u64;
        let mut buffers = Vec::new();
        for _ in 0..crate::FRAMES_IN_FLIGHT {
            let buffer = cx.create_buffer(size, vk::BufferUsageFlags::TRANSFER_SRC, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
            let mapped = unsafe { cx.device.map_memory(buffer.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8 };
            buffers.push((buffer, crate::Mapped(mapped)));
        }
        Ok(Staging { buffers, slot: 0, width, height, dirty: false })
    }

    /// New pixels (`width * height * 4` bytes): the frame's first write goes into the next buffer. Call after the
    /// frame's slot was waited for.
    pub fn write(&mut self, rgba: &[u8]) {
        let n = (self.width * self.height * 4) as usize;
        if rgba.len() == n {
            if !self.dirty {
                self.slot = (self.slot + 1) % self.buffers.len();
            }
            unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), self.buffers[self.slot].1.0, n) };
            self.dirty = true;
        }
    }

    /// Records the copy into `texture` (made with one level of the same size) if new pixels were written. Outside a
    /// render pass.
    pub fn record_upload(&mut self, cx: &Context, cb: vk::CommandBuffer, texture: &GpuTexture) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        unsafe {
            let to_dst = [vk::ImageMemoryBarrier2::default()
                .image(texture.image)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_dst));
            let region = [vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 })];
            cx.device.cmd_copy_buffer_to_image(cb, self.buffers[self.slot].0.buffer, texture.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &region);
            let to_read = [vk::ImageMemoryBarrier2::default()
                .image(texture.image)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .subresource_range(range)];
            cx.device.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&to_read));
        }
    }

    pub fn destroy(self, cx: &Context) {
        for (b, _) in self.buffers {
            unsafe { cx.device.unmap_memory(b.memory) };
            cx.destroy_buffer(b);
        }
    }
}
