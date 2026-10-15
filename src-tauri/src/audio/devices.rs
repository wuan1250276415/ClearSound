//! Render endpoints. Windows uses WASAPI so the same id can reopen an
//! exclusive stream. Other systems list the shared-mode devices.

use crate::models::OutputDevice;

pub fn list_output_devices() -> Vec<OutputDevice> {
    let mut devices = vec![OutputDevice {
        id: String::new(),
        name: "系统默认".into(),
    }];
    #[cfg(windows)]
    {
        devices.extend(super::wasapi_out::list_render_devices());
    }
    #[cfg(not(windows))]
    {
        devices.extend(shared_names());
    }
    devices
}

pub fn shared_device_name(device_id: &str) -> Option<String> {
    if device_id.is_empty() {
        return None;
    }
    list_output_devices()
        .into_iter()
        .find(|device| device.id == device_id)
        .map(|device| device.name)
}

#[cfg(not(windows))]
fn shared_names() -> Vec<OutputDevice> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let Ok(list) = host.output_devices() else {
        return Vec::new();
    };
    list.filter_map(|device| {
        let name = device.description().ok()?.name().to_string();
        if name.is_empty() {
            return None;
        }
        Some(OutputDevice {
            id: name.clone(),
            name,
        })
    })
    .collect()
}
