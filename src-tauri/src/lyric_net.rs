//! Online lyrics. LRCLIB is tried first; NetEase Cloud Music fills in Chinese songs.
//! Results are cached under the app data directory. Network errors leave the panel empty.

use crate::http_client::{self, HttpBody};
use crate::lyrics::{self, Lyrics, LyricsSource};
use crate::models::Track;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HIT_TTL_SECS: u64 = 14 * 24 * 3600;
const MISS_TTL_SECS: u64 = 24 * 3600;
const AUTO_SCORE: i32 = 70;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricHit {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub synced: bool,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredHit {
    hit: LyricHit,
    body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheFile {
    at: u64,
    negative: bool,
    hits: Vec<StoredHit>,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}

impl Query {
    fn from_track(track: &Track) -> Self {
        Self {
            title: track.title.trim().to_string(),
            artist: preferred_artist(track).to_string(),
            album: track.album.trim().to_string(),
            duration_ms: track.duration_ms,
        }
    }
}

pub fn search(track: &Track, cache_dir: &Path) -> Result<Vec<LyricHit>, String> {
    let query = Query::from_track(track);
    if query.title.is_empty() {
        return Err("这首曲目没有标题，无法搜索歌词".into());
    }
    let path = cache_path(cache_dir, &query);
    if let Some(cache) = read_cache(&path) {
        if !cache.hits.is_empty() && fresh(&cache, HIT_TTL_SECS) {
            return Ok(visible(&cache.hits));
        }
    }
    match fetch(&query) {
        Fetch::Hits(hits) => {
            write_cache(
                &path,
                &CacheFile {
                    at: now_secs(),
                    negative: hits.is_empty(),
                    hits: hits.clone(),
                },
            );
            if hits.is_empty() {
                Err("没有匹配的歌词".into())
            } else {
                Ok(visible(&hits))
            }
        }
        Fetch::Offline => {
            if let Some(cache) = read_cache(&path) {
                if !cache.hits.is_empty() {
                    return Ok(visible(&cache.hits));
                }
            }
            Err("网络不可用，无法搜索歌词".into())
        }
    }
}

pub fn choose(track: &Track, cache_dir: &Path, key: &str, save_beside: bool) -> Result<Lyrics, String> {
    let query = Query::from_track(track);
    let path = cache_path(cache_dir, &query);
    let cache = read_cache(&path).ok_or_else(|| "请先搜索歌词".to_string())?;
    let stored = cache
        .hits
        .iter()
        .find(|item| item.hit.key == key)
        .ok_or_else(|| "找不到这条歌词，请重新搜索".to_string())?;
    let lyrics = lyrics_from_body(&stored.body, &stored.hit.provider);
    if !lyrics.is_usable() {
        return Err("这条歌词是空的".into());
    }
    if save_beside {
        let _ = write_sidecar(track, &lyrics, true);
    }
    Ok(lyrics)
}

pub fn auto_match(track: &Track, cache_dir: &Path, save_beside: bool) -> Lyrics {
    let query = Query::from_track(track);
    if query.title.is_empty() {
        return Lyrics::empty_public();
    }
    let path = cache_path(cache_dir, &query);
    if let Some(cache) = read_cache(&path) {
        let ttl = if cache.negative { MISS_TTL_SECS } else { HIT_TTL_SECS };
        if fresh(&cache, ttl) {
            return best_saved(&cache.hits, track, save_beside);
        }
    }
    match fetch(&query) {
        Fetch::Hits(hits) => {
            write_cache(
                &path,
                &CacheFile {
                    at: now_secs(),
                    negative: !acceptable(&query, &hits),
                    hits: hits.clone(),
                },
            );
            best_saved(&hits, track, save_beside)
        }
        Fetch::Offline => {
            if let Some(cache) = read_cache(&path) {
                return best_saved(&cache.hits, track, false);
            }
            Lyrics::empty_public()
        }
    }
}

enum Fetch {
    Hits(Vec<StoredHit>),
    Offline,
}

fn fetch(query: &Query) -> Fetch {
    let mut hits = Vec::new();
    let mut saw_server = false;
    match lrclib_get(query) {
        Provider::Hits(found) => {
            saw_server = true;
            hits.extend(found);
        }
        Provider::Miss => saw_server = true,
        Provider::Offline => {}
    }
    if !acceptable(query, &hits) {
        match lrclib_search(query) {
            Provider::Hits(found) => {
                saw_server = true;
                hits.extend(found);
            }
            Provider::Miss => saw_server = true,
            Provider::Offline => {}
        }
    }
    if !acceptable(query, &hits) {
        match netease(query) {
            Provider::Hits(found) => {
                saw_server = true;
                hits.extend(found);
            }
            Provider::Miss => saw_server = true,
            Provider::Offline => {}
        }
    }
    if hits.is_empty() && !saw_server {
        Fetch::Offline
    } else {
        hits.sort_by(|a, b| score(query, &b.hit).cmp(&score(query, &a.hit)));
        let mut seen = std::collections::HashSet::new();
        hits.retain(|item| seen.insert(item.hit.key.clone()));
        Fetch::Hits(hits)
    }
}

enum Provider {
    Hits(Vec<StoredHit>),
    Miss,
    Offline,
}

fn lrclib_get(query: &Query) -> Provider {
    let duration = query.duration_ms / 1000;
    let url = format!(
        "https://lrclib.net/api/get?track_name={}&artist_name={}&album_name={}&duration={duration}",
        http_client::encode_query(&query.title),
        http_client::encode_query(&query.artist),
        http_client::encode_query(&query.album),
    );
    match http_client::get(&url, None, 1_048_576) {
        HttpBody::Bytes(bytes) => match serde_json::from_slice::<LrcLibTrack>(&bytes) {
            Ok(item) => Provider::Hits(lrclib_hits(query, &[item])),
            Err(_) => Provider::Miss,
        },
        HttpBody::Status(404) => Provider::Miss,
        HttpBody::Status(_) => Provider::Miss,
        HttpBody::Offline => Provider::Offline,
    }
}

fn lrclib_search(query: &Query) -> Provider {
    let url = format!(
        "https://lrclib.net/api/search?track_name={}&artist_name={}",
        http_client::encode_query(&query.title),
        http_client::encode_query(&query.artist),
    );
    match http_client::get(&url, None, 1_048_576) {
        HttpBody::Bytes(bytes) => match serde_json::from_slice::<Vec<LrcLibTrack>>(&bytes) {
            Ok(items) => Provider::Hits(lrclib_hits(query, &items)),
            Err(_) => Provider::Miss,
        },
        HttpBody::Status(_) => Provider::Miss,
        HttpBody::Offline => Provider::Offline,
    }
}

fn netease(query: &Query) -> Provider {
    let keyword = format!("{} {}", query.title, query.artist).trim().to_string();
    let url = format!(
        "https://music.163.com/api/search/get?s={}&type=1&offset=0&limit=8",
        http_client::encode_query(&keyword),
    );
    let songs = match http_client::get(&url, Some("https://music.163.com/"), 1_048_576) {
        HttpBody::Bytes(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => netease_songs(&value),
            Err(_) => return Provider::Miss,
        },
        HttpBody::Status(_) => return Provider::Miss,
        HttpBody::Offline => return Provider::Offline,
    };
    let mut hits = Vec::new();
    for song in songs.into_iter().take(5) {
        let url = format!(
            "https://music.163.com/api/song/lyric?id={}&lv=-1&kv=-1&tv=-1",
            song.id
        );
        let body = match http_client::get(&url, Some("https://music.163.com/"), 1_048_576) {
            HttpBody::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            HttpBody::Status(_) => continue,
            HttpBody::Offline => {
                if hits.is_empty() {
                    return Provider::Offline;
                }
                break;
            }
        };
        let Some(text) = netease_lyric_text(&body) else {
            continue;
        };
        let parsed = lyrics::parse_lrc(&text);
        if !parsed.is_usable() {
            continue;
        }
        let hit = LyricHit {
            key: format!("netease:{}", song.id),
            title: song.title,
            artist: song.artist,
            album: song.album,
            duration_ms: song.duration_ms,
            synced: parsed.synced,
            provider: "网易云音乐".into(),
        };
        hits.push(StoredHit { hit, body: text });
    }
    if hits.is_empty() {
        Provider::Miss
    } else {
        Provider::Hits(hits)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibTrack {
    id: i64,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: String,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn lrclib_hits(query: &Query, items: &[LrcLibTrack]) -> Vec<StoredHit> {
    let mut hits = Vec::new();
    for item in items {
        if item.instrumental && item.synced_lyrics.as_deref().unwrap_or("").trim().is_empty() {
            continue;
        }
        let synced_body = item.synced_lyrics.as_deref().unwrap_or("").trim();
        let plain_body = item.plain_lyrics.as_deref().unwrap_or("").trim();
        let (body, synced) = if !synced_body.is_empty() && lyrics::parse_lrc(synced_body).synced {
            (synced_body.to_string(), true)
        } else if !plain_body.is_empty() {
            (plain_body.to_string(), false)
        } else if !synced_body.is_empty() {
            (synced_body.to_string(), false)
        } else {
            continue;
        };
        let hit = LyricHit {
            key: format!("lrclib:{}", item.id),
            title: item.track_name.clone(),
            artist: item.artist_name.clone(),
            album: item.album_name.clone(),
            duration_ms: (item.duration * 1000.0).round().max(0.0) as u64,
            synced,
            provider: "LRCLIB".into(),
        };
        if score(query, &hit) < 35 && !query.title.is_empty() {
            let same = normalize(&hit.title) == normalize(&query.title);
            if !same {
                continue;
            }
        }
        hits.push(StoredHit { hit, body });
    }
    hits
}

struct NeteaseSong {
    id: i64,
    title: String,
    artist: String,
    album: String,
    duration_ms: u64,
}

fn netease_songs(value: &Value) -> Vec<NeteaseSong> {
    let Some(songs) = value.pointer("/result/songs").and_then(|item| item.as_array()) else {
        return Vec::new();
    };
    songs
        .iter()
        .filter_map(|song| {
            let id = song.get("id")?.as_i64()?;
            let title = song.get("name")?.as_str()?.trim().to_string();
            if title.is_empty() {
                return None;
            }
            let artist = song
                .pointer("/artists")
                .and_then(|item| item.as_array())
                .map(|artists| {
                    artists
                        .iter()
                        .filter_map(|artist| artist.get("name").and_then(|name| name.as_str()))
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            let album = song.pointer("/album/name").and_then(|name| name.as_str()).unwrap_or("").to_string();
            let duration_ms = song.get("duration").and_then(|value| value.as_u64()).unwrap_or(0);
            Some(NeteaseSong {
                id,
                title,
                artist,
                album,
                duration_ms,
            })
        })
        .collect()
}

pub(crate) fn netease_lyric_text(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let lrc = value.pointer("/lrc/lyric").and_then(|item| item.as_str()).unwrap_or("").trim();
    if lrc.is_empty() {
        return None;
    }
    Some(lrc.to_string())
}

pub(crate) fn score(query: &Query, hit: &LyricHit) -> i32 {
    let mut total = 0;
    let title_q = normalize(&query.title);
    let title_h = normalize(&hit.title);
    if !title_q.is_empty() && title_q == title_h {
        total += 60;
    } else if !title_q.is_empty() && (title_h.contains(&title_q) || title_q.contains(&title_h)) {
        total += 35;
    }
    let artist_q = normalize(&query.artist);
    let artist_h = normalize(&hit.artist);
    if !artist_q.is_empty() && (artist_q == artist_h || artist_h.contains(&artist_q) || artist_q.contains(&artist_h)) {
        total += 30;
    }
    let album_q = normalize(&query.album);
    let album_h = normalize(&hit.album);
    if !album_q.is_empty() && !album_h.is_empty() && (album_q == album_h || album_h.contains(&album_q)) {
        total += 15;
    }
    if query.duration_ms > 0 && hit.duration_ms > 0 {
        let delta = query.duration_ms.abs_diff(hit.duration_ms);
        if delta <= 2_000 {
            total += 25;
        } else if delta <= 5_000 {
            total += 12;
        } else if delta <= 10_000 {
            total += 4;
        }
    }
    if hit.synced {
        total += 8;
    }
    total
}

fn normalize(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '-' | '_' | '·' | '・' | '(' | ')' | '（' | '）' | '[' | ']' | ',' | '，'))
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn lyrics_from_body(body: &str, provider: &str) -> Lyrics {
    let mut lyrics = lyrics::parse_lrc(body);
    lyrics.source = Some(LyricsSource::Online);
    lyrics.provider = Some(provider.to_string());
    lyrics
}

fn acceptable(query: &Query, hits: &[StoredHit]) -> bool {
    hits.iter().any(|item| score(query, &item.hit) >= AUTO_SCORE)
}

fn best_saved(hits: &[StoredHit], track: &Track, save_beside: bool) -> Lyrics {
    let query = Query::from_track(track);
    let Some(chosen) = hits.iter().max_by_key(|item| score(&query, &item.hit)) else {
        return Lyrics::empty_public();
    };
    if score(&query, &chosen.hit) < AUTO_SCORE {
        return Lyrics::empty_public();
    }
    let lyrics = lyrics_from_body(&chosen.body, &chosen.hit.provider);
    finish(lyrics, track, save_beside, true)
}

fn finish(lyrics: Lyrics, track: &Track, save_beside: bool, only_if_missing: bool) -> Lyrics {
    if lyrics.is_usable() && save_beside {
        let _ = write_sidecar(track, &lyrics, !only_if_missing);
    }
    if lyrics.is_usable() {
        lyrics
    } else {
        Lyrics::empty_public()
    }
}

fn write_sidecar(track: &Track, lyrics: &Lyrics, overwrite: bool) -> Option<PathBuf> {
    let path = lyrics::lrc_save_path(track)?;
    if path.is_file() && !overwrite {
        return Some(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let text = lyrics::format_lrc(lyrics, &track.title, &track.artist, &track.album);
    std::fs::write(&path, text).ok()?;
    Some(path)
}

fn visible(hits: &[StoredHit]) -> Vec<LyricHit> {
    hits.iter().map(|item| item.hit.clone()).collect()
}

fn cache_path(cache_dir: &Path, query: &Query) -> PathBuf {
    let raw = format!(
        "{}\n{}\n{}\n{}",
        normalize(&query.title),
        normalize(&query.artist),
        normalize(&query.album),
        query.duration_ms / 1000
    );
    let digest = Sha256::digest(raw.as_bytes());
    let name: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    cache_dir.join("lyrics").join(format!("{name}.json"))
}

fn read_cache(path: &Path) -> Option<CacheFile> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cache(path: &Path, cache: &CacheFile) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = std::fs::write(path, bytes);
    }
}

fn fresh(cache: &CacheFile, ttl: u64) -> bool {
    now_secs().saturating_sub(cache.at) < ttl
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn preferred_artist(track: &Track) -> &str {
    let album_artist = track.album_artist.trim();
    if !album_artist.is_empty() && album_artist != "Various Artists" {
        return album_artist;
    }
    track.artist.trim()
}

impl Lyrics {
    pub(crate) fn empty_public() -> Self {
        Self {
            synced: false,
            source: None,
            provider: None,
            lines: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> Query {
        Query {
            title: "靠岸".into(),
            artist: "林深".into(),
            album: "夜航".into(),
            duration_ms: 213_000,
        }
    }

    fn hit(title: &str, artist: &str, album: &str, duration_ms: u64, synced: bool) -> LyricHit {
        LyricHit {
            key: format!("{title}-{artist}"),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms,
            synced,
            provider: "LRCLIB".into(),
        }
    }

    #[test]
    fn score_prefers_title_artist_and_close_duration() {
        let query = query();
        let exact = hit("靠岸", "林深", "夜航", 213_400, true);
        let other = hit("灯塔", "林深", "夜航", 213_000, true);
        let far = hit("靠岸", "林深", "夜航", 400_000, false);
        assert!(score(&query, &exact) > score(&query, &other));
        assert!(score(&query, &exact) > score(&query, &far));
        assert!(score(&query, &exact) >= AUTO_SCORE);
    }

    #[test]
    fn lrclib_prefers_synced_body() {
        let item = LrcLibTrack {
            id: 7,
            track_name: "靠岸".into(),
            artist_name: "林深".into(),
            album_name: "夜航".into(),
            duration: 213.0,
            instrumental: false,
            plain_lyrics: Some("纯文本".into()),
            synced_lyrics: Some("[00:01.00]船靠近了".into()),
        };
        let hits = lrclib_hits(&query(), &[item]);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].hit.synced);
        assert!(hits[0].body.contains("船靠近了"));
        assert_eq!(hits[0].hit.provider, "LRCLIB");
    }

    #[test]
    fn netease_lyric_and_song_parse() {
        let songs = r#"{"result":{"songs":[{"id":42,"name":"潮汐","duration":205000,"artists":[{"name":"南风"}],"album":{"name":"海岸线"}}]}}"#;
        let parsed = netease_songs(&serde_json::from_str(songs).unwrap());
        assert_eq!(parsed[0].id, 42);
        assert_eq!(parsed[0].artist, "南风");
        let lyric = r#"{"lrc":{"lyric":"[00:01.50]海岸线慢慢后退\n"}}"#;
        assert!(netease_lyric_text(lyric).unwrap().contains("海岸线"));
        assert!(netease_lyric_text(r#"{"lrc":{"lyric":""}}"#).is_none());
    }

    #[test]
    fn auto_match_uses_cached_hit_without_network() {
        let dir = std::env::temp_dir().join(format!("chengyin-lyric-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lyrics")).unwrap();
        let track = sample_track();
        let query = Query::from_track(&track);
        let mut stored = StoredHit {
            hit: hit("靠岸", "林深", "夜航", 213_000, true),
            body: "[00:01.00]缓存的靠岸".into(),
        };
        stored.hit.key = "lrclib:1".into();
        write_cache(
            &cache_path(&dir, &query),
            &CacheFile {
                at: now_secs(),
                negative: false,
                hits: vec![stored],
            },
        );
        let lyrics = auto_match(&track, &dir, false);
        assert_eq!(lyrics.provider.as_deref(), Some("LRCLIB"));
        assert_eq!(lyrics.lines[0].text, "缓存的靠岸");
        assert!(lyrics.synced);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sample_track() -> Track {
        Track {
            id: 1,
            path: "C:/music/night.flac".into(),
            cue_path: None,
            start_ms: 0,
            end_ms: None,
            title: "靠岸".into(),
            artist: "林深".into(),
            album_artist: "林深".into(),
            album: "夜航".into(),
            track_number: Some(2),
            disc_number: None,
            year: String::new(),
            duration_ms: 213_000,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            format: "FLAC".into(),
            folder: String::new(),
            has_art: false,
            art_path: None,
        }
    }
}
