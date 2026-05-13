use anyhow::Result;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use gpu_allocator::MemoryLocation;
use std::sync::{Arc, Mutex};

pub struct GpuBuffer {
    pub buffer: vk::Buffer,
    pub allocation: Option<Allocation>,
    pub size: vk::DeviceSize,
}

impl GpuBuffer {
    pub fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
        name: &str,
    ) -> Result<Self> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);

        let buffer = unsafe { device.create_buffer(&buffer_info, None)? };
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };

        let allocation = allocator.lock().unwrap().allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;

        unsafe {
            device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset())?;
        }

        Ok(Self {
            buffer,
            allocation: Some(allocation),
            size,
        })
    }

    pub fn write<T: Copy>(&mut self, data: &[T]) {
        let allocation = self.allocation.as_mut().unwrap();
        let dst = allocation
            .mapped_slice_mut()
            .expect("Buffer not host-visible");
        let src = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data))
        };
        dst[..src.len()].copy_from_slice(src);
    }

    pub fn cleanup(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        if let Some(allocation) = self.allocation.take() {
            allocator.lock().unwrap().free(allocation).ok();
        }
        unsafe {
            device.destroy_buffer(self.buffer, None);
        }
    }
}

/// Create a device-local buffer by staging through a host-visible buffer.
pub fn create_device_local_buffer<T: Copy>(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    data: &[T],
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<GpuBuffer> {
    let size = (std::mem::size_of::<T>() * data.len()) as vk::DeviceSize;

    // Create staging buffer
    let mut staging = GpuBuffer::new(
        device,
        allocator,
        size,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryLocation::CpuToGpu,
        &format!("{}_staging", name),
    )?;
    staging.write(data);

    // Create device-local buffer
    let gpu_buffer = GpuBuffer::new(
        device,
        allocator,
        size,
        usage | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuOnly,
        name,
    )?;

    // Copy via command buffer
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);

    let cmd = unsafe { device.allocate_command_buffers(&alloc_info)?[0] };

    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe {
        device.begin_command_buffer(cmd, &begin_info)?;
        device.cmd_copy_buffer(
            cmd,
            staging.buffer,
            gpu_buffer.buffer,
            &[vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size,
            }],
        );
        device.end_command_buffer(cmd)?;
    }

    let submit_info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
    unsafe {
        device.queue_submit(queue, &[submit_info], vk::Fence::null())?;
        device.queue_wait_idle(queue)?;
        device.free_command_buffers(command_pool, &[cmd]);
    }

    staging.cleanup(device, allocator);

    Ok(gpu_buffer)
}

/// Create a host-visible staging buffer with data already written.
pub fn create_host_buffer<T: Copy>(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    data: &[T],
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<GpuBuffer> {
    let size = (std::mem::size_of::<T>() * data.len()) as vk::DeviceSize;
    let mut buffer = GpuBuffer::new(
        device,
        allocator,
        size,
        usage,
        MemoryLocation::CpuToGpu,
        name,
    )?;
    buffer.write(data);
    Ok(buffer)
}
