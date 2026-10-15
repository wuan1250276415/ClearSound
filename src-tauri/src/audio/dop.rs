//! DSD over PCM. DSF and DFF stay bitstreams here; each 16 DSD bits become one
//! 24-bit PCM sample with an alternating 0x05 / 0xFA marker. The normal FFmpeg
//! path still converts DSD to 176.4 kHz PCM when DoP is off.

use super::decode::PcmSource;
use crate::models::Track;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub fn open_dop(track: &Track) -> Result<Box<dyn PcmSource>, String> {
    let path = Path::new(&track.path);
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut file = File::open(path).map_err(|err| format!("无法打开 DSD 文件：{err}"))?;
    let layout = if ext == "dsf" {
        read_dsf_layout(&mut file)?
    } else if ext == "dff" {
        read_dff_layout(&mut file)?
    } else {
        return Err("只有 DSF 和 DFF 可以输出 DoP".into());
    };
    let pcm_rate = super::negotiate::dop_pcm_rate(layout.dsd_rate)
        .ok_or_else(|| "这种 DSD 速率不能封装成 DoP".to_string())?;
    let channels = layout.channels.clamp(1, 2);
    let start_bits = track.start_ms.saturating_mul(u64::from(layout.dsd_rate)) / 1000;
    let end_bits = track
        .end_ms
        .map(|end| end.saturating_mul(u64::from(layout.dsd_rate)) / 1000);
    Ok(Box::new(DopSource {
        file,
        layout,
        pcm_rate,
        channels,
        file_bits: 0,
        skip_bits: start_bits,
        end_bits,
        marker_high: true,
        pending: vec![Vec::new(); channels as usize],
        block_index: 0,
    }))
}

struct Layout {
    dsd_rate: u32,
    channels: u16,
    block_size: usize,
    data_start: u64,
    data_bytes: u64,
    lsb_first: bool,
}

struct DopSource {
    file: File,
    layout: Layout,
    pcm_rate: u32,
    channels: u16,
    file_bits: u64,
    skip_bits: u64,
    end_bits: Option<u64>,
    marker_high: bool,
    pending: Vec<Vec<u8>>,
    block_index: u64,
}

impl PcmSource for DopSource {
    fn rate(&self) -> u32 {
        self.pcm_rate
    }

    fn pull(&mut self) -> Result<Option<Vec<f32>>, String> {
        self.discard_skipped()?;
        if self.end_bits.is_some_and(|end| self.file_bits >= end) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(1024 * 2);
        for _ in 0..1024 {
            if self.end_bits.is_some_and(|end| self.file_bits + 16 > end) {
                break;
            }
            let marker = if self.marker_high { 0x05 } else { 0xFA };
            self.marker_high = !self.marker_high;
            let mut words = [0i32; 2];
            for channel in 0..self.channels as usize {
                let pair = self.next_pair(channel)?;
                let Some(pair) = pair else {
                    if out.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(out));
                };
                words[channel] = dop_word(marker, pair[0], pair[1]);
            }
            if self.channels == 1 {
                words[1] = words[0];
            }
            out.push(word_to_unit(words[0]));
            out.push(word_to_unit(words[1]));
            self.file_bits += 16;
        }
        if out.is_empty() {
            Ok(None)
        } else {
            Ok(Some(out))
        }
    }
}

impl DopSource {
    fn discard_skipped(&mut self) -> Result<(), String> {
        while self.skip_bits >= 16 {
            for channel in 0..self.channels as usize {
                if self.next_pair(channel)?.is_none() {
                    self.skip_bits = 0;
                    return Ok(());
                }
            }
            self.skip_bits -= 16;
            self.file_bits += 16;
        }
        Ok(())
    }

    fn next_pair(&mut self, channel: usize) -> Result<Option<[u8; 2]>, String> {
        while self.pending[channel].len() < 2 {
            if !self.read_block()? {
                return Ok(None);
            }
        }
        let first = self.pending[channel].remove(0);
        let second = self.pending[channel].remove(0);
        Ok(Some([first, second]))
    }

    fn read_block(&mut self) -> Result<bool, String> {
        let channels = self.layout.channels as usize;
        let block = self.layout.block_size.max(1);
        let offset = self.layout.data_start
            + self.block_index.saturating_mul(block as u64).saturating_mul(channels as u64);
        if offset >= self.layout.data_start + self.layout.data_bytes {
            return Ok(false);
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|err| format!("无法读取 DSD 数据：{err}"))?;
        for channel in 0..channels {
            let mut buf = vec![0u8; block];
            let read = self
                .file
                .read(&mut buf)
                .map_err(|err| format!("无法读取 DSD 数据：{err}"))?;
            if read == 0 {
                return Ok(false);
            }
            buf.truncate(read);
            if self.layout.lsb_first {
                for byte in &mut buf {
                    *byte = byte.reverse_bits();
                }
            }
            if channel < self.pending.len() {
                self.pending[channel].extend(buf);
            }
        }
        self.block_index += 1;
        Ok(true)
    }
}

pub fn dop_word(marker: u8, first: u8, second: u8) -> i32 {
    let unsigned = (u32::from(marker) << 16) | (u32::from(first) << 8) | u32::from(second);
    if unsigned & 0x80_0000 != 0 {
        unsigned as i32 - 0x100_0000
    } else {
        unsigned as i32
    }
}

pub fn word_to_unit(word: i32) -> f32 {
    word as f32 / 8_388_608.0
}

fn read_dsf_layout(file: &mut File) -> Result<Layout, String> {
    let mut header = [0u8; 28];
    file.read_exact(&mut header).map_err(|_| "DSF 头不完整".to_string())?;
    if &header[0..4] != b"DSD " {
        return Err("不是 DSF 文件".into());
    }
    let mut fmt = [0u8; 52];
    file.read_exact(&mut fmt).map_err(|_| "DSF fmt 不完整".to_string())?;
    if &fmt[0..4] != b"fmt " {
        return Err("DSF 缺少 fmt".into());
    }
    let channels = u32::from_le_bytes(fmt[24..28].try_into().unwrap());
    let dsd_rate = u32::from_le_bytes(fmt[28..32].try_into().unwrap());
    let block_size = u32::from_le_bytes(fmt[44..48].try_into().unwrap()) as usize;
    let mut data = [0u8; 12];
    file.read_exact(&mut data).map_err(|_| "DSF data 不完整".to_string())?;
    if &data[0..4] != b"data" {
        return Err("DSF 缺少 data".into());
    }
    let data_bytes = u64::from_le_bytes(data[4..12].try_into().unwrap()).saturating_sub(12);
    let data_start = file.stream_position().map_err(|err| err.to_string())?;
    if channels == 0 || dsd_rate == 0 || block_size == 0 {
        return Err("DSF 参数无效".into());
    }
    Ok(Layout {
        dsd_rate,
        channels: channels as u16,
        block_size,
        data_start,
        data_bytes,
        lsb_first: true,
    })
}

fn read_dff_layout(file: &mut File) -> Result<Layout, String> {
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|_| "DFF 头不完整".to_string())?;
    if &magic != b"FRM8" {
        return Err("不是 DFF 文件".into());
    }
    let _form_size = read_be_u64(file)?;
    file.read_exact(&mut magic).map_err(|_| "DFF 头不完整".to_string())?;
    if &magic != b"DSD " {
        return Err("不是 DFF 文件".into());
    }
    let mut dsd_rate = 0u32;
    let mut channels = 0u16;
    let file_len = file.metadata().map_err(|err| err.to_string())?.len();
    while file.stream_position().map_err(|err| err.to_string())? + 12 <= file_len {
        let mut id = [0u8; 4];
        file.read_exact(&mut id).map_err(|_| "DFF 块不完整".to_string())?;
        let size = read_be_u64(file)?;
        let start = file.stream_position().map_err(|err| err.to_string())?;
        if &id == b"PROP" {
            let mut prop = [0u8; 4];
            file.read_exact(&mut prop).map_err(|_| "DFF PROP 不完整".to_string())?;
            if &prop == b"SND " {
                let prop_end = start + size;
                while file.stream_position().map_err(|err| err.to_string())? + 12 <= prop_end {
                    let mut sub = [0u8; 4];
                    file.read_exact(&mut sub).map_err(|_| "DFF SND 不完整".to_string())?;
                    let sub_size = read_be_u64(file)?;
                    let sub_start = file.stream_position().map_err(|err| err.to_string())?;
                    if &sub == b"FS  " && sub_size >= 4 {
                        dsd_rate = read_be_u32(file)?;
                    } else if &sub == b"CHNL" && sub_size >= 2 {
                        channels = read_be_u16(file)?;
                    }
                    let next = sub_start + sub_size + (sub_size & 1);
                    file.seek(SeekFrom::Start(next)).map_err(|err| err.to_string())?;
                }
            }
        } else if &id == b"DSD " {
            if channels == 0 || dsd_rate == 0 {
                return Err("DFF 缺少采样率或声道".into());
            }
            return Ok(Layout {
                dsd_rate,
                channels,
                block_size: 1,
                data_start: start,
                data_bytes: size,
                lsb_first: false,
            });
        }
        let next = start + size + (size & 1);
        file.seek(SeekFrom::Start(next)).map_err(|err| err.to_string())?;
    }
    Err("DFF 里没有 DSD 数据".into())
}

fn read_be_u16(file: &mut File) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).map_err(|_| "DFF 字段不完整".to_string())?;
    Ok(u16::from_be_bytes(buf))
}

fn read_be_u32(file: &mut File) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).map_err(|_| "DFF 字段不完整".to_string())?;
    Ok(u32::from_be_bytes(buf))
}

fn read_be_u64(file: &mut File) -> Result<u64, String> {
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).map_err(|_| "DFF 字段不完整".to_string())?;
    Ok(u64::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_alternates_and_survives_float() {
        let low = dop_word(0x05, 0x12, 0x34);
        let high = dop_word(0xFA, 0x12, 0x34);
        assert_eq!(low, 0x05_1234);
        assert!(high < 0);
        let back = (word_to_unit(high) * 8_388_608.0).round() as i32;
        assert_eq!(back, high);
        let back_low = (word_to_unit(low) * 8_388_608.0).round() as i32;
        assert_eq!(back_low, low);
    }

    #[test]
    fn dsf_bytes_become_dop_frames() {
        let dir = std::env::temp_dir().join(format!("chengyin-dop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.dsf");
        std::fs::write(&path, tiny_dsf()).unwrap();
        let track = Track {
            id: 1,
            path: path.to_string_lossy().to_string(),
            cue_path: None,
            start_ms: 0,
            end_ms: None,
            title: "dsd".into(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            track_number: None,
            disc_number: None,
            year: String::new(),
            duration_ms: 1,
            sample_rate: Some(2_822_400),
            bit_depth: Some(1),
            channels: Some(2),
            format: "DSF".into(),
            folder: dir.to_string_lossy().to_string(),
            has_art: false,
            art_path: None,
        };
        let mut source = open_dop(&track).unwrap();
        assert_eq!(source.rate(), 176_400);
        let frames = source.pull().unwrap().unwrap();
        assert!(frames.len() >= 4);
        let first = (frames[0] * 8_388_608.0).round() as i32;
        assert_eq!(first, dop_word(0x05, 0x80, 0x80));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tiny_dsf() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DSD ");
        bytes.extend_from_slice(&28u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&52u64.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&2_822_400u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&16u64.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&20u64.to_le_bytes());
        // Two stereo blocks. LSB-first 0x01 reverses to 0x80.
        bytes.extend_from_slice(&[0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01]);
        bytes
    }
}
