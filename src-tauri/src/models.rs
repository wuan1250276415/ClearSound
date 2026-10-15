use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub id: i64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub cue_path: Option<String>,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: String,
    pub duration_ms: u64,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub format: String,
    pub folder: String,
    pub has_art: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub name: String,
    pub artist: String,
    pub track_count: u32,
    pub duration_ms: u64,
    pub art_track_id: i64,
    pub year: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub name: String,
    pub album_count: u32,
    pub track_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub path: String,
    pub track_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub track_count: u32,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub item_id: i64,
    pub position: i64,
    pub track: Track,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn from_str(value: &str) -> Self {
        match value {
            "all" => Self::All,
            "one" => Self::One,
            _ => Self::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum OutputMode {
    #[default]
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub volume: f32,
    pub shuffle: bool,
    pub repeat_mode: RepeatMode,
    #[serde(default = "default_on")]
    pub auto_online_lyrics: bool,
    #[serde(default = "default_on")]
    pub auto_online_cover: bool,
    #[serde(default)]
    pub save_lrc_beside_file: bool,
    #[serde(default)]
    pub output_device_id: String,
    #[serde(default)]
    pub output_mode: OutputMode,
    #[serde(default)]
    pub exclusive_volume_lock: bool,
    #[serde(default)]
    pub dop: bool,
}

fn default_on() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.85,
            shuffle: false,
            repeat_mode: RepeatMode::Off,
            auto_online_lyrics: true,
            auto_online_cover: true,
            save_lrc_beside_file: false,
            output_device_id: String::new(),
            output_mode: OutputMode::Shared,
            exclusive_volume_lock: false,
            dop: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat_mode: RepeatMode,
    pub queue_index: Option<usize>,
    pub current: Option<Track>,
    pub queue: Vec<Track>,
    pub error: Option<String>,
    pub warning: Option<String>,
    #[serde(default)]
    pub output_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub skipped: u32,
    pub errors: Vec<String>,
}

impl ScanReport {
    pub fn merge(&mut self, other: ScanReport) {
        self.added += other.added;
        self.updated += other.updated;
        self.removed += other.removed;
        self.skipped += other.skipped;
        self.errors.extend(other.errors);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanned: u32,
    pub total: u32,
    pub current: String,
}
