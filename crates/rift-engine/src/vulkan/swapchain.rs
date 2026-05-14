use anyhow::Result;
use ash::{vk, Device};

fn choose_present_mode(
    available: &[vk::PresentModeKHR],
    vsync_enabled: bool,
) -> vk::PresentModeKHR {
    if vsync_enabled {
        return vk::PresentModeKHR::FIFO;
    }

    [vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::IMMEDIATE]
        .into_iter()
        .find(|mode| available.contains(mode))
        .unwrap_or(vk::PresentModeKHR::FIFO)
}

pub struct Swapchain {
    pub swapchain_fn: ash::khr::swapchain::Device,
    pub swapchain: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    pub image_views: Vec<vk::ImageView>,
    pub format: vk::SurfaceFormatKHR,
    pub extent: vk::Extent2D,
}

impl Swapchain {
    pub fn new(
        instance: &ash::Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        surface: vk::SurfaceKHR,
        surface_fn: &ash::khr::surface::Instance,
        graphics_family: u32,
        present_family: u32,
        window_size: [u32; 2],
        vsync_enabled: bool,
    ) -> Result<Self> {
        let capabilities = unsafe {
            surface_fn.get_physical_device_surface_capabilities(physical_device, surface)?
        };
        let formats =
            unsafe { surface_fn.get_physical_device_surface_formats(physical_device, surface)? };
        let present_modes = unsafe {
            surface_fn.get_physical_device_surface_present_modes(physical_device, surface)?
        };

        let format = formats
            .iter()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_SRGB
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(&formats[0])
            .clone();

        let present_mode = choose_present_mode(&present_modes, vsync_enabled);

        let extent = if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            vk::Extent2D {
                width: window_size[0].clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: window_size[1].clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        };

        let image_count =
            (capabilities.min_image_count + 1).min(if capabilities.max_image_count > 0 {
                capabilities.max_image_count
            } else {
                u32::MAX
            });

        let (sharing_mode, queue_family_indices) = if graphics_family != present_family {
            (
                vk::SharingMode::CONCURRENT,
                vec![graphics_family, present_family],
            )
        } else {
            (vk::SharingMode::EXCLUSIVE, vec![])
        };

        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(sharing_mode)
            .queue_family_indices(&queue_family_indices)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true);

        let swapchain_fn = ash::khr::swapchain::Device::new(instance, device);
        let swapchain = unsafe { swapchain_fn.create_swapchain(&create_info, None)? };
        let images = unsafe { swapchain_fn.get_swapchain_images(swapchain)? };

        let image_views: Vec<vk::ImageView> = images
            .iter()
            .map(|&image| {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format.format)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    });
                unsafe { device.create_image_view(&view_info, None) }
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;

        log::info!(
            "Swapchain created: {}x{}, {} images, {:?} (available: {:?})",
            extent.width,
            extent.height,
            images.len(),
            present_mode,
            present_modes
        );

        Ok(Self {
            swapchain_fn,
            swapchain,
            images,
            image_views,
            format,
            extent,
        })
    }

    pub fn cleanup(&mut self, device: &Device) {
        unsafe {
            for &view in &self.image_views {
                device.destroy_image_view(view, None);
            }
            self.swapchain_fn.destroy_swapchain(self.swapchain, None);
        }
    }
}
