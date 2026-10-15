//! Online cover art. Embedded and folder images stay first; this runs only when
//! those are missing. iTunes, then MusicBrainz / Cover Art Archive, then NetEase.

use crate::http_client::{self, HttpBody};
use crate::models::Track;
use image::imageops::FilterType;
use image::ImageFormat;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MISS_TTL_SECS: u64 = 24 * 3600;

#[derive(Debug)]
pub enum CoverFetch {
    Image(Vec<u8>),
    Miss,
    Offline,
}

pub fn fetch(track: &Track, cache_dir: &Path) -> CoverFetch {
    let artist = preferred_artist(track);
    let album = track.album.trim();
    let title = track.title.trim();
    if artist.is_empty() && album.is_empty() {
        return CoverFetch::Miss;
    }
    let key_album = if album.is_empty() { title } else { album };
    let path = cache_path(cache_dir, artist, key_album);
    if let Some(bytes) = std::fs::read(&path).ok().filter(|bytes| !bytes.is_empty()) {
        return CoverFetch::Image(bytes);
    }
    let miss = path.with_extension("miss");
    if miss.is_file() && recent(&miss, MISS_TTL_SECS) {
        return CoverFetch::Miss;
    }
    match lookup(artist, key_album, album.is_empty()) {
        CoverFetch::Image(bytes) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, &bytes).is_ok() {
                let _ = std::fs::remove_file(&miss);
            }
            CoverFetch::Image(bytes)
        }
        CoverFetch::Miss => {
            if let Some(parent) = miss.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&miss, b"");
            CoverFetch::Miss
        }
        CoverFetch::Offline => CoverFetch::Offline,
    }
}

enum UrlResult {
    Url(String),
    Miss,
    Offline,
}

fn lookup(artist: &str, album: &str, album_was_empty: bool) -> CoverFetch {
    let mut answered = false;
    match itunes(artist, album) {
        UrlResult::Url(url) => {
            answered = true;
            match download(&url) {
                CoverFetch::Image(bytes) => return CoverFetch::Image(bytes),
                CoverFetch::Miss => {}
                CoverFetch::Offline => answered = false,
            }
        }
        UrlResult::Miss => answered = true,
        UrlResult::Offline => {}
    }
    match musicbrainz(artist, album) {
        CoverFetch::Image(bytes) => return CoverFetch::Image(bytes),
        CoverFetch::Miss => answered = true,
        CoverFetch::Offline => {}
    }
    if !album_was_empty {
        match netease_pic(artist, album) {
            CoverFetch::Image(bytes) => return CoverFetch::Image(bytes),
            CoverFetch::Miss => answered = true,
            CoverFetch::Offline => {}
        }
    }
    if answered {
        CoverFetch::Miss
    } else {
        CoverFetch::Offline
    }
}

fn itunes(artist: &str, album: &str) -> UrlResult {
    let term = format!("{artist} {album}").trim().to_string();
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=album&limit=5",
        http_client::encode_query(&term),
    );
    let bytes = match http_client::get(&url, None, 1_048_576) {
        HttpBody::Bytes(bytes) => bytes,
        HttpBody::Status(_) => return UrlResult::Miss,
        HttpBody::Offline => return UrlResult::Offline,
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return UrlResult::Miss;
    };
    match itunes_artwork(&value, artist, album) {
        Some(url) => UrlResult::Url(url),
        None => UrlResult::Miss,
    }
}

pub(crate) fn itunes_artwork(value: &Value, artist: &str, album: &str) -> Option<String> {
    let results = value.get("results")?.as_array()?;
    let album_n = normalize(album);
    let artist_n = normalize(artist);
    let mut fallback = None;
    for item in results {
        let art = item.get("artworkUrl100").and_then(|url| url.as_str())?;
        let name = item.get("collectionName").and_then(|name| name.as_str()).unwrap_or("");
        let person = item.get("artistName").and_then(|name| name.as_str()).unwrap_or("");
        let upgraded = upgrade_itunes(art);
        if fallback.is_none() {
            fallback = Some(upgraded.clone());
        }
        let name_ok = album_n.is_empty() || normalize(name).contains(&album_n) || album_n.contains(&normalize(name));
        let artist_ok = artist_n.is_empty() || normalize(person).contains(&artist_n) || artist_n.contains(&normalize(person));
        if name_ok && artist_ok {
            return Some(upgraded);
        }
    }
    fallback
}

fn upgrade_itunes(url: &str) -> String {
    url.replace("100x100bb", "600x600bb").replace("100x100-75", "600x600-75")
}

fn musicbrainz(artist: &str, album: &str) -> CoverFetch {
    let query = format!("release:\"{album}\" AND artist:\"{artist}\"");
    let url = format!(
        "https://musicbrainz.org/ws/2/release/?query={}&fmt=json&limit=3",
        http_client::encode_query(&query),
    );
    let bytes = match http_client::get(&url, None, 1_048_576) {
        HttpBody::Bytes(bytes) => bytes,
        HttpBody::Status(_) => return CoverFetch::Miss,
        HttpBody::Offline => return CoverFetch::Offline,
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return CoverFetch::Miss;
    };
    let ids = musicbrainz_ids(&value);
    if ids.is_empty() {
        return CoverFetch::Miss;
    }
    let mut saw_offline = false;
    for id in ids {
        let url = format!("https://coverartarchive.org/release/{id}/front-500");
        match download(&url) {
            CoverFetch::Image(bytes) => return CoverFetch::Image(bytes),
            CoverFetch::Offline => saw_offline = true,
            CoverFetch::Miss => {}
        }
    }
    if saw_offline {
        CoverFetch::Offline
    } else {
        CoverFetch::Miss
    }
}

pub(crate) fn musicbrainz_ids(value: &Value) -> Vec<String> {
    value
        .get("releases")
        .and_then(|item| item.as_array())
        .map(|releases| {
            releases
                .iter()
                .filter_map(|release| release.get("id").and_then(|id| id.as_str()).map(str::to_string))
                .take(3)
                .collect()
        })
        .unwrap_or_default()
}

fn netease_pic(artist: &str, album: &str) -> CoverFetch {
    let keyword = format!("{artist} {album}").trim().to_string();
    let url = format!(
        "https://music.163.com/api/search/get?s={}&type=10&offset=0&limit=5",
        http_client::encode_query(&keyword),
    );
    let bytes = match http_client::get(&url, Some("https://music.163.com/"), 1_048_576) {
        HttpBody::Bytes(bytes) => bytes,
        HttpBody::Status(_) => return CoverFetch::Miss,
        HttpBody::Offline => return CoverFetch::Offline,
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return CoverFetch::Miss;
    };
    let Some(url) = netease_pic_url(&value, album) else {
        return CoverFetch::Miss;
    };
    download(&url)
}

pub(crate) fn netease_pic_url(value: &Value, album: &str) -> Option<String> {
    let albums = value.pointer("/result/albums")?.as_array()?;
    let want = normalize(album);
    let mut fallback = None;
    for item in albums {
        let pic = item.get("picUrl").and_then(|url| url.as_str()).unwrap_or("").trim();
        if pic.is_empty() {
            continue;
        }
        let sized = size_netease(pic);
        if fallback.is_none() {
            fallback = Some(sized.clone());
        }
        let name = item.get("name").and_then(|name| name.as_str()).unwrap_or("");
        if want.is_empty() || normalize(name).contains(&want) || want.contains(&normalize(name)) {
            return Some(sized);
        }
    }
    fallback
}

fn size_netease(url: &str) -> String {
    let url = url.replacen("http://", "https://", 1);
    if url.contains('?') {
        url
    } else {
        format!("{url}?param=800y800")
    }
}

fn download(url: &str) -> CoverFetch {
    match http_client::get(url, None, 4_000_000) {
        HttpBody::Bytes(bytes) => match make_thumbnail(&bytes) {
            Some(jpeg) => CoverFetch::Image(jpeg),
            None => CoverFetch::Miss,
        },
        HttpBody::Status(_) => CoverFetch::Miss,
        HttpBody::Offline => CoverFetch::Offline,
    }
}

pub fn make_thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image.resize(800, 800, FilterType::Triangle);
    let mut out = Vec::new();
    image.write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg).ok()?;
    (!out.is_empty()).then_some(out)
}

fn cache_path(cache_dir: &Path, artist: &str, album: &str) -> PathBuf {
    let raw = format!("{}\n{}", normalize(artist), normalize(album));
    let digest = Sha256::digest(raw.as_bytes());
    let name: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    cache_dir.join("covers").join(format!("{name}.jpg"))
}

fn normalize(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn preferred_artist(track: &Track) -> &str {
    let album_artist = track.album_artist.trim();
    if !album_artist.is_empty() {
        album_artist
    } else {
        track.artist.trim()
    }
}

fn recent(path: &Path, ttl: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return true;
    };
    age.as_secs() < ttl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itunes_upgrades_artwork_and_matches_album() {
        let json = r#"{"results":[
            {"artistName":"Other","collectionName":"Something","artworkUrl100":"https://example.test/100x100bb.jpg"},
            {"artistName":"林深","collectionName":"夜航","artworkUrl100":"https://example.test/night/100x100bb.jpg"}
        ]}"#;
        let value: Value = serde_json::from_str(json).unwrap();
        let url = itunes_artwork(&value, "林深", "夜航").unwrap();
        assert!(url.contains("600x600bb"));
        assert!(url.contains("night"));
    }

    #[test]
    fn musicbrainz_ids_are_read() {
        let json = r#"{"releases":[{"id":"abc","title":"夜航"},{"id":"def"}]}"#;
        let value: Value = serde_json::from_str(json).unwrap();
        assert_eq!(musicbrainz_ids(&value), vec!["abc".to_string(), "def".to_string()]);
    }

    #[test]
    fn netease_pic_prefers_https_and_size() {
        let json = r#"{"result":{"albums":[{"name":"夜航","picUrl":"http://p1.music.126.net/cover.jpg"}]}}"#;
        let value: Value = serde_json::from_str(json).unwrap();
        let url = netease_pic_url(&value, "夜航").unwrap();
        assert!(url.starts_with("https://"));
        assert!(url.contains("param=800y800"));
    }

    #[test]
    fn thumbnail_shrinks_a_wide_image() {
        let image = image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(1600, 900, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])
        }));
        let mut png = Vec::new();
        image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let jpeg = make_thumbnail(&png).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert!(decoded.width() <= 800);
        assert!(decoded.height() <= 800);
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
    }
}
