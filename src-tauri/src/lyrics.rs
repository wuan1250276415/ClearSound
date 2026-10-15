//! Sidecar `.lrc` files and embedded lyrics.
//!
//! A same-name `.lrc` beside the audio file wins. CUE tracks also look for a
//! per-track file (title or track number) whose timestamps start at the cue
//! slice. An album-wide `.lrc` is shifted by the cue start so the panel stays
//! in sync with playback position. Embedded tags are the fallback.

use crate::models::Track;
use encoding_rs::GB18030;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::{ItemKey, Tag};
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAX_LYRICS_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub synced: bool,
    pub source: Option<LyricsSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub lines: Vec<LyricLine>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LyricsSource {
    File,
    Embedded,
    Online,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    pub time_ms: Option<u64>,
    pub text: String,
}

impl Lyrics {
    fn empty() -> Self {
        Self {
            synced: false,
            source: None,
            provider: None,
            lines: Vec::new(),
        }
    }

    pub(crate) fn is_usable(&self) -> bool {
        self.lines.iter().any(|line| !line.text.trim().is_empty())
    }
}

pub fn load(track: &Track) -> Lyrics {
    if let Some(lyrics) = sidecar(track) {
        if lyrics.is_usable() {
            return lyrics;
        }
    }
    if let Some(lyrics) = embedded(track) {
        if lyrics.is_usable() {
            return lyrics;
        }
    }
    Lyrics::empty()
}

fn sidecar(track: &Track) -> Option<Lyrics> {
    let audio = Path::new(&track.path);
    let audio_dir = audio.parent()?;
    let audio_stem = stem_string(audio);
    let cue_dir = track.cue_path.as_deref().and_then(|path| Path::new(path).parent());
    let cue_stem = track
        .cue_path
        .as_deref()
        .map(|path| stem_string(Path::new(path)));
    let mut dirs = vec![audio_dir.to_path_buf()];
    if let Some(dir) = cue_dir {
        if dir != audio_dir {
            dirs.push(dir.to_path_buf());
        }
    }

    if track.cue_path.is_some() {
        for dir in &dirs {
            if let Some(path) = find_named(dir, &per_track_names(track, &audio_stem, cue_stem.as_deref()))
            {
                return read_lrc_file(&path, false, track);
            }
        }
    }

    let mut album_names = vec![audio_stem];
    if let Some(stem) = cue_stem {
        if !album_names.iter().any(|name| name.eq_ignore_ascii_case(&stem)) {
            album_names.push(stem);
        }
    }
    for dir in &dirs {
        if let Some(path) = find_named(dir, &album_names) {
            let shift = track.cue_path.is_some();
            return read_lrc_file(&path, shift, track);
        }
    }
    None
}

fn per_track_names(track: &Track, audio_stem: &str, cue_stem: Option<&str>) -> Vec<String> {
    let title = track.title.trim();
    let mut names = Vec::new();
    if let Some(number) = track.track_number {
        let padded = format!("{number:02}");
        let plain = number.to_string();
        let numbers = if padded == plain {
            vec![plain]
        } else {
            vec![padded, plain]
        };
        if !title.is_empty() {
            for number in &numbers {
                for sep in [" ", ".", "-", "_"] {
                    names.push(format!("{number}{sep}{title}"));
                }
            }
        }
        for stem in [Some(audio_stem), cue_stem].into_iter().flatten() {
            if stem.is_empty() {
                continue;
            }
            for number in &numbers {
                names.push(format!("{stem}.{number}"));
                names.push(format!("{stem}-{number}"));
            }
        }
        names.extend(numbers);
    }
    if !title.is_empty() {
        names.push(title.to_string());
    }
    names
}

fn find_named(dir: &Path, names: &[String]) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let stem = name
                .rsplit_once('.')
                .filter(|(_, ext)| ext.eq_ignore_ascii_case("lrc"))?
                .0;
            Some((normalize_name(stem), path))
        })
        .collect();
    for wanted in names {
        let wanted = normalize_name(wanted);
        if wanted.is_empty() {
            continue;
        }
        if let Some((_, path)) = files.iter().find(|(stem, _)| stem == &wanted) {
            return Some(path.clone());
        }
    }
    None
}

fn normalize_name(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\''))
        .chars()
        .filter(|ch| !matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn stem_string(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_lrc_file(path: &Path, cue_shift: bool, track: &Track) -> Option<Lyrics> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_LYRICS_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let text = decode_lyrics_bytes(&bytes);
    let mut lyrics = parse_lrc(&text);
    if cue_shift {
        lyrics.lines = shift_cue_window(&lyrics.lines, track.start_ms, track.end_ms);
        lyrics.synced = lyrics.lines.iter().any(|line| line.time_ms.is_some());
    }
    if !lyrics.is_usable() {
        return None;
    }
    lyrics.source = Some(LyricsSource::File);
    Some(lyrics)
}

fn embedded(track: &Track) -> Option<Lyrics> {
    let path = Path::new(&track.path);
    let tagged = lofty::read_from_path(path).ok();
    let sample_rate = tagged.as_ref().and_then(|file| file.properties().sample_rate());
    let mut texts = Vec::new();
    if let Some(file) = tagged.as_ref() {
        for tag in file.tags() {
            texts.extend(lyric_strings(tag));
        }
    }
    let mut synced = None;
    let mut plain = None;
    for text in &texts {
        let parsed = parse_lrc(text);
        if parsed.synced && synced.is_none() {
            synced = Some(parsed);
        } else if plain.is_none() && parsed.is_usable() {
            plain = Some(parsed);
        }
    }
    if let Some(sylt) = read_id3_sylt(path, sample_rate) {
        if sylt.is_usable() {
            synced = Some(sylt);
        }
    }
    let mut lyrics = synced.or(plain)?;
    if track.cue_path.is_some() && lyrics.synced {
        lyrics.lines = shift_cue_window(&lyrics.lines, track.start_ms, track.end_ms);
        lyrics.synced = lyrics.lines.iter().any(|line| line.time_ms.is_some());
    }
    if !lyrics.is_usable() {
        return None;
    }
    lyrics.source = Some(LyricsSource::Embedded);
    Some(lyrics)
}

fn lyric_strings(tag: &Tag) -> Vec<String> {
    let mut out = Vec::new();
    for key in [ItemKey::Lyrics, ItemKey::UnsyncLyrics] {
        for text in tag.get_strings(key) {
            let text = text.trim();
            if !text.is_empty() && !out.iter().any(|seen: &String| seen == text) {
                out.push(text.to_string());
            }
        }
    }
    out
}

pub fn decode_lyrics_bytes(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    GB18030.decode(bytes).0.into_owned()
}

pub fn parse_lrc(text: &str) -> Lyrics {
    let mut offset_ms = 0i64;
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let (times, meta_offset, rest) = split_prefix_tags(line);
        if let Some(offset) = meta_offset {
            offset_ms = offset;
        }
        let text = strip_word_times(rest).trim().to_string();
        if times.is_empty() {
            if meta_offset.is_some() && text.is_empty() {
                continue;
            }
            if !text.is_empty() && !is_metadata_only(line) {
                plain.push(LyricLine {
                    time_ms: None,
                    text,
                });
            }
            continue;
        }
        for time in times {
            timed.push(LyricLine {
                time_ms: Some(time),
                text: text.clone(),
            });
        }
    }
    if timed.is_empty() {
        return Lyrics {
            synced: false,
            source: None,
            provider: None,
            lines: plain,
        };
    }
    for line in &mut timed {
        if let Some(time) = line.time_ms {
            let adjusted = time as i64 + offset_ms;
            line.time_ms = Some(if adjusted < 0 { 0 } else { adjusted as u64 });
        }
    }
    timed.sort_by(|a, b| a.time_ms.cmp(&b.time_ms));
    Lyrics {
        synced: true,
        source: None,
        provider: None,
        lines: timed,
    }
}

fn is_metadata_only(line: &str) -> bool {
    let (times, _, rest) = split_prefix_tags(line);
    times.is_empty() && rest.trim().is_empty()
}

fn split_prefix_tags(input: &str) -> (Vec<u64>, Option<i64>, &str) {
    let mut rest = input;
    let mut times = Vec::new();
    let mut offset = None;
    loop {
        let Some(body) = rest.strip_prefix('[') else {
            break;
        };
        let Some(end) = body.find(']') else {
            break;
        };
        let inside = &body[..end];
        if let Some(ms) = parse_timestamp(inside) {
            times.push(ms);
            rest = body[end + 1..].trim_start();
            continue;
        }
        if let Some((key, value)) = inside.split_once(':') {
            if key.chars().all(|ch| ch.is_ascii_alphabetic()) && !key.is_empty() {
                if key.eq_ignore_ascii_case("offset") {
                    offset = parse_offset(value);
                }
                rest = body[end + 1..].trim_start();
                continue;
            }
        }
        break;
    }
    (times, offset, rest)
}

fn parse_timestamp(inside: &str) -> Option<u64> {
    let (minutes, after) = inside.split_once(':')?;
    if minutes.is_empty() || !minutes.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let minutes: u64 = minutes.parse().ok()?;
    let (seconds, fraction) = if let Some((seconds, fraction)) = after.split_once(['.', ':']) {
        (seconds, Some(fraction))
    } else {
        (after, None)
    };
    if seconds.len() != 2 || !seconds.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let seconds: u64 = seconds.parse().ok()?;
    let fraction_ms = match fraction {
        None => 0,
        Some(raw) => {
            if raw.is_empty() || raw.len() > 3 || !raw.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            let value: u64 = raw.parse().ok()?;
            match raw.len() {
                1 => value.saturating_mul(100),
                2 => value.saturating_mul(10),
                _ => value,
            }
        }
    };
    Some(
        minutes
            .saturating_mul(60_000)
            .saturating_add(seconds.saturating_mul(1_000))
            .saturating_add(fraction_ms),
    )
}

fn parse_offset(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    value.parse::<i64>().ok()
}

fn strip_word_times(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('>') {
            let inside = &after[..end];
            if parse_timestamp(inside).is_some() {
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push('<');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn shift_cue_window(lines: &[LyricLine], start_ms: u64, end_ms: Option<u64>) -> Vec<LyricLine> {
    let mut shifted = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(time) = line.time_ms else {
            continue;
        };
        if let Some(end) = end_ms {
            if time >= end {
                continue;
            }
        }
        if time < start_ms {
            let next = lines.iter().skip(index + 1).find_map(|item| item.time_ms);
            let covers_start = next.map(|next| next >= start_ms).unwrap_or(true);
            if covers_start && shifted.is_empty() && !line.text.trim().is_empty() {
                shifted.push(LyricLine {
                    time_ms: Some(0),
                    text: line.text.clone(),
                });
            }
            continue;
        }
        shifted.push(LyricLine {
            time_ms: Some(time - start_ms),
            text: line.text.clone(),
        });
    }
    shifted
}

fn read_id3_sylt(path: &Path, sample_rate: Option<u32>) -> Option<Lyrics> {
    let bytes = read_id3_tag(path)?;
    let frames = id3_frames(&bytes)?;
    let mut best: Option<(u8, Vec<LyricLine>)> = None;
    for frame in frames {
        if !frame.id.eq_ignore_ascii_case("SYLT") {
            continue;
        }
        let Some((kind, lines)) = parse_sylt(&frame.body, sample_rate) else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((prev_kind, prev)) => {
                (kind == 1 && *prev_kind != 1) || (kind == *prev_kind && lines.len() > prev.len())
            }
        };
        if better && !lines.is_empty() {
            best = Some((kind, lines));
        }
    }
    let (_, lines) = best?;
    Some(Lyrics {
        synced: true,
        source: None,
        provider: None,
        lines,
    })
}

struct Id3Frame {
    id: String,
    body: Vec<u8>,
}

fn read_id3_tag(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    if let Some(tag) = read_id3_at(&mut file, 0) {
        return Some(tag);
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    if ext.as_deref() == Some("dsf") {
        file.seek(SeekFrom::Start(20)).ok()?;
        let mut offset = [0u8; 8];
        file.read_exact(&mut offset).ok()?;
        let offset = u64::from_le_bytes(offset);
        if offset > 0 {
            return read_id3_at(&mut file, offset);
        }
    }
    None
}

fn read_id3_at(file: &mut File, offset: u64) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut header = [0u8; 10];
    file.read_exact(&mut header).ok()?;
    if &header[0..3] != b"ID3" {
        return None;
    }
    let size = synchsafe(&header[6..10]) as usize;
    if size == 0 || size > 8 * 1024 * 1024 {
        return None;
    }
    let mut data = vec![0u8; 10 + size];
    data[..10].copy_from_slice(&header);
    file.read_exact(&mut data[10..]).ok()?;
    Some(data)
}

fn id3_frames(tag: &[u8]) -> Option<Vec<Id3Frame>> {
    if tag.len() < 10 || &tag[0..3] != b"ID3" {
        return None;
    }
    let version = tag[3];
    let mut frames = Vec::new();
    let mut pos = 10;
    if version == 2 {
        while pos + 6 <= tag.len() {
            let id = &tag[pos..pos + 3];
            if id.iter().all(|byte| *byte == 0) {
                break;
            }
            let size = u32::from_be_bytes([0, tag[pos + 3], tag[pos + 4], tag[pos + 5]]) as usize;
            let start = pos + 6;
            if size == 0 || start + size > tag.len() {
                break;
            }
            let key = match std::str::from_utf8(id).unwrap_or("") {
                "SLT" => "SYLT",
                "ULT" => "USLT",
                other => other,
            };
            frames.push(Id3Frame {
                id: key.to_string(),
                body: tag[start..start + size].to_vec(),
            });
            pos = start + size;
        }
        return Some(frames);
    }
    let v4 = version >= 4;
    while pos + 10 <= tag.len() {
        let id = &tag[pos..pos + 4];
        if id.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = if v4 {
            synchsafe(&tag[pos + 4..pos + 8]) as usize
        } else {
            u32::from_be_bytes(tag[pos + 4..pos + 8].try_into().ok()?) as usize
        };
        let start = pos + 10;
        if size == 0 || start + size > tag.len() {
            break;
        }
        frames.push(Id3Frame {
            id: String::from_utf8_lossy(id).into_owned(),
            body: tag[start..start + size].to_vec(),
        });
        pos = start + size;
    }
    Some(frames)
}

fn parse_sylt(body: &[u8], sample_rate: Option<u32>) -> Option<(u8, Vec<LyricLine>)> {
    if body.len() < 6 {
        return None;
    }
    let encoding = body[0];
    let format = body[4];
    let kind = body[5];
    let mut pos = 6;
    let (_, consumed) = take_terminated(&body[pos..], encoding)?;
    pos += consumed;
    let mut lines = Vec::new();
    while pos < body.len() {
        let (text, consumed) = take_terminated(&body[pos..], encoding)?;
        pos += consumed;
        if pos + 4 > body.len() {
            break;
        }
        let stamp = u32::from_be_bytes(body[pos..pos + 4].try_into().ok()?);
        pos += 4;
        let time_ms = match format {
            2 => u64::from(stamp),
            1 => {
                let rate = u64::from(sample_rate.filter(|rate| *rate > 0).unwrap_or(44_100));
                u64::from(stamp).saturating_mul(1152).saturating_mul(1000) / rate
            }
            _ => continue,
        };
        let text = text.trim_matches('\0').trim().to_string();
        lines.push(LyricLine {
            time_ms: Some(time_ms),
            text,
        });
    }
    if lines.is_empty() {
        None
    } else {
        Some((kind, lines))
    }
}

fn take_terminated(data: &[u8], encoding: u8) -> Option<(String, usize)> {
    match encoding {
        0 | 3 => {
            let end = data.iter().position(|byte| *byte == 0).unwrap_or(data.len());
            let raw = &data[..end];
            let text = if encoding == 0 {
                encoding_rs::WINDOWS_1252.decode(raw).0.into_owned()
            } else {
                String::from_utf8_lossy(raw).into_owned()
            };
            let consumed = if end < data.len() { end + 1 } else { end };
            Some((text, consumed))
        }
        1 | 2 => {
            let mut end = 0;
            while end + 1 < data.len() {
                if data[end] == 0 && data[end + 1] == 0 {
                    break;
                }
                end += 2;
            }
            let raw = &data[..end];
            let text = if encoding == 2 || raw.starts_with(&[0xFE, 0xFF]) {
                let slice = raw.strip_prefix(&[0xFE, 0xFF]).unwrap_or(raw);
                encoding_rs::UTF_16BE.decode(slice).0.into_owned()
            } else {
                let slice = raw.strip_prefix(&[0xFF, 0xFE]).unwrap_or(raw);
                encoding_rs::UTF_16LE.decode(slice).0.into_owned()
            };
            let consumed = if end + 1 < data.len() { end + 2 } else { end };
            Some((text, consumed))
        }
        _ => None,
    }
}

fn synchsafe(bytes: &[u8]) -> u32 {
    ((bytes[0] as u32 & 0x7f) << 21)
        | ((bytes[1] as u32 & 0x7f) << 14)
        | ((bytes[2] as u32 & 0x7f) << 7)
        | (bytes[3] as u32 & 0x7f)
}

pub fn lrc_save_path(track: &Track) -> Option<PathBuf> {
    let audio = Path::new(&track.path);
    let dir = audio.parent()?;
    let stem = if track.cue_path.is_some() {
        sanitize_file_stem(&track.title)
    } else {
        let stem = stem_string(audio);
        if stem.is_empty() {
            sanitize_file_stem(&track.title)
        } else {
            stem
        }
    };
    if stem.is_empty() {
        return None;
    }
    Some(dir.join(format!("{stem}.lrc")))
}

pub fn sanitize_file_stem(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| !matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'))
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    cleaned.chars().take(80).collect()
}

pub fn format_lrc(lyrics: &Lyrics, title: &str, artist: &str, album: &str) -> String {
    let mut out = String::new();
    if !title.trim().is_empty() {
        out.push_str(&format!("[ti:{}]\n", title.trim()));
    }
    if !artist.trim().is_empty() {
        out.push_str(&format!("[ar:{}]\n", artist.trim()));
    }
    if !album.trim().is_empty() {
        out.push_str(&format!("[al:{}]\n", album.trim()));
    }
    if lyrics.synced {
        for line in &lyrics.lines {
            let Some(ms) = line.time_ms else { continue };
            let minutes = ms / 60_000;
            let seconds = (ms % 60_000) / 1_000;
            let hundredths = (ms % 1_000) / 10;
            out.push_str(&format!("[{minutes:02}:{seconds:02}.{hundredths:02}]{}\n", line.text));
        }
    } else {
        for line in &lyrics.lines {
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::tag::{Tag, TagType};

    fn track(path: &str, title: &str) -> Track {
        Track {
            id: 1,
            path: path.into(),
            cue_path: None,
            start_ms: 0,
            end_ms: None,
            title: title.into(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            track_number: Some(1),
            disc_number: None,
            year: String::new(),
            duration_ms: 10_000,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            format: "FLAC".into(),
            folder: String::new(),
            has_art: false,
            art_path: None,
        }
    }

    #[test]
    fn parses_multiple_timestamps_offset_and_metadata() {
        let text = "\
[ti:夜航]
[offset:+500]
[00:01.00][00:02.50]靠岸
[00:03]灯塔
[00:04.2]半拍
[00:05:20]百分
";
        let lyrics = parse_lrc(text);
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines.len(), 5);
        assert_eq!(lyrics.lines[0].time_ms, Some(1_500));
        assert_eq!(lyrics.lines[0].text, "靠岸");
        assert_eq!(lyrics.lines[1].time_ms, Some(3_000));
        assert_eq!(lyrics.lines[1].text, "靠岸");
        assert_eq!(lyrics.lines[2].time_ms, Some(3_500));
        assert_eq!(lyrics.lines[2].text, "灯塔");
        assert_eq!(lyrics.lines[3].time_ms, Some(4_700));
        assert_eq!(lyrics.lines[4].time_ms, Some(5_700));
        assert_eq!(lyrics.lines[4].text, "百分");
    }

    #[test]
    fn negative_offset_clamps_and_plain_text_is_unsynced() {
        let synced = parse_lrc("[offset:-5000]\n[00:01.00]早了");
        assert_eq!(synced.lines[0].time_ms, Some(0));
        let plain = parse_lrc("第一行\n\n第二行\n");
        assert!(!plain.synced);
        assert_eq!(plain.lines.len(), 2);
        assert_eq!(plain.lines[1].text, "第二行");
        assert!(plain.lines[1].time_ms.is_none());
    }

    #[test]
    fn strips_word_timings_and_keeps_utf8_bom() {
        let lyrics = parse_lrc("\u{feff}[00:01.000]你<00:01.20>好 <00:01.40>啊");
        assert_eq!(lyrics.lines[0].text, "你好 啊");
        assert_eq!(lyrics.lines[0].time_ms, Some(1_000));
        let bom = decode_lyrics_bytes(&[0xEF, 0xBB, 0xBF, 0xE4, 0xBD, 0xA0]);
        assert_eq!(bom, "你");
    }

    #[test]
    fn decodes_gbk_lrc() {
        let text = "[00:01.50]潮汐";
        let (bytes, _, _) = encoding_rs::GBK.encode(text);
        assert!(std::str::from_utf8(&bytes).is_err());
        let decoded = decode_lyrics_bytes(&bytes);
        let lyrics = parse_lrc(&decoded);
        assert_eq!(lyrics.lines[0].text, "潮汐");
        assert_eq!(lyrics.lines[0].time_ms, Some(1_500));
    }

    #[test]
    fn cue_window_is_relative_to_the_slice() {
        let lines = parse_lrc("[00:00.00]专辑\n[00:10.00]前一首\n[01:00.00]这一首\n[01:30.00]下一首").lines;
        let shifted = shift_cue_window(&lines, 60_000, Some(90_000));
        assert_eq!(shifted.len(), 2);
        assert_eq!(shifted[0].text, "前一首");
        assert_eq!(shifted[0].time_ms, Some(0));
        assert_eq!(shifted[1].text, "这一首");
        assert_eq!(shifted[1].time_ms, Some(0));
    }

    #[test]
    fn cue_track_prefers_title_lrc_over_album_file() {
        let dir = std::env::temp_dir().join(format!("chengyin-lrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("night.lrc"), "[00:30.00]整轨").unwrap();
        std::fs::write(dir.join("靠岸.lrc"), "[00:01.00]分轨").unwrap();
        std::fs::write(dir.join("01.靠岸.lrc"), "[00:02.00]编号").unwrap();
        let mut item = track(&dir.join("night.flac").to_string_lossy(), "靠岸");
        item.cue_path = Some(dir.join("night.cue").to_string_lossy().into_owned());
        item.start_ms = 12_000;
        item.end_ms = Some(40_000);
        item.track_number = Some(1);
        let lyrics = load(&item);
        assert_eq!(lyrics.source, Some(LyricsSource::File));
        assert_eq!(lyrics.lines[0].text, "编号");
        assert_eq!(lyrics.lines[0].time_ms, Some(2_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn album_lrc_shifts_when_no_per_track_file() {
        let dir = std::env::temp_dir().join(format!("chengyin-lrc-album-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("night.lrc"), "[00:00.00]引子\n[00:12.00]靠岸\n[00:40.00]结束").unwrap();
        let mut item = track(&dir.join("night.flac").to_string_lossy(), "靠岸");
        item.cue_path = Some(dir.join("night.cue").to_string_lossy().into_owned());
        item.start_ms = 12_000;
        item.end_ms = Some(40_000);
        item.track_number = Some(2);
        let lyrics = load(&item);
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "引子");
        assert_eq!(lyrics.lines[0].time_ms, Some(0));
        assert_eq!(lyrics.lines[1].text, "靠岸");
        assert_eq!(lyrics.lines[1].time_ms, Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_name_lrc_for_a_normal_file() {
        let dir = std::env::temp_dir().join(format!("chengyin-lrc-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (bytes, _, _) = encoding_rs::GB18030.encode("[00:00.00]盐");
        std::fs::write(dir.join("salt.lrc"), &bytes).unwrap();
        let item = track(&dir.join("salt.mp3").to_string_lossy(), "盐");
        let lyrics = load(&item);
        assert_eq!(lyrics.lines[0].text, "盐");
        assert_eq!(lyrics.source, Some(LyricsSource::File));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vorbis_lyrics_key_parses_as_lrc() {
        let mut tag = Tag::new(TagType::VorbisComments);
        tag.insert_text(ItemKey::Lyrics, "[00:01.00]海风".into());
        tag.insert_text(ItemKey::UnsyncLyrics, "纯文本".into());
        let texts = lyric_strings(&tag);
        let parsed = parse_lrc(&texts[0]);
        assert!(parsed.synced);
        assert_eq!(parsed.lines[0].text, "海风");
    }

    #[test]
    fn reads_id3_sylt_milliseconds() {
        let mut body = vec![3, b'e', b'n', b'g', 2, 1, 0];
        body.extend(b"\xe6\xbd\xae\xe6\xb1\x90\x00");
        body.extend(1_500u32.to_be_bytes());
        let frame = id3_frame("SYLT", &body);
        let tag = id3_tag(&frame);
        let dir = std::env::temp_dir().join(format!("chengyin-sylt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("song.mp3");
        std::fs::write(&path, &tag).unwrap();
        let lyrics = read_id3_sylt(&path, Some(44_100)).unwrap();
        assert_eq!(lyrics.lines[0].text, "潮汐");
        assert_eq!(lyrics.lines[0].time_ms, Some(1_500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn id3_frame(id: &str, body: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend(id.as_bytes());
        frame.extend((body.len() as u32).to_be_bytes());
        frame.extend([0, 0]);
        frame.extend(body);
        frame
    }

    fn id3_tag(frames: &[u8]) -> Vec<u8> {
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        let size = frames.len() as u32;
        tag.push(((size >> 21) & 0x7f) as u8);
        tag.push(((size >> 14) & 0x7f) as u8);
        tag.push(((size >> 7) & 0x7f) as u8);
        tag.push((size & 0x7f) as u8);
        tag.extend(frames);
        tag
    }
}
