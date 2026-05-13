use anyhow::Result;
use ash::{vk, Entry, Instance};
use raw_window_handle::HasDisplayHandle;
use std::ffi::{CStr, CString};

pub struct VulkanInstance {
    pub entry: Entry,
    pub instance: Instance,
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
    debug_utils: Option<ash::ext::debug_utils::Instance>,
}

impl VulkanInstance {
    pub fn new(window: &winit::window::Window) -> Result<Self> {
        let entry = unsafe { Entry::load()? };

        let app_name = CString::new("Rift Engine")?;
        let engine_name = CString::new("Rift")?;

        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(&engine_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_3);

        let mut extension_names =
            ash_window::enumerate_required_extensions(window.display_handle()?.as_raw())?.to_vec();

        let enable_validation = cfg!(debug_assertions) && Self::validation_layers_available(&entry);
        let layer_names: Vec<CString> = if enable_validation {
            extension_names.push(ash::ext::debug_utils::NAME.as_ptr());
            vec![CString::new("VK_LAYER_KHRONOS_validation")?]
        } else {
            vec![]
        };
        let layer_ptrs: Vec<*const i8> = layer_names.iter().map(|n| n.as_ptr()).collect();

        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&extension_names)
            .enabled_layer_names(&layer_ptrs);

        let instance = unsafe { entry.create_instance(&create_info, None)? };

        let (debug_utils, debug_messenger) = if enable_validation {
            let debug_utils = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_callback));

            let messenger =
                unsafe { debug_utils.create_debug_utils_messenger(&messenger_info, None)? };
            (Some(debug_utils), Some(messenger))
        } else {
            (None, None)
        };

        log::info!("Vulkan instance created successfully");

        Ok(Self {
            entry,
            instance,
            debug_messenger,
            debug_utils,
        })
    }

    fn validation_layers_available(entry: &Entry) -> bool {
        let Ok(layers) = (unsafe { entry.enumerate_instance_layer_properties() }) else {
            return false;
        };
        layers.iter().any(|l| {
            let name = unsafe { CStr::from_ptr(l.layer_name.as_ptr()) };
            name.to_bytes() == b"VK_LAYER_KHRONOS_validation"
        })
    }
}

impl Drop for VulkanInstance {
    fn drop(&mut self) {
        unsafe {
            if let (Some(debug_utils), Some(messenger)) = (&self.debug_utils, self.debug_messenger)
            {
                debug_utils.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _type: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let message = CStr::from_ptr((*data).p_message).to_string_lossy();
    match severity {
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR => log::error!("[Vulkan] {}", message),
        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING => log::warn!("[Vulkan] {}", message),
        _ => log::debug!("[Vulkan] {}", message),
    }
    vk::FALSE
}
