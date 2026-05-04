use anyhow::Result;
use ash::{vk, Device, Instance};
use std::ffi::CStr;

pub struct VulkanDevice {
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub graphics_queue: vk::Queue,
    pub present_queue: vk::Queue,
    pub graphics_queue_family: u32,
    pub present_queue_family: u32,
}

impl VulkanDevice {
    pub fn new(instance: &Instance, surface: vk::SurfaceKHR, surface_fn: &ash::khr::surface::Instance) -> Result<Self> {
        let (physical_device, graphics_family, present_family) =
            pick_physical_device(instance, surface, surface_fn)?;

        let unique_families: Vec<u32> = if graphics_family == present_family {
            vec![graphics_family]
        } else {
            vec![graphics_family, present_family]
        };

        let queue_priority = [1.0f32];
        let queue_create_infos: Vec<vk::DeviceQueueCreateInfo> = unique_families
            .iter()
            .map(|&family| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(family)
                    .queue_priorities(&queue_priority)
            })
            .collect();

        let device_extensions = [ash::khr::swapchain::NAME.as_ptr()];

        let features = vk::PhysicalDeviceFeatures::default()
            .sampler_anisotropy(true)
            .fill_mode_non_solid(true);

        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&device_extensions)
            .enabled_features(&features);

        let device = unsafe { instance.create_device(physical_device, &device_create_info, None)? };

        let graphics_queue = unsafe { device.get_device_queue(graphics_family, 0) };
        let present_queue = unsafe { device.get_device_queue(present_family, 0) };

        log::info!("Vulkan device created (graphics family: {}, present family: {})", graphics_family, present_family);

        Ok(Self {
            physical_device,
            device,
            graphics_queue,
            present_queue,
            graphics_queue_family: graphics_family,
            present_queue_family: present_family,
        })
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();
            self.device.destroy_device(None);
        }
    }
}

fn pick_physical_device(
    instance: &Instance,
    surface: vk::SurfaceKHR,
    surface_fn: &ash::khr::surface::Instance,
) -> Result<(vk::PhysicalDevice, u32, u32)> {
    let devices = unsafe { instance.enumerate_physical_devices()? };

    for device in &devices {
        let props = unsafe { instance.get_physical_device_properties(*device) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy();

        let queue_families =
            unsafe { instance.get_physical_device_queue_family_properties(*device) };

        let graphics_family = queue_families.iter().enumerate().find_map(|(i, qf)| {
            if qf.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                Some(i as u32)
            } else {
                None
            }
        });

        let present_family = queue_families.iter().enumerate().find_map(|(i, _)| {
            let supported = unsafe {
                surface_fn
                    .get_physical_device_surface_support(*device, i as u32, surface)
                    .unwrap_or(false)
            };
            if supported { Some(i as u32) } else { None }
        });

        if let (Some(gf), Some(pf)) = (graphics_family, present_family) {
            // Prefer discrete GPU
            if props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU {
                log::info!("Selected GPU: {} (discrete)", name);
                return Ok((*device, gf, pf));
            }
        }
    }

    // Fallback: any device with both queues
    for device in &devices {
        let queue_families =
            unsafe { instance.get_physical_device_queue_family_properties(*device) };

        let graphics_family = queue_families.iter().enumerate().find_map(|(i, qf)| {
            if qf.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                Some(i as u32)
            } else {
                None
            }
        });

        let present_family = queue_families.iter().enumerate().find_map(|(i, _)| {
            let supported = unsafe {
                surface_fn
                    .get_physical_device_surface_support(*device, i as u32, surface)
                    .unwrap_or(false)
            };
            if supported { Some(i as u32) } else { None }
        });

        if let (Some(gf), Some(pf)) = (graphics_family, present_family) {
            let props = unsafe { instance.get_physical_device_properties(*device) };
            let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy();
            log::info!("Selected GPU: {} (fallback)", name);
            return Ok((*device, gf, pf));
        }
    }

    anyhow::bail!("No suitable Vulkan GPU found")
}
