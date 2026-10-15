//! Symphonia decodes FLAC, MP3, WAV, and M4A/ALAC. DSF, DFF, and APE go
//! through FFmpeg, which converts DSD to 176.4 kHz PCM.

use crate::models::Track;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

const DSD_PCM_RATE: u32 = 176_400;

pub trait PcmSource: Send {
    fn rate(&self) -> u32;
    /// Stereo interleaved PCM. `Ok(None)` is end of stream.
    fn pull(&mut self) -> Result<Option<Vec<f32>>, String>;
}

pub fn open_source(
    track: &Track,
    ffmpeg: Option<&Path>,
    offset_ms: u64,
) -> Result<Box<dyn PcmSource>, String> {
    let start = track.start_ms.saturating_add(offset_ms);
    if let Some(end) = track.end_ms {
        if start >= end {
            return Ok(Box::new(SilentEof { rate: track.sample_rate.unwrap_or(44_100) }));
        }
    }
    if needs_ffmpeg(&track.format) {
        let ffmpeg = ffmpeg.ok_or_else(|| ffmpeg_missing(&track.format))?;
        let rate = if is_dsd(&track.format) {
            DSD_PCM_RATE
        } else {
            track.sample_rate.unwrap_or(44_100).max(1)
        };
        return FfmpegSource::spawn(ffmpeg, Path::new(&track.path), start, track.end_ms, rate)
            .map(|source| Box::new(source) as Box<dyn PcmSource>);
    }
    match SymphoniaSource::open(Path::new(&track.path), start, track.end_ms) {
        Ok(source) => Ok(Box::new(source)),
        Err(err) => {
            if let Some(ffmpeg) = ffmpeg {
                let rate = track.sample_rate.unwrap_or(44_100).max(1);
                if let Ok(source) =
                    FfmpegSource::spawn(ffmpeg, Path::new(&track.path), start, track.end_ms, rate)
                {
                    return Ok(Box::new(source));
                }
            }
            Err(err)
        }
    }
}

fn needs_ffmpeg(format: &str) -> bool {
    matches!(format, "DSF" | "DFF" | "APE")
}

fn is_dsd(format: &str) -> bool {
    matches!(format, "DSF" | "DFF")
}

fn ffmpeg_missing(format: &str) -> String {
    format!("播放 {format} 需要 FFmpeg。Windows 安装包会附带它，也可以设置环境变量 CHENGYIN_FFMPEG。")
}

struct SilentEof {
    rate: u32,
}

impl PcmSource for SilentEof {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn pull(&mut self) -> Result<Option<Vec<f32>>, String> {
        Ok(None)
    }
}

struct SymphoniaSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    params: AudioCodecParameters,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    end_ms: Option<u64>,
    eof: bool,
}

impl SymphoniaSource {
    fn open(path: &Path, start_ms: u64, end_ms: Option<u64>) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|err| format!("无法打开音频文件：{err}"))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|err| format!("无法识别音频格式：{err}"))?;
        let track = probed
            .default_track(TrackType::Audio)
            .or_else(|| probed.first_track(TrackType::Audio))
            .cloned()
            .ok_or_else(|| "文件里没有音频轨".to_string())?;
        let params = match track.codec_params.clone() {
            Some(CodecParameters::Audio(params)) => params,
            _ => return Err("文件里没有可用的音频编码".into()),
        };
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|err| format!("无法解码此文件：{err}"))?;
        let rate = params.sample_rate.unwrap_or(44_100).max(1);
        let mut source = Self {
            reader: probed,
            decoder,
            params,
            track_id: track.id,
            time_base: track.time_base,
            rate,
            end_ms,
            eof: false,
        };
        if start_ms > 0 {
            source.seek_ms(start_ms)?;
        }
        Ok(source)
    }

    fn seek_ms(&mut self, ms: u64) -> Result<(), String> {
        let time = Time::try_from_secs_f64(ms as f64 / 1000.0).unwrap_or(Time::ZERO);
        self.reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|err| format!("无法跳转到指定位置：{err}"))?;
        self.decoder.reset();
        self.eof = false;
        Ok(())
    }

    fn packet_ms(&self, packet: &symphonia::core::packet::Packet) -> Option<u64> {
        let time_base = self.time_base?;
        let time = time_base.calc_time(packet.pts).unwrap_or(Time::ZERO);
        let millis = time.as_millis();
        if millis < 0 {
            Some(0)
        } else {
            u64::try_from(millis).ok()
        }
    }
}

impl PcmSource for SymphoniaSource {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn pull(&mut self) -> Result<Option<Vec<f32>>, String> {
        if self.eof {
            return Ok(None);
        }
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(SymError::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(SymError::ResetRequired) => {
                    self.recreate_decoder()?;
                    continue;
                }
                Err(SymError::DecodeError(_)) => continue,
                Err(err) => return Err(format!("读取音频失败：{err}")),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let packet_ms = self.packet_ms(&packet);
            if let (Some(end), Some(ms)) = (self.end_ms, packet_ms) {
                if ms >= end {
                    self.eof = true;
                    return Ok(None);
                }
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(buffer) => buffer,
                Err(SymError::DecodeError(_)) => continue,
                Err(SymError::ResetRequired) => {
                    self.recreate_decoder()?;
                    continue;
                }
                Err(SymError::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(err) => return Err(format!("解码失败：{err}")),
            };
            let mut samples = stereo_from_buffer(&decoded);
            let spec_rate = decoded.spec().rate().max(1);
            if spec_rate != self.rate {
                self.rate = spec_rate;
            }
            if let (Some(end), Some(ms)) = (self.end_ms, packet_ms) {
                let allowed_ms = end.saturating_sub(ms);
                let allowed_frames = (allowed_ms.saturating_mul(u64::from(self.rate)) / 1000) as usize;
                let frames = samples.len() / 2;
                if frames > allowed_frames {
                    samples.truncate(allowed_frames * 2);
                    self.eof = true;
                }
            }
            if samples.is_empty() {
                if self.eof {
                    return Ok(None);
                }
                continue;
            }
            return Ok(Some(samples));
        }
    }
}

impl SymphoniaSource {
    fn recreate_decoder(&mut self) -> Result<(), String> {
        self.decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&self.params, &AudioDecoderOptions::default())
            .map_err(|err| format!("无法重置解码器：{err}"))?;
        Ok(())
    }
}

fn stereo_from_buffer(buffer: &GenericAudioBufferRef<'_>) -> Vec<f32> {
    let frames = buffer.frames();
    let channels = buffer.spec().channels().count().max(1);
    let mut interleaved = vec![0.0f32; frames.saturating_mul(channels)];
    if interleaved.is_empty() {
        return Vec::new();
    }
    buffer.copy_to_slice_interleaved::<f32, _>(&mut interleaved);
    if channels == 2 {
        return interleaved;
    }
    let mut stereo = Vec::with_capacity(frames * 2);
    if channels == 1 {
        for sample in interleaved {
            stereo.push(sample);
            stereo.push(sample);
        }
        return stereo;
    }
    for frame in interleaved.chunks_exact(channels) {
        stereo.push(frame[0]);
        stereo.push(frame[1]);
    }
    stereo
}

struct FfmpegSource {
    child: Child,
    stdout: std::process::ChildStdout,
    rate: u32,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
    produced: bool,
}

impl FfmpegSource {
    fn spawn(
        ffmpeg: &Path,
        input: &Path,
        start_ms: u64,
        end_ms: Option<u64>,
        rate: u32,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(ffmpeg);
        cmd.arg("-hide_banner")
            .arg("-nostdin")
            .arg("-loglevel")
            .arg("error");
        let stamp = format!("{:.3}", start_ms as f64 / 1000.0);
        let dsd = rate == DSD_PCM_RATE;
        if start_ms > 0 && dsd {
            cmd.arg("-ss").arg(&stamp);
        }
        cmd.arg("-i").arg(input);
        if start_ms > 0 && !dsd {
            cmd.arg("-ss").arg(&stamp);
        }
        if let Some(end) = end_ms {
            let dur = end.saturating_sub(start_ms);
            cmd.arg("-t").arg(format!("{:.3}", dur as f64 / 1000.0));
        }
        cmd.args([
            "-vn",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ac",
            "2",
            "-ar",
        ]);
        cmd.arg(rate.to_string());
        cmd.arg("pipe:1");
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|err| format!("无法启动 FFmpeg：{err}"))?;
        let stdout = child.stdout.take().ok_or_else(|| "FFmpeg 没有标准输出".to_string())?;
        let mut stderr = child.stderr.take();
        let err_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let err_clone = err_buf.clone();
        std::thread::spawn(move || {
            let Some(pipe) = stderr.take() else {
                return;
            };
            let mut reader = std::io::BufReader::new(pipe);
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut reader, &mut text);
            if let Ok(mut slot) = err_clone.lock() {
                text.truncate(400);
                *slot = text;
            }
        });
        Ok(Self {
            child,
            stdout,
            rate,
            stderr: err_buf,
            produced: false,
        })
    }
}

impl Drop for FfmpegSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl PcmSource for FfmpegSource {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn pull(&mut self) -> Result<Option<Vec<f32>>, String> {
        let mut bytes = vec![0u8; 16 * 1024];
        let mut filled = 0usize;
        while filled + 4 <= bytes.len() {
            match self.stdout.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(format!("读取 FFmpeg 输出失败：{err}")),
            }
            if filled >= 4096 {
                break;
            }
        }
        filled -= filled % 4;
        if filled == 0 {
            if !self.produced {
                let message = self
                    .stderr
                    .lock()
                    .map(|text| text.clone())
                    .unwrap_or_default();
                if message.trim().is_empty() {
                    return Err("FFmpeg 没有输出音频".into());
                }
                return Err(format!("FFmpeg 解码失败：{}", message.trim()));
            }
            return Ok(None);
        }
        self.produced = true;
        let mut samples = Vec::with_capacity(filled / 4);
        for chunk in bytes[..filled].chunks_exact(4) {
            samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        if samples.len() % 2 == 1 {
            samples.pop();
        }
        Ok(Some(samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Track;

    fn track_for(path: &Path) -> Track {
        Track {
            id: 1,
            path: path.to_string_lossy().to_string(),
            cue_path: None,
            start_ms: 0,
            end_ms: None,
            title: "测试".into(),
            artist: "澄音".into(),
            album_artist: String::new(),
            album: "测试".into(),
            track_number: Some(1),
            disc_number: None,
            year: String::new(),
            duration_ms: 1000,
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            format: "WAV".into(),
            folder: path.parent().unwrap().to_string_lossy().to_string(),
            has_art: false,
            art_path: None,
        }
    }

    #[test]
    fn decodes_wav_to_stereo_pcm() {
        let dir = std::env::temp_dir().join(format!("chengyin-decode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..1600 {
            let sample = ((index as f32) * 0.2).sin();
            writer.write_sample((sample * 8000.0) as i16).unwrap();
        }
        writer.finalize().unwrap();

        let mut source = open_source(&track_for(&path), None, 0).unwrap();
        assert_eq!(source.rate(), 8_000);
        let mut frames = 0usize;
        loop {
            match source.pull().unwrap() {
                Some(samples) => {
                    assert_eq!(samples.len() % 2, 0);
                    frames += samples.len() / 2;
                }
                None => break,
            }
        }
        assert!(frames > 1000, "decoded {frames} frames");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
