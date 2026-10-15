//! Choose a device format for shared or exclusive output.
//! Exclusive mode prefers the track's own rate and bit depth. Anything the
//! device rejects falls back to the nearest common rate.

// Format packing is called from the Windows exclusive renderer.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::models::Track;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    Int,
    Float,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    pub rate: u32,
    pub store_bits: u16,
    pub valid_bits: u16,
    pub kind: SampleKind,
    pub channels: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAudio {
    pub rate: u32,
    pub bits: u16,
    pub channels: u16,
    pub dsd: bool,
    pub dsd_rate: u32,
    pub want_dop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenPlan {
    pub format: PcmFormat,
    pub resampled: bool,
    pub dop: bool,
    pub exclusive: bool,
    pub notice: Option<String>,
}

const COMMON_RATES: &[u32] = &[
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
];

pub fn source_from_track(track: &Track, exclusive: bool, dop: bool) -> SourceAudio {
    let dsd = matches!(track.format.as_str(), "DSF" | "DFF");
    let channels = track.channels.unwrap_or(2).clamp(1, 8) as u16;
    if dsd {
        SourceAudio {
            rate: 176_400,
            bits: 24,
            channels: channels.min(2),
            dsd: true,
            dsd_rate: track.sample_rate.unwrap_or(2_822_400),
            want_dop: dop && exclusive && channels <= 2,
        }
    } else {
        SourceAudio {
            rate: track.sample_rate.unwrap_or(44_100).max(1),
            bits: track.bit_depth.unwrap_or(16).clamp(8, 32) as u16,
            channels,
            dsd: false,
            dsd_rate: 0,
            want_dop: false,
        }
    }
}

pub fn dop_pcm_rate(dsd_rate: u32) -> Option<u32> {
    match dsd_rate {
        2_822_400 => Some(176_400),
        5_644_800 => Some(352_800),
        _ => None,
    }
}

pub fn choose_exclusive(source: &SourceAudio, supports: impl Fn(&PcmFormat) -> bool) -> OpenPlan {
    let channels = source.channels.max(1);
    if source.want_dop {
        if let Some(rate) = dop_pcm_rate(source.dsd_rate) {
            if let Some(format) = first_supported(&dop_layouts(rate, channels), &supports) {
                return OpenPlan {
                    format,
                    resampled: false,
                    dop: true,
                    exclusive: true,
                    notice: None,
                };
            }
            let pcm = choose_pcm(source, &supports);
            let mut notice = format!(
                "设备不支持 DoP 所需的 {}/24bit，已改为 PCM",
                rate_text(rate)
            );
            if let Some(extra) = pcm.notice {
                notice = format!("{notice}。{extra}");
            }
            return OpenPlan {
                notice: Some(notice),
                dop: false,
                ..pcm
            };
        }
        let pcm = choose_pcm(source, &supports);
        let notice = match pcm.notice {
            Some(extra) => format!("这种 DSD 速率不能封装成 DoP，已改为 PCM。{extra}"),
            None => "这种 DSD 速率不能封装成 DoP，已改为 PCM".to_string(),
        };
        return OpenPlan {
            notice: Some(notice),
            dop: false,
            ..pcm
        };
    }
    choose_pcm(source, &supports)
}

fn choose_pcm(source: &SourceAudio, supports: &impl Fn(&PcmFormat) -> bool) -> OpenPlan {
    let channels = source.channels.max(1);
    let native = layouts_at(source.rate, source.bits, channels);
    if let Some(format) = first_supported(&native, supports) {
        return accepted(format, false);
    }
    let mut rates = COMMON_RATES.to_vec();
    rates.retain(|rate| *rate != source.rate);
    rates.sort_by_key(|rate| rate_distance(*rate, source.rate));
    for rate in rates {
        if let Some(format) = first_supported(&layouts_at(rate, source.bits, channels), supports) {
            return OpenPlan {
                notice: Some(format!(
                    "设备不支持 {}，已改用 {} 并重采样",
                    describe_source(source),
                    describe_format(&format)
                )),
                format,
                resampled: true,
                dop: false,
                exclusive: true,
            };
        }
    }
    OpenPlan {
        format: PcmFormat {
            rate: 48_000,
            store_bits: 32,
            valid_bits: 32,
            kind: SampleKind::Float,
            channels,
        },
        resampled: source.rate != 48_000,
        dop: false,
        exclusive: false,
        notice: Some("设备没有可用的独占格式，已改用共享模式".into()),
    }
}

fn accepted(format: PcmFormat, resampled: bool) -> OpenPlan {
    OpenPlan {
        format,
        resampled,
        dop: false,
        exclusive: true,
        notice: None,
    }
}

fn first_supported(layouts: &[PcmFormat], supports: &impl Fn(&PcmFormat) -> bool) -> Option<PcmFormat> {
    layouts.iter().copied().find(|format| supports(format))
}

fn dop_layouts(rate: u32, channels: u16) -> Vec<PcmFormat> {
    vec![
        pcm(rate, 24, 24, SampleKind::Int, channels),
        pcm(rate, 32, 24, SampleKind::Int, channels),
    ]
}

fn layouts_at(rate: u32, native_bits: u16, channels: u16) -> Vec<PcmFormat> {
    depth_order(native_bits)
        .into_iter()
        .map(|(store, valid, kind)| pcm(rate, store, valid, kind, channels))
        .collect()
}

fn depth_order(native_bits: u16) -> Vec<(u16, u16, SampleKind)> {
    let all = [
        (32, 32, SampleKind::Int),
        (24, 24, SampleKind::Int),
        (16, 16, SampleKind::Int),
        (32, 32, SampleKind::Float),
        (32, 24, SampleKind::Int),
    ];
    let preferred = match native_bits {
        32 => (32, 32, SampleKind::Int),
        24 => (24, 24, SampleKind::Int),
        16 => (16, 16, SampleKind::Int),
        8 => (16, 16, SampleKind::Int),
        _ => (32, 32, SampleKind::Float),
    };
    let mut order = vec![preferred];
    for item in all {
        if item != preferred {
            order.push(item);
        }
    }
    order
}

fn pcm(rate: u32, store_bits: u16, valid_bits: u16, kind: SampleKind, channels: u16) -> PcmFormat {
    PcmFormat {
        rate,
        store_bits,
        valid_bits,
        kind,
        channels,
    }
}

fn rate_distance(candidate: u32, wanted: u32) -> u32 {
    candidate.abs_diff(wanted)
}

pub fn rate_text(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{}kHz", rate / 1000)
    } else if rate % 100 == 0 {
        format!("{:.1}kHz", rate as f64 / 1000.0)
    } else {
        format!("{rate}Hz")
    }
}

pub fn describe_format(format: &PcmFormat) -> String {
    let depth = match (format.kind, format.valid_bits, format.store_bits) {
        (SampleKind::Float, _, bits) => format!("{bits}bit float"),
        (SampleKind::Int, valid, store) if valid != store => format!("{valid}bit"),
        (SampleKind::Int, bits, _) => format!("{bits}bit"),
    };
    format!("{}/{depth}", rate_text(format.rate))
}

fn describe_source(source: &SourceAudio) -> String {
    format!("{}/{}bit", rate_text(source.rate), source.bits)
}

pub fn output_label(exclusive: bool, format: &PcmFormat, dop: bool) -> String {
    let mode = if exclusive { "WASAPI 独占" } else { "WASAPI 共享" };
    let dop_mark = if dop { " DoP" } else { "" };
    format!("{mode} · {}{dop_mark}", describe_format(format))
}

pub fn shared_label(rate: u32, bits: u16, float: bool) -> String {
    let format = PcmFormat {
        rate,
        store_bits: bits,
        valid_bits: bits,
        kind: if float { SampleKind::Float } else { SampleKind::Int },
        channels: 2,
    };
    let text = output_label(false, &format, false);
    #[cfg(windows)]
    {
        text
    }
    #[cfg(not(windows))]
    {
        text.replacen("WASAPI ", "", 1)
    }
}

/// Pack one stereo frame into the device's PCM layout.
pub fn append_frame(out: &mut Vec<u8>, left: f32, right: f32, format: &PcmFormat) {
    let channels = format.channels.max(1) as usize;
    let mono = (left + right) * 0.5;
    for index in 0..channels {
        let sample = match index {
            0 if channels == 1 => mono,
            0 => left,
            1 => right,
            _ => 0.0,
        };
        push_sample(out, sample, format);
    }
}

fn push_sample(out: &mut Vec<u8>, sample: f32, format: &PcmFormat) {
    match (format.kind, format.store_bits, format.valid_bits) {
        (SampleKind::Float, 32, _) => out.extend_from_slice(&sample.to_le_bytes()),
        (SampleKind::Float, 64, _) => out.extend_from_slice(&(f64::from(sample)).to_le_bytes()),
        (SampleKind::Int, 16, _) => {
            let value = quantize(sample, 16);
            out.extend_from_slice(&(value as i16).to_le_bytes());
        }
        (SampleKind::Int, 24, _) => {
            let value = quantize(sample, 24) as u32;
            out.extend_from_slice(&value.to_le_bytes()[..3]);
        }
        (SampleKind::Int, 32, valid) => {
            let bits = valid.min(32);
            let mut value = quantize(sample, bits);
            if bits < 32 {
                value <<= 32 - bits;
            }
            out.extend_from_slice(&value.to_le_bytes());
        }
        _ => {
            let value = quantize(sample, 16);
            out.extend_from_slice(&(value as i16).to_le_bytes());
        }
    }
}

fn quantize(sample: f32, bits: u16) -> i32 {
    let bits = bits.clamp(1, 32);
    let full = 1i32 << (bits - 1);
    let scaled = (sample.clamp(-1.0, 1.0) * full as f32).round() as i32;
    scaled.clamp(-full, full - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supports_only(allowed: &[PcmFormat]) -> impl Fn(&PcmFormat) -> bool + '_ {
        move |format| allowed.iter().any(|item| item == format)
    }

    fn pcm_source(rate: u32, bits: u16) -> SourceAudio {
        SourceAudio {
            rate,
            bits,
            channels: 2,
            dsd: false,
            dsd_rate: 0,
            want_dop: false,
        }
    }

    #[test]
    fn prefers_native_rate_and_depth() {
        let source = pcm_source(96_000, 24);
        let native = pcm(96_000, 24, 24, SampleKind::Int, 2);
        let plan = choose_exclusive(&source, supports_only(&[native]));
        assert_eq!(plan.format, native);
        assert!(!plan.resampled);
        assert!(plan.exclusive);
        assert_eq!(output_label(true, &plan.format, false), "WASAPI 独占 · 96kHz/24bit");
    }

    #[test]
    fn falls_back_to_nearest_rate() {
        let source = pcm_source(96_000, 24);
        let fallback = pcm(88_200, 24, 24, SampleKind::Int, 2);
        let plan = choose_exclusive(&source, supports_only(&[fallback]));
        assert_eq!(plan.format.rate, 88_200);
        assert!(plan.resampled);
        let notice = plan.notice.unwrap();
        assert!(notice.contains("96kHz/24bit"));
        assert!(notice.contains("重采样"));
    }

    #[test]
    fn uses_packed_24_in_32_when_24_is_rejected() {
        let source = pcm_source(48_000, 24);
        let packed = pcm(48_000, 32, 24, SampleKind::Int, 2);
        let plan = choose_exclusive(&source, supports_only(&[packed]));
        assert_eq!(plan.format.store_bits, 32);
        assert_eq!(plan.format.valid_bits, 24);
        assert!(!plan.resampled);
    }

    #[test]
    fn dop_uses_176k_for_dsd64() {
        let source = SourceAudio {
            rate: 176_400,
            bits: 24,
            channels: 2,
            dsd: true,
            dsd_rate: 2_822_400,
            want_dop: true,
        };
        let format = pcm(176_400, 24, 24, SampleKind::Int, 2);
        let plan = choose_exclusive(&source, supports_only(&[format]));
        assert!(plan.dop);
        assert_eq!(plan.format.rate, 176_400);
        assert_eq!(
            output_label(true, &plan.format, true),
            "WASAPI 独占 · 176.4kHz/24bit DoP"
        );
    }

    #[test]
    fn dop_falls_back_to_pcm_when_the_dop_rate_is_rejected() {
        let source = SourceAudio {
            rate: 176_400,
            bits: 24,
            channels: 2,
            dsd: true,
            dsd_rate: 2_822_400,
            want_dop: true,
        };
        let plan = choose_exclusive(&source, |format| format.rate == 48_000 && format.valid_bits == 16);
        assert!(!plan.dop);
        assert!(plan.resampled);
        assert!(plan.notice.unwrap().contains("DoP"));
    }

    #[test]
    fn quantize_roundtrip_for_16_and_24_in_32() {
        let mut bytes = Vec::new();
        let format = pcm(44_100, 16, 16, SampleKind::Int, 2);
        append_frame(&mut bytes, 0.5, -0.5, &format);
        assert_eq!(bytes.len(), 4);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 16384);

        bytes.clear();
        let wide = pcm(96_000, 32, 24, SampleKind::Int, 1);
        append_frame(&mut bytes, 0.5, 0.5, &wide);
        assert_eq!(bytes.len(), 4);
        let value = i32::from_le_bytes(bytes.as_slice().try_into().unwrap());
        assert_eq!(value, 1 << 30);
    }
}
