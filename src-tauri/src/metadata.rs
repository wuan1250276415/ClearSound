//! Tag, duration, and cover reading. DSF/DFF headers are parsed directly
//! because tag libraries often skip DSD.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct FileTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<String>,
    pub duration_ms: u64,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub art: Option<(String, Vec<u8>)>,
}

impl Default for FileTags {
    fn default() -> Self {
        Self {
            title: None,
            artist: None,
            album: None,
            album_artist: None,
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: 0,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            art: None,
        }
    }
}

pub fn read_tags(path: &Path) -> FileTags {
    let mut tags = match lofty::read_from_path(path) {
        Ok(tagged) => tags_from_lofty(&tagged),
        Err(_) => FileTags::default(),
    };
    if is_dsd(path) {
        if let Some(dsd) = read_dsd_header(path) {
            if tags.duration_ms == 0 {
                tags.duration_ms = dsd.duration_ms;
            }
            tags.sample_rate = Some(dsd.sample_rate);
            tags.bit_depth = Some(dsd.bits.max(1));
            if tags.channels.is_none() {
                tags.channels = Some(dsd.channels);
            }
            if dsd.metadata_offset > 0 {
                if let Some(id3) = read_id3_at(path, dsd.metadata_offset) {
                    if tags.title.is_none() {
                        tags.title = id3.title;
                    }
                    if tags.artist.is_none() {
                        tags.artist = id3.artist;
                    }
                    if tags.album.is_none() {
                        tags.album = id3.album;
                    }
                    if tags.track_number.is_none() {
                        tags.track_number = id3.track_number;
                    }
                    if tags.art.is_none() {
                        tags.art = id3.art;
                    }
                }
            }
        }
    }
    tags
}

fn tags_from_lofty(tagged: &lofty::file::TaggedFile) -> FileTags {
    let props = tagged.properties();
    let mut tags = FileTags {
        duration_ms: u64::try_from(props.duration().as_millis()).unwrap_or(0),
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth().map(u32::from),
        channels: props.channels().map(u32::from),
        ..FileTags::default()
    };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return tags;
    };
    tags.title = tag.title().map(|v| v.into_owned()).filter(|s| !s.is_empty());
    tags.artist = tag.artist().map(|v| v.into_owned()).filter(|s| !s.is_empty());
    tags.album = tag.album().map(|v| v.into_owned()).filter(|s| !s.is_empty());
    tags.album_artist = tag
        .get_string(ItemKey::AlbumArtist)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    tags.track_number = tag.track();
    tags.disc_number = tag.disk();
    tags.year = tag.date().map(|d| d.year.to_string());
    if let Some(picture) = tag
        .get_picture_type(PictureType::CoverFront)
        .or_else(|| tag.pictures().first())
    {
        let bytes = picture.data();
        if !bytes.is_empty() && bytes.len() <= 20 * 1024 * 1024 {
            let mime = picture
                .mime_type()
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "image/jpeg".to_string());
            tags.art = Some((mime, bytes.to_vec()));
        }
    }
    tags
}

fn is_dsd(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("dsf" | "dff")
    )
}

struct DsdInfo {
    sample_rate: u32,
    bits: u32,
    channels: u32,
    duration_ms: u64,
    metadata_offset: u64,
}

fn read_dsd_header(path: &Path) -> Option<DsdInfo> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    let mut file = File::open(path).ok()?;
    if ext == "dsf" {
        return read_dsf(&mut file);
    }
    if ext == "dff" {
        return read_dff(&mut file);
    }
    None
}

fn read_dsf(file: &mut File) -> Option<DsdInfo> {
    let mut dsd = [0u8; 28];
    file.read_exact(&mut dsd).ok()?;
    if &dsd[0..4] != b"DSD " {
        return None;
    }
    let metadata_offset = u64::from_le_bytes(dsd[20..28].try_into().ok()?);
    let mut fmt = [0u8; 52];
    file.read_exact(&mut fmt).ok()?;
    if &fmt[0..4] != b"fmt " {
        return None;
    }
    let channels = u32::from_le_bytes(fmt[24..28].try_into().ok()?);
    let sample_rate = u32::from_le_bytes(fmt[28..32].try_into().ok()?);
    let bits = u32::from_le_bytes(fmt[32..36].try_into().ok()?);
    let samples = u64::from_le_bytes(fmt[36..44].try_into().ok()?);
    let duration_ms = if sample_rate > 0 {
        samples.saturating_mul(1000) / u64::from(sample_rate)
    } else {
        0
    };
    Some(DsdInfo {
        sample_rate,
        bits,
        channels,
        duration_ms,
        metadata_offset,
    })
}

/// DSDIFF: find the PROP/SND chunk and read sample rate, channels, and sample count.
fn read_dff(file: &mut File) -> Option<DsdInfo> {
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"FRM8" {
        return None;
    }
    let _form_size = read_be_u64(file)?;
    file.read_exact(&mut magic).ok()?;
    if &magic != b"DSD " {
        return None;
    }
    let mut sample_rate = 0u32;
    let mut channels = 0u32;
    let mut samples = 0u64;
    let file_len = file.metadata().ok()?.len();
    while file.stream_position().ok()? + 12 <= file_len {
        let mut id = [0u8; 4];
        if file.read_exact(&mut id).is_err() {
            break;
        }
        let size = read_be_u64(file)?;
        let start = file.stream_position().ok()?;
        if &id == b"PROP" {
            let mut prop = [0u8; 4];
            file.read_exact(&mut prop).ok()?;
            if &prop == b"SND " {
                let prop_end = start + size;
                while file.stream_position().ok()? + 12 <= prop_end {
                    let mut sub = [0u8; 4];
                    file.read_exact(&mut sub).ok()?;
                    let sub_size = read_be_u64(file)?;
                    let sub_start = file.stream_position().ok()?;
                    if &sub == b"FS  " && sub_size >= 4 {
                        sample_rate = read_be_u32(file)?;
                    } else if &sub == b"CHNL" && sub_size >= 2 {
                        channels = u32::from(read_be_u16(file)?);
                    } else if &sub == b"CMPR" {
                        // compression name, ignored
                    }
                    let next = sub_start + sub_size + (sub_size & 1);
                    file.seek(SeekFrom::Start(next)).ok()?;
                }
            }
        } else if &id == b"DSD " {
            // 1-bit samples are packed 8 per byte. Channel-interleaved in blocks,
            // but the chunk size in bytes is samples * channels / 8.
            if channels > 0 {
                samples = size.saturating_mul(8) / u64::from(channels);
            }
        }
        let next = start + size + (size & 1);
        if file.seek(SeekFrom::Start(next)).is_err() {
            break;
        }
        if sample_rate > 0 && channels > 0 && samples > 0 {
            break;
        }
    }
    if sample_rate == 0 {
        return None;
    }
    let duration_ms = samples.saturating_mul(1000) / u64::from(sample_rate);
    Some(DsdInfo {
        sample_rate,
        bits: 1,
        channels,
        duration_ms,
        metadata_offset: 0,
    })
}

struct Id3Text {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    track_number: Option<u32>,
    art: Option<(String, Vec<u8>)>,
}

fn read_id3_at(path: &Path, offset: u64) -> Option<Id3Text> {
    if offset == 0 {
        return None;
    }
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut header = [0u8; 10];
    file.read_exact(&mut header).ok()?;
    if &header[0..3] != b"ID3" {
        return None;
    }
    let version = header[3];
    let size = synchsafe(&header[6..10]) as usize;
    if size == 0 || size > 8 * 1024 * 1024 {
        return None;
    }
    let mut data = vec![0u8; size];
    file.read_exact(&mut data).ok()?;
    let mut text = Id3Text {
        title: None,
        artist: None,
        album: None,
        track_number: None,
        art: None,
    };
    let mut pos = 0;
    while pos + 10 <= data.len() {
        let id = &data[pos..pos + 4];
        if id.iter().all(|b| *b == 0) {
            break;
        }
        let frame_size = if version >= 4 {
            synchsafe(&data[pos + 4..pos + 8]) as usize
        } else {
            u32::from_be_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize
        };
        let frame_start = pos + 10;
        if frame_size == 0 || frame_start + frame_size > data.len() {
            break;
        }
        let body = &data[frame_start..frame_start + frame_size];
        let key = std::str::from_utf8(id).unwrap_or("");
        if key == "APIC" && text.art.is_none() {
            text.art = parse_apic(body);
        } else if matches!(key, "TIT2" | "TPE1" | "TALB" | "TRCK") {
            if let Some(value) = decode_id3_text(body) {
                match key {
                    "TIT2" => text.title = Some(value),
                    "TPE1" => text.artist = Some(value),
                    "TALB" => text.album = Some(value),
                    "TRCK" => {
                        text.track_number = value
                            .split('/')
                            .next()
                            .and_then(|n| n.trim().parse().ok());
                    }
                    _ => {}
                }
            }
        }
        pos = frame_start + frame_size;
    }
    Some(text)
}

fn parse_apic(body: &[u8]) -> Option<(String, Vec<u8>)> {
    if body.len() < 8 {
        return None;
    }
    let encoding = body[0];
    let mime_end = body[1..].iter().position(|byte| *byte == 0)?;
    let mut mime = String::from_utf8_lossy(&body[1..1 + mime_end]).trim().to_string();
    let mut pos = 1 + mime_end + 1;
    if pos >= body.len() {
        return None;
    }
    pos += 1;
    let desc = terminated_len(&body[pos..], encoding)?;
    pos += desc;
    if pos >= body.len() {
        return None;
    }
    let data = body[pos..].to_vec();
    if data.len() < 16 || data.len() > 20 * 1024 * 1024 {
        return None;
    }
    if mime.is_empty() {
        mime = sniff_image(&data)?.to_string();
    } else if !mime.contains('/') {
        mime = format!("image/{mime}");
    }
    Some((mime, data))
}

fn terminated_len(data: &[u8], encoding: u8) -> Option<usize> {
    match encoding {
        1 | 2 => {
            let mut end = 0;
            while end + 1 < data.len() {
                if data[end] == 0 && data[end + 1] == 0 {
                    return Some(end + 2);
                }
                end += 2;
            }
            Some(data.len())
        }
        _ => data.iter().position(|byte| *byte == 0).map(|end| end + 1).or(Some(data.len())),
    }
}

fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

fn decode_utf16_bom(raw: &[u8]) -> String {
    if raw.starts_with(&[0xFE, 0xFF]) {
        encoding_rs::UTF_16BE.decode(&raw[2..]).0.into_owned()
    } else if raw.starts_with(&[0xFF, 0xFE]) {
        encoding_rs::UTF_16LE.decode(&raw[2..]).0.into_owned()
    } else {
        encoding_rs::UTF_16LE.decode(raw).0.into_owned()
    }
}

fn decode_id3_text(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let encoding = body[0];
    let raw = &body[1..];
    let text = match encoding {
        0 => encoding_rs::WINDOWS_1252.decode(raw).0.into_owned(),
        1 => decode_utf16_bom(raw),
        2 => encoding_rs::UTF_16BE.decode(raw).0.into_owned(),
        3 => String::from_utf8_lossy(raw).into_owned(),
        _ => return None,
    };
    let text = text.trim_matches('\0').trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn synchsafe(bytes: &[u8]) -> u32 {
    ((bytes[0] as u32 & 0x7f) << 21)
        | ((bytes[1] as u32 & 0x7f) << 14)
        | ((bytes[2] as u32 & 0x7f) << 7)
        | (bytes[3] as u32 & 0x7f)
}

fn read_be_u64(file: &mut File) -> Option<u64> {
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).ok()?;
    Some(u64::from_be_bytes(buf))
}

fn read_be_u32(file: &mut File) -> Option<u32> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).ok()?;
    Some(u32::from_be_bytes(buf))
}

fn read_be_u16(file: &mut File) -> Option<u16> {
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).ok()?;
    Some(u16::from_be_bytes(buf))
}

pub fn folder_cover(dir: &Path) -> Option<(String, Vec<u8>)> {
    let names = [
        "cover.jpg",
        "cover.jpeg",
        "cover.png",
        "folder.jpg",
        "folder.jpeg",
        "folder.png",
        "front.jpg",
        "front.png",
        "album.jpg",
        "album.png",
        "artwork.jpg",
    ];
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    for name in names {
        if let Some(found) = entries.iter().find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        }) {
            let bytes = std::fs::read(found.path()).ok()?;
            if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
                return None;
            }
            let mime = if name.ends_with("png") {
                "image/png"
            } else {
                "image/jpeg"
            };
            return Some((mime.to_string(), bytes));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsf_header_duration() {
        let dir = std::env::temp_dir().join(format!(
            "chengyin-dsf-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tone.dsf");
        let mut bytes = vec![0u8; 80];
        bytes[0..4].copy_from_slice(b"DSD ");
        bytes[4..12].copy_from_slice(&28u64.to_le_bytes());
        bytes[28..32].copy_from_slice(b"fmt ");
        bytes[32..40].copy_from_slice(&52u64.to_le_bytes());
        bytes[52..56].copy_from_slice(&2u32.to_le_bytes());
        bytes[56..60].copy_from_slice(&2_822_400u32.to_le_bytes());
        bytes[60..64].copy_from_slice(&1u32.to_le_bytes());
        bytes[64..72].copy_from_slice(&2_822_400u64.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let info = read_dsf(&mut File::open(&path).unwrap()).unwrap();
        assert_eq!(info.sample_rate, 2_822_400);
        assert_eq!(info.channels, 2);
        assert_eq!(info.duration_ms, 1000);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn apic_frame_keeps_jpeg() {
        let mut body = vec![0];
        body.extend(b"image/jpeg\0");
        body.push(3);
        body.push(0);
        body.extend(&[0xFF, 0xD8, 0xFF, 0xD9]);
        body.extend(std::iter::repeat(1).take(20));
        let (mime, data) = parse_apic(&body).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(data.starts_with(&[0xFF, 0xD8, 0xFF]));
    }
}
