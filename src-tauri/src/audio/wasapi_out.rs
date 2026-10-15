//! WASAPI exclusive render. Event-driven first, then polling if the driver
//! rejects the event callback. A busy or missing device is a fallback, not a crash.

use super::negotiate::{self, OpenPlan, PcmFormat, SampleKind, SourceAudio};
use super::Status;
use crate::models::OutputDevice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub fn list_render_devices() -> Vec<OutputDevice> {
    let _ = wasapi::initialize_mta();
    let Ok(enumerator) = wasapi::DeviceEnumerator::new() else {
        return Vec::new();
    };
    let Ok(collection) = enumerator.get_device_collection(&wasapi::Direction::Render) else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for device in &collection {
        let Ok(device) = device else {
            continue;
        };
        let Ok(id) = device.get_id() else {
            continue;
        };
        let name = device.get_friendlyname().unwrap_or_else(|_| id.clone());
        devices.push(OutputDevice { id, name });
    }
    devices
}

pub fn probe(device_id: &str, source: &SourceAudio) -> Result<OpenPlan, String> {
    let _ = wasapi::initialize_mta();
    let device = open_device(device_id)?;
    let client = device
        .get_iaudioclient()
        .map_err(|err| format!("无法打开音频设备：{err}"))?;
    let plan = negotiate::choose_exclusive(source, |format| {
        let wave = to_wave(format);
        client.is_supported_exclusive_with_quirks(&wave).is_ok()
    });
    if !plan.exclusive {
        return Err(plan
            .notice
            .unwrap_or_else(|| "设备没有可用的独占格式".into()));
    }
    Ok(plan)
}

pub struct ExclusiveStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start_exclusive(
    status: Arc<Status>,
    device_id: String,
    format: PcmFormat,
) -> Result<ExclusiveStream, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("chengyin-wasapi".into())
        .spawn(move || {
            if let Err(err) = render(thread_stop, status.clone(), &device_id, format) {
                *status.device_fault.lock() = Some(err);
            }
        })
        .map_err(|err| format!("无法启动独占输出：{err}"))?;
    Ok(ExclusiveStream {
        stop,
        thread: Some(thread),
    })
}

fn render(stop: Arc<AtomicBool>, status: Arc<Status>, device_id: &str, format: PcmFormat) -> Result<(), String> {
    status.exclusive.store(true, Ordering::Relaxed);
    let _ = wasapi::initialize_mta();
    let device = open_device(device_id)?;
    let mut client = device
        .get_iaudioclient()
        .map_err(|err| format!("无法打开音频设备：{err}"))?;
    let wave = to_wave(&format);
    let accepted = client
        .is_supported_exclusive_with_quirks(&wave)
        .map_err(|_| "设备不接受这个独占格式".to_string())?;
    let period = client
        .calculate_aligned_period_near(100_000, Some(128), &accepted)
        .unwrap_or(100_000);
    let event_mode = wasapi::StreamMode::EventsExclusive { period_hns: period };
    if client
        .initialize_client(&accepted, &wasapi::Direction::Render, &event_mode)
        .is_err()
    {
        // Initialize can be called once. A driver that rejects the event
        // callback still gets exclusive mode through a fresh polling client.
        let mut client = device
            .get_iaudioclient()
            .map_err(|err| format!("无法打开音频设备：{err}"))?;
        let polling = wasapi::StreamMode::PollingExclusive {
            period_hns: period,
            buffer_duration_hns: period.saturating_mul(4).max(period),
        };
        client
            .initialize_client(&accepted, &wasapi::Direction::Render, &polling)
            .map_err(|err| fallback_message(&err))?;
        let render = client
            .get_audiorenderclient()
            .map_err(|err| fallback_message(&err))?;
        prime(&client, &render, &status, &format)?;
        client.start_stream().map_err(|err| fallback_message(&err))?;
        let mut smoothed = unity_or_volume(&status);
        while !stop.load(Ordering::Relaxed) {
            let frames = client
                .get_available_space_in_frames()
                .map_err(|err| fallback_message(&err))?;
            if frames == 0 {
                thread::sleep(Duration::from_millis(3));
                continue;
            }
            write_frames(&render, &status, frames as usize, &format, &mut smoothed)
                .map_err(|err| fallback_message(&err))?;
        }
        let _ = client.stop_stream();
        return Ok(());
    }
    let handle = client
        .set_get_eventhandle()
        .map_err(|err| fallback_message(&err))?;
    let render = client
        .get_audiorenderclient()
        .map_err(|err| fallback_message(&err))?;
    prime(&client, &render, &status, &format)?;
    client.start_stream().map_err(|err| fallback_message(&err))?;
    let mut smoothed = unity_or_volume(&status);
    let mut stalls = 0u8;
    while !stop.load(Ordering::Relaxed) {
        match handle.wait_for_event(200) {
            Ok(()) => stalls = 0,
            Err(wasapi::WasapiError::EventTimeout) => {
                stalls = stalls.saturating_add(1);
                if stalls >= 15 {
                    return Err("独占模式无法打开（设备被占用或已拔出），已改用共享模式".into());
                }
                continue;
            }
            Err(err) => return Err(fallback_message(&err)),
        }
        let frames = client
            .get_available_space_in_frames()
            .map_err(|err| fallback_message(&err))? as usize;
        if frames == 0 {
            continue;
        }
        write_frames(&render, &status, frames, &format, &mut smoothed).map_err(|err| fallback_message(&err))?;
    }
    let _ = client.stop_stream();
    Ok(())
}

fn prime(
    client: &wasapi::AudioClient,
    render: &wasapi::AudioRenderClient,
    status: &Status,
    format: &PcmFormat,
) -> Result<(), String> {
    let frames = client
        .get_available_space_in_frames()
        .map_err(|err| fallback_message(&err))? as usize;
    if frames == 0 {
        return Ok(());
    }
    let mut smoothed = unity_or_volume(status);
    write_frames(render, status, frames, format, &mut smoothed).map_err(|err| fallback_message(&err))
}

fn write_frames(
    render: &wasapi::AudioRenderClient,
    status: &Status,
    frames: usize,
    format: &PcmFormat,
    smoothed: &mut f32,
) -> Result<(), wasapi::WasapiError> {
    let pairs = super::pull_stereo(status, smoothed, frames);
    let mut bytes = Vec::with_capacity(frames * frame_bytes(format));
    for index in 0..frames {
        let (left, right) = pairs.get(index).copied().unwrap_or((0.0, 0.0));
        negotiate::append_frame(&mut bytes, left, right, format);
    }
    render.write_to_device(frames, &bytes, None)
}

fn frame_bytes(format: &PcmFormat) -> usize {
    format.channels.max(1) as usize * (format.store_bits.max(8) as usize / 8)
}

fn unity_or_volume(status: &Status) -> f32 {
    if status.exclusive.load(Ordering::Relaxed) && status.lock_volume.load(Ordering::Relaxed) {
        1.0
    } else {
        f32::from_bits(status.volume.load(Ordering::Relaxed))
    }
}

fn open_device(device_id: &str) -> Result<wasapi::Device, String> {
    let enumerator = wasapi::DeviceEnumerator::new().map_err(|err| format!("无法枚举音频设备：{err}"))?;
    if device_id.is_empty() {
        enumerator
            .get_default_device(&wasapi::Direction::Render)
            .map_err(|err| format!("没有默认输出设备：{err}"))
    } else {
        enumerator
            .get_device(device_id)
            .map_err(|_| "找不到所选输出设备".to_string())
    }
}

fn to_wave(format: &PcmFormat) -> wasapi::WaveFormat {
    let kind = match format.kind {
        SampleKind::Float => wasapi::SampleType::Float,
        SampleKind::Int => wasapi::SampleType::Int,
    };
    wasapi::WaveFormat::new(
        format.store_bits as usize,
        format.valid_bits as usize,
        &kind,
        format.rate as usize,
        format.channels as usize,
        None,
    )
}

fn fallback_message(err: &wasapi::WasapiError) -> String {
    if device_unavailable(err) {
        "独占模式无法打开（设备被占用或已拔出），已改用共享模式".into()
    } else {
        format!("独占模式无法打开，已改用共享模式：{err}")
    }
}

fn device_unavailable(err: &wasapi::WasapiError) -> bool {
    match err {
        wasapi::WasapiError::UnsupportedFormat | wasapi::WasapiError::DeviceNotFound(_) => true,
        wasapi::WasapiError::Windows(inner) => {
            let code = inner.code().0 as u32;
            matches!(
                code,
                0x8889_0004 | 0x8889_0008 | 0x8889_000A | 0x8889_000E | 0x8889_0019
            )
        }
        _ => false,
    }
}
