use anyhow::Result;
use ash::{vk, Device};

pub const MAX_FRAMES_IN_FLIGHT: usize = 3;

pub struct FrameSync {
    pub image_available: Vec<vk::Semaphore>,
    pub render_finished: Vec<vk::Semaphore>,
    pub in_flight: Vec<vk::Fence>,
}

impl FrameSync {
    pub fn new(device: &Device) -> Result<Self> {
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);

        let mut image_available = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut render_finished = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut in_flight = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe {
                image_available.push(device.create_semaphore(&semaphore_info, None)?);
                render_finished.push(device.create_semaphore(&semaphore_info, None)?);
                in_flight.push(device.create_fence(&fence_info, None)?);
            }
        }

        Ok(Self {
            image_available,
            render_finished,
            in_flight,
        })
    }

    pub fn cleanup(&self, device: &Device) {
        unsafe {
            for i in 0..MAX_FRAMES_IN_FLIGHT {
                device.destroy_semaphore(self.image_available[i], None);
                device.destroy_semaphore(self.render_finished[i], None);
                device.destroy_fence(self.in_flight[i], None);
            }
        }
    }
}
