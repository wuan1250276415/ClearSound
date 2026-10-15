//! CUE sheet parser. Offsets use CD frames (75 per second). Text may be
//! UTF-8, UTF-16, GBK/GB18030, or Shift-JIS.

use encoding_rs::{Encoding, GBK, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueSheet {
    pub title: Option<String>,
    pub performer: Option<String>,
    pub year: Option<String>,
    pub files: Vec<CueFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueFile {
    pub name: String,
    pub tracks: Vec<CueTrack>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueTrack {
    pub number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    /// Playback start, from INDEX 01 (or INDEX 00 when 01 is absent).
    pub start_ms: u64,
    /// Start of the next track, or `None` for the last track in the file.
    pub end_ms: Option<u64>,
}

pub fn parse_cue(bytes: &[u8]) -> Result<CueSheet, String> {
    let text = decode_cue_text(bytes);
    parse_cue_str(&text)
}

pub fn parse_cue_str(text: &str) -> Result<CueSheet, String> {
    let mut sheet = CueSheet {
        title: None,
        performer: None,
        year: None,
        files: Vec::new(),
    };
    let mut file_idx: Option<usize> = None;
    let mut track_idx: Option<usize> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let (key, rest) = split_keyword(line);
        let key = key.to_ascii_uppercase();
        match key.as_str() {
            "REM" => {
                let (sub, value) = split_keyword(rest);
                if sheet.files.is_empty() && track_idx.is_none() {
                    match sub.to_ascii_uppercase().as_str() {
                        "DATE" | "YEAR" => {
                            let year = unquote(value);
                            if !year.is_empty() {
                                sheet.year = Some(year);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "TITLE" => {
                let value = unquote(rest);
                if let (Some(fi), Some(ti)) = (file_idx, track_idx) {
                    if let Some(track) = sheet.files.get_mut(fi).and_then(|f| f.tracks.get_mut(ti))
                    {
                        track.title = Some(value);
                    }
                } else {
                    sheet.title = Some(value);
                }
            }
            "PERFORMER" => {
                let value = unquote(rest);
                if let (Some(fi), Some(ti)) = (file_idx, track_idx) {
                    if let Some(track) = sheet.files.get_mut(fi).and_then(|f| f.tracks.get_mut(ti))
                    {
                        track.performer = Some(value);
                    }
                } else {
                    sheet.performer = Some(value);
                }
            }
            "FILE" => {
                let name = parse_file_name(rest).ok_or_else(|| "CUE FILE 行缺少文件名".to_string())?;
                sheet.files.push(CueFile {
                    name,
                    tracks: Vec::new(),
                });
                file_idx = Some(sheet.files.len() - 1);
                track_idx = None;
            }
            "TRACK" => {
                let fi = file_idx.ok_or_else(|| "CUE TRACK 出现在 FILE 之前".to_string())?;
                let number = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .ok_or_else(|| format!("无法解析音轨号：{rest}"))?;
                sheet.files[fi].tracks.push(CueTrack {
                    number,
                    title: None,
                    performer: None,
                    start_ms: 0,
                    end_ms: None,
                });
                track_idx = Some(sheet.files[fi].tracks.len() - 1);
            }
            "INDEX" => {
                let (fi, ti) = match (file_idx, track_idx) {
                    (Some(fi), Some(ti)) => (fi, ti),
                    _ => continue,
                };
                let mut parts = rest.split_whitespace();
                let index_no = parts.next().unwrap_or("");
                let stamp = parts.next().unwrap_or("");
                let Some(ms) = parse_cue_time(stamp) else {
                    continue;
                };
                let track = &mut sheet.files[fi].tracks[ti];
                if index_no == "01" || (index_no == "00" && track.start_ms == 0 && stamp != "00:00:00")
                {
                    if index_no == "01" || track.start_ms == 0 {
                        track.start_ms = ms;
                    }
                }
                if index_no == "01" {
                    track.start_ms = ms;
                } else if index_no == "00" && track.start_ms == 0 {
                    track.start_ms = ms;
                }
            }
            _ => {}
        }
    }

    if sheet.files.is_empty() {
        return Err("CUE 中没有 FILE 条目".into());
    }

    for file in &mut sheet.files {
        if file.tracks.is_empty() {
            continue;
        }
        // INDEX 01 is the audible start. The next track's INDEX 01 is this track's end,
        // which keeps pregaps attached to the previous track for gapless albums.
        let starts: Vec<u64> = file.tracks.iter().map(|t| t.start_ms).collect();
        for (i, track) in file.tracks.iter_mut().enumerate() {
            track.end_ms = starts.get(i + 1).copied();
        }
    }

    let has_track = sheet.files.iter().any(|f| !f.tracks.is_empty());
    if !has_track {
        return Err("CUE 中没有音轨".into());
    }
    Ok(sheet)
}

pub fn resolve_cue_audio(cue_dir: &Path, file_name: &str) -> Option<std::path::PathBuf> {
    let raw = std::path::PathBuf::from(file_name);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        cue_dir.join(&raw)
    };
    if candidate.is_file() {
        return Some(canonicalize_file(&candidate));
    }
    let wanted = std::path::Path::new(file_name).file_name()?;
    let entries = std::fs::read_dir(cue_dir).ok()?;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(&wanted.to_string_lossy())
            && entry.path().is_file()
        {
            return Some(canonicalize_file(&entry.path()));
        }
    }
    None
}

fn canonicalize_file(path: &Path) -> std::path::PathBuf {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = canon.to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(stripped)
    } else {
        canon
    }
}

pub fn decode_cue_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return UTF_16LE.decode(&bytes[2..]).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return UTF_16BE.decode(&bytes[2..]).0.into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }

    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let guessed = detector.guess(None, chardetng::Utf8Detection::Allow);
    let (text, _, had_errors) = guessed.decode(bytes);
    if !had_errors {
        return text.into_owned();
    }
    for encoding in [GBK, SHIFT_JIS, encoding_rs::GB18030, encoding_rs::BIG5] {
        let (text, _, had_errors) = encoding.decode(bytes);
        if !had_errors {
            return text.into_owned();
        }
    }
    let _ = UTF_8;
    guessed.decode(bytes).0.into_owned()
}

pub fn parse_cue_time(stamp: &str) -> Option<u64> {
    let mut parts = stamp.split(':');
    let mm: u64 = parts.next()?.parse().ok()?;
    let ss: u64 = parts.next()?.parse().ok()?;
    let ff: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || ss >= 60 {
        return None;
    }
    Some(mm.saturating_mul(60_000) + ss.saturating_mul(1_000) + ff.saturating_mul(1_000) / 75)
}

fn split_keyword(line: &str) -> (&str, &str) {
    let line = line.trim();
    match line.find(char::is_whitespace) {
        Some(idx) => (&line[..idx], line[idx..].trim()),
        None => (line, ""),
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].replace("\\\"", "\"")
    } else {
        value.to_string()
    }
}

fn parse_file_name(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        return Some(stripped[..end].to_string());
    }
    rest.split_whitespace().next().map(str::to_string)
}

#[allow(dead_code)]
fn _encoding_name(encoding: &'static Encoding) -> &'static str {
    encoding.name()
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTF8_CUE: &str = r#"REM DATE 2020
PERFORMER "林深"
TITLE "夜航"
FILE "night.flac" WAVE
  TRACK 01 AUDIO
    TITLE "启程"
    PERFORMER "林深"
    INDEX 00 00:00:00
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "靠岸"
    PERFORMER "林深"
    INDEX 00 03:11:00
    INDEX 01 03:12:37
"#;

    #[test]
    fn parses_offsets_and_metadata() {
        let sheet = parse_cue_str(UTF8_CUE).unwrap();
        assert_eq!(sheet.title.as_deref(), Some("夜航"));
        assert_eq!(sheet.performer.as_deref(), Some("林深"));
        assert_eq!(sheet.year.as_deref(), Some("2020"));
        assert_eq!(sheet.files.len(), 1);
        let tracks = &sheet.files[0].tracks;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title.as_deref(), Some("启程"));
        assert_eq!(tracks[0].start_ms, 0);
        let expected = 3 * 60_000 + 12 * 1_000 + 37 * 1_000 / 75;
        assert_eq!(tracks[0].end_ms, Some(expected));
        assert_eq!(tracks[1].start_ms, expected);
        assert_eq!(tracks[1].end_ms, None);
        assert_eq!(tracks[1].title.as_deref(), Some("靠岸"));
    }

    #[test]
    fn decodes_gbk_cue() {
        let (bytes, _, _) = encoding_rs::GBK.encode(UTF8_CUE);
        assert!(std::str::from_utf8(&bytes).is_err());
        let sheet = parse_cue(&bytes).unwrap();
        assert_eq!(sheet.title.as_deref(), Some("夜航"));
        assert_eq!(sheet.files[0].tracks[1].title.as_deref(), Some("靠岸"));
        assert_eq!(sheet.files[0].name, "night.flac");
    }

    #[test]
    fn decodes_shift_jis_cue() {
        let source = "PERFORMER \"浜田\"\nTITLE \"夜航船\"\nFILE \"album.flac\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"序\"\n    INDEX 01 00:00:00\n";
        let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(source);
        assert!(!errors);
        assert!(std::str::from_utf8(&bytes).is_err());
        let sheet = parse_cue(&bytes).unwrap();
        assert_eq!(sheet.performer.as_deref(), Some("浜田"));
        assert_eq!(sheet.title.as_deref(), Some("夜航船"));
        assert_eq!(sheet.files[0].tracks[0].title.as_deref(), Some("序"));
    }

    #[test]
    fn cue_time_frames() {
        assert_eq!(parse_cue_time("00:00:00"), Some(0));
        assert_eq!(parse_cue_time("00:00:75"), Some(1000));
        assert_eq!(parse_cue_time("01:02:03"), Some(62_000 + 3 * 1000 / 75));
        assert_eq!(parse_cue_time("nope"), None);
    }
}
