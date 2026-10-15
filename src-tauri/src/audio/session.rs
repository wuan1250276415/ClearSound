//! The output device. Shared mode keeps one cpal stream. Exclusive mode
//! reopens the WASAPI stream when the track's rate or bit depth changes.

use super::devices::shared_device_name;
use super::negotiate::{self, OpenPlan, PcmFormat, SampleKind, SourceAudio};
use super::{f32_callback, i16_callback, null_clock, Status};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

#[cfg(windows)]
use super::wasapi_out::ExclusiveStream;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPrefs {
    pub device_id: String,
    pub exclusive: bool,
    pub dop: bool,
    pub lock_volume: bool,
}

impl Default for OutputPrefs {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            exclusive: false,
            dop: false,
            lock_volume: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Opened {
    pub rate: u32,
    pub label: String,
    pub notice: Option<String>,
    pub exclusive: bool,
    pub dop: bool,
}

struct Signature {
    device_id: String,
    plan: OpenPlan,
}

pub struct OutputSession {
    status: Arc<Status>,
    shared: Option<cpal::Stream>,
    #[cfg(windows)]
    exclusive: Option<ExclusiveStream>,
    clock_stop: Option<Arc<AtomicBool>>,
    signature: Option<Signature>,
    blocked: bool,
    rate: u32,
}

impl OutputSession {
    pub fn start(status: Arc<Status>) -> Self {
        let mut session = Self {
            status,
            shared: None,
            #[cfg(windows)]
            exclusive: None,
            clock_stop: None,
            signature: None,
            blocked: false,
            rate: 48_000,
        };
        let opened = session.open_shared(&OutputPrefs::default(), None);
        session.publish(&opened);
        session
    }

    pub fn clear_block(&mut self) {
        self.blocked = false;
    }

    pub fn satisfies(&mut self, spec: &SourceAudio, prefs: &OutputPrefs) -> bool {
        let stored = self.signature.as_ref().map(|signature| {
            (
                signature.device_id.clone(),
                signature.plan.exclusive,
                signature.plan.dop,
                signature.plan.format,
            )
        });
        let Some((device_id, exclusive, dop, format)) = stored else {
            return false;
        };
        if device_id != prefs.device_id {
            return false;
        }
        let plan = self.plan(spec, prefs);
        if !plan.exclusive && !exclusive {
            return true;
        }
        plan.exclusive == exclusive && plan.dop == dop && plan.format == format
    }

    pub fn realign(&mut self, spec: &SourceAudio, prefs: &OutputPrefs) -> Opened {
        let plan = self.plan(spec, prefs);
        if self.signature.as_ref().is_some_and(|signature| {
            signature.device_id == prefs.device_id
                && ((!plan.exclusive && !signature.plan.exclusive)
                    || (plan.exclusive == signature.plan.exclusive
                        && plan.dop == signature.plan.dop
                        && plan.format == signature.plan.format))
        }) {
            return self.current_opened();
        }
        self.reopen(plan, prefs)
    }

    pub fn force_shared(&mut self, notice: String) -> Opened {
        self.blocked = true;
        let prefs = OutputPrefs {
            device_id: self
                .signature
                .as_ref()
                .map(|signature| signature.device_id.clone())
                .unwrap_or_default(),
            exclusive: false,
            dop: false,
            lock_volume: false,
        };
        let mut opened = self.open_shared(&prefs, Some(notice));
        opened.exclusive = false;
        opened.dop = false;
        self.signature = Some(Signature {
            device_id: prefs.device_id,
            plan: OpenPlan {
                format: PcmFormat {
                    rate: opened.rate,
                    store_bits: 32,
                    valid_bits: 32,
                    kind: SampleKind::Float,
                    channels: 2,
                },
                resampled: true,
                dop: false,
                exclusive: false,
                notice: opened.notice.clone(),
            },
        });
        self.publish(&opened);
        opened
    }

    fn plan(&mut self, spec: &SourceAudio, prefs: &OutputPrefs) -> OpenPlan {
        if prefs.exclusive && !self.blocked {
            #[cfg(windows)]
            {
                match super::wasapi_out::probe(&prefs.device_id, spec) {
                    Ok(plan) if plan.exclusive => return plan,
                    Ok(plan) => {
                        return shared_placeholder(spec, plan.notice);
                    }
                    Err(err) => return shared_placeholder(spec, Some(err)),
                }
            }
            #[cfg(not(windows))]
            {
                return shared_placeholder(
                    spec,
                    Some("当前系统没有 WASAPI 独占，已使用共享输出".into()),
                );
            }
        }
        shared_placeholder(spec, None)
    }

    fn reopen(&mut self, plan: OpenPlan, prefs: &OutputPrefs) -> Opened {
        self.close();
        let opened = if plan.exclusive {
            #[cfg(windows)]
            {
                match super::wasapi_out::start_exclusive(
                    Arc::clone(&self.status),
                    prefs.device_id.clone(),
                    plan.format,
                ) {
                    Ok(stream) => {
                        self.exclusive = Some(stream);
                        self.rate = plan.format.rate;
                        Opened {
                            rate: plan.format.rate,
                            label: negotiate::output_label(true, &plan.format, plan.dop),
                            notice: plan.notice.clone(),
                            exclusive: true,
                            dop: plan.dop,
                        }
                    }
                    Err(err) => {
                        self.blocked = true;
                        self.open_shared(prefs, Some(err))
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = &plan;
                self.open_shared(
                    prefs,
                    Some("当前系统没有 WASAPI 独占，已使用共享输出".into()),
                )
            }
        } else {
            self.open_shared(prefs, plan.notice.clone())
        };
        let stored = if opened.exclusive { plan } else { shared_plan_from(&opened) };
        self.signature = Some(Signature {
            device_id: prefs.device_id.clone(),
            plan: stored,
        });
        self.publish(&opened);
        opened
    }

    fn open_shared(&mut self, prefs: &OutputPrefs, notice: Option<String>) -> Opened {
        self.close();
        let name = shared_device_name(&prefs.device_id);
        if let Some((stream, rate, bits, float)) = start_shared(Arc::clone(&self.status), name.as_deref()) {
            self.shared = Some(stream);
            self.rate = rate;
            return Opened {
                rate,
                label: negotiate::shared_label(rate, bits, float),
                notice,
                exclusive: false,
                dop: false,
            };
        }
        self.rate = 48_000;
        self.status.out_rate.store(48_000, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let clock_status = Arc::clone(&self.status);
        let clock_stop = Arc::clone(&stop);
        let _ = thread::Builder::new()
            .name("chengyin-clock".into())
            .spawn(move || null_clock(clock_status, clock_stop));
        self.clock_stop = Some(stop);
        Opened {
            rate: 48_000,
            label: negotiate::shared_label(48_000, 32, true),
            notice: Some(
                notice.unwrap_or_else(|| "未检测到音频输出设备，播放进度仍会走动，但没有声音。".into()),
            ),
            exclusive: false,
            dop: false,
        }
    }

    fn close(&mut self) {
        self.shared = None;
        #[cfg(windows)]
        {
            self.exclusive = None;
        }
        if let Some(stop) = self.clock_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn publish(&self, opened: &Opened) {
        self.status.out_rate.store(opened.rate.max(1), Ordering::Relaxed);
        self.status.exclusive.store(opened.exclusive, Ordering::Relaxed);
        self.status.dop.store(opened.dop, Ordering::Relaxed);
        *self.status.output_label.lock() = opened.label.clone();
        *self.status.format_notice.lock() = opened.notice.clone();
    }

    fn current_opened(&self) -> Opened {
        Opened {
            rate: self.rate,
            label: self.status.output_label.lock().clone(),
            notice: self.status.format_notice.lock().clone(),
            exclusive: self.status.exclusive.load(Ordering::Relaxed),
            dop: self.signature.as_ref().is_some_and(|signature| signature.plan.dop),
        }
    }
}

fn shared_placeholder(spec: &SourceAudio, notice: Option<String>) -> OpenPlan {
    OpenPlan {
        format: PcmFormat {
            rate: spec.rate.max(1),
            store_bits: 32,
            valid_bits: 32,
            kind: SampleKind::Float,
            channels: spec.channels.max(1),
        },
        resampled: false,
        dop: false,
        exclusive: false,
        notice,
    }
}

fn shared_plan_from(opened: &Opened) -> OpenPlan {
    OpenPlan {
        format: PcmFormat {
            rate: opened.rate,
            store_bits: 32,
            valid_bits: 32,
            kind: SampleKind::Float,
            channels: 2,
        },
        resampled: false,
        dop: false,
        exclusive: false,
        notice: opened.notice.clone(),
    }
}

fn start_shared(
    status: Arc<Status>,
    device_name: Option<&str>,
) -> Option<(cpal::Stream, u32, u16, bool)> {
    let host = cpal::default_host();
    let device = named_device(&host, device_name).or_else(|| host.default_output_device())?;
    let supported = device.default_output_config().ok()?;
    let format = supported.sample_format();
    let config = supported.config();
    let rate = config.sample_rate.max(1);
    status.out_rate.store(rate, Ordering::Relaxed);
    let channels = config.channels as usize;
    let err_status = Arc::clone(&status);
    let on_error = move |err: cpal::Error| {
        *err_status.device_fault.lock() = Some(format!("音频输出中断：{err}。将改用共享模式"));
    };
    let stream = match format {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                config,
                f32_callback(Arc::clone(&status), channels),
                on_error,
                None,
            )
            .ok()?,
        cpal::SampleFormat::I16 => device
            .build_output_stream(
                config,
                i16_callback(Arc::clone(&status), channels),
                on_error,
                None,
            )
            .ok()?,
        _ => return None,
    };
    stream.play().ok()?;
    let (bits, float) = match format {
        cpal::SampleFormat::I16 => (16, false),
        _ => (32, true),
    };
    Some((stream, rate, bits, float))
}

fn named_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    let name = name?;
    let devices = host.output_devices().ok()?;
    devices.into_iter().find(|device| {
        device
            .description()
            .ok()
            .is_some_and(|description| description.name() == name)
    })
}
