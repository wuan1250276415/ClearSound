//! SQLite library. Album art is stored as files under the app data directory,
//! addressed by a short SHA-256 prefix.

use crate::cue::{parse_cue, resolve_cue_audio};
use crate::metadata::{folder_cover, read_tags};
use crate::models::{
    Album, Artist, Folder, Playlist, PlaylistEntry, RepeatMode, Root, ScanProgress, ScanReport,
    Settings, Track,
};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const AUDIO_EXTS: &[&str] = &[
    "flac", "mp3", "wav", "wave", "dsf", "dff", "ape", "m4a", "aac", "alac",
];

const TRACK_LIMIT: u32 = 4000;

pub fn open_db(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("无法创建资料库目录：{err}"))?;
    }
    let conn = Connection::open(path).map_err(|err| format!("无法打开资料库：{err}"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|err| format!("无法初始化资料库：{err}"))?;
    conn.execute_batch(SCHEMA)
        .map_err(|err| format!("无法创建资料库表：{err}"))?;
    Ok(conn)
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS roots (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY,
    root_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    cue_path TEXT NOT NULL DEFAULT '',
    cue_index INTEGER NOT NULL DEFAULT -1,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album_artist TEXT NOT NULL,
    album TEXT NOT NULL,
    track_number INTEGER,
    disc_number INTEGER,
    year TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL,
    sample_rate INTEGER,
    bit_depth INTEGER,
    channels INTEGER,
    format TEXT NOT NULL,
    folder TEXT NOT NULL,
    art_hash TEXT,
    art_mime TEXT,
    mtime INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    cue_mtime INTEGER NOT NULL DEFAULT 0,
    UNIQUE(path, cue_path, cue_index)
);
CREATE TABLE IF NOT EXISTS playlists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS playlist_items (
    id INTEGER PRIMARY KEY,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tracks_root ON tracks(root_id);
CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_playlist_items_pos ON playlist_items(playlist_id, position);
"#;

pub fn list_roots(conn: &Connection) -> Result<Vec<Root>, String> {
    let mut stmt = conn
        .prepare("SELECT id, path FROM roots ORDER BY path")
        .map_err(sql_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Root {
                id: row.get(0)?,
                path: row.get(1)?,
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

pub fn add_root(conn: &Connection, path: &Path) -> Result<Root, String> {
    if !path.is_dir() {
        return Err("请选择一个文件夹".into());
    }
    let text = normalize_path(path);
    conn.execute(
        "INSERT INTO roots(path) VALUES (?1) ON CONFLICT(path) DO NOTHING",
        params![text],
    )
    .map_err(sql_err)?;
    let id: i64 = conn
        .query_row("SELECT id FROM roots WHERE path = ?1", params![text], |row| {
            row.get(0)
        })
        .map_err(sql_err)?;
    Ok(Root { id, path: text })
}

pub fn remove_root(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM tracks WHERE root_id = ?1", params![id])
        .map_err(sql_err)?;
    conn.execute("DELETE FROM roots WHERE id = ?1", params![id])
        .map_err(sql_err)?;
    Ok(())
}

pub fn scan_database(
    db_path: &Path,
    art_dir: &Path,
    progress: impl Fn(ScanProgress),
) -> Result<ScanReport, String> {
    std::fs::create_dir_all(art_dir).map_err(|err| format!("无法创建封面目录：{err}"))?;
    let conn = open_db(db_path)?;
    let roots = list_roots(&conn)?;
    let mut report = ScanReport::default();
    for root in roots {
        let root_path = PathBuf::from(&root.path);
        if !root_path.is_dir() {
            report.errors.push(format!("文件夹不存在：{}", root.path));
            continue;
        }
        let part = scan_root(&conn, root.id, &root_path, art_dir, &progress)?;
        report.merge(part);
    }
    Ok(report)
}

fn scan_root(
    conn: &Connection,
    root_id: i64,
    root_path: &Path,
    art_dir: &Path,
    progress: &impl Fn(ScanProgress),
) -> Result<ScanReport, String> {
    let (audio, cues) = collect_files(root_path);
    let total = (audio.len() + cues.len()) as u32;
    let mut scanned = 0u32;
    let mut report = ScanReport::default();
    let mut seen: HashSet<(String, String, i64)> = HashSet::new();
    let mut consumed: HashSet<String> = HashSet::new();
    let mut covers: HashMap<PathBuf, Option<(String, String)>> = HashMap::new();

    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql_err)?;
    let outcome = (|| {
        for cue_path in &cues {
            scanned += 1;
            progress(ScanProgress {
                scanned,
                total,
                current: cue_path.to_string_lossy().to_string(),
            });
            if let Err(err) = ingest_cue(
                conn,
                root_id,
                cue_path,
                art_dir,
                &mut seen,
                &mut consumed,
                &mut covers,
                &mut report,
            ) {
                push_error(&mut report, err);
            }
        }
        for audio_path in &audio {
            scanned += 1;
            progress(ScanProgress {
                scanned,
                total,
                current: audio_path.to_string_lossy().to_string(),
            });
            let key = normalize_path(audio_path);
            if consumed.contains(&key) {
                continue;
            }
            if let Err(err) = ingest_file(
                conn,
                root_id,
                audio_path,
                "",
                -1,
                0,
                None,
                None,
                None,
                None,
                0,
                art_dir,
                &mut seen,
                &mut covers,
                &mut report,
            ) {
                push_error(&mut report, err);
            }
        }
        remove_missing(conn, root_id, &seen, &mut report)?;
        Ok(())
    })();
    match outcome {
        Ok(()) => {
            conn.execute_batch("COMMIT").map_err(sql_err)?;
            Ok(report)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

fn ingest_cue(
    conn: &Connection,
    root_id: i64,
    cue_path: &Path,
    art_dir: &Path,
    seen: &mut HashSet<(String, String, i64)>,
    consumed: &mut HashSet<String>,
    covers: &mut HashMap<PathBuf, Option<(String, String)>>,
    report: &mut ScanReport,
) -> Result<(), String> {
    let bytes = std::fs::read(cue_path).map_err(|err| format!("无法读取 CUE：{err}"))?;
    let sheet = parse_cue(&bytes)?;
    let cue_dir = cue_path.parent().unwrap_or(Path::new("."));
    let cue_key = normalize_path(cue_path);
    let (cue_mtime, _) = file_stat(cue_path);
    for file in sheet.files {
        let Some(audio) = resolve_cue_audio(cue_dir, &file.name) else {
            push_error(report, format!("CUE 找不到音频文件：{}", file.name));
            continue;
        };
        consumed.insert(normalize_path(&audio));
        for cue_track in file.tracks {
            let performer = cue_track
                .performer
                .clone()
                .or_else(|| sheet.performer.clone());
            ingest_file(
                conn,
                root_id,
                &audio,
                &cue_key,
                i64::from(cue_track.number),
                cue_track.start_ms,
                cue_track.end_ms,
                cue_track.title.clone(),
                performer,
                sheet.title.clone(),
                cue_mtime,
                art_dir,
                seen,
                covers,
                report,
            )?;
        }
    }
    let _ = sheet.year;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ingest_file(
    conn: &Connection,
    root_id: i64,
    audio: &Path,
    cue_path: &str,
    cue_index: i64,
    start_ms: u64,
    end_ms: Option<u64>,
    cue_title: Option<String>,
    cue_artist: Option<String>,
    cue_album: Option<String>,
    cue_mtime: i64,
    art_dir: &Path,
    seen: &mut HashSet<(String, String, i64)>,
    covers: &mut HashMap<PathBuf, Option<(String, String)>>,
    report: &mut ScanReport,
) -> Result<(), String> {
    let path = normalize_path(audio);
    let key = (path.clone(), cue_path.to_string(), cue_index);
    let (mtime, size) = file_stat(audio);
    if let Some((old_mtime, old_size, old_cue)) = existing_stat(conn, &key.0, &key.1, key.2)? {
        if old_mtime == mtime && old_size == size && old_cue == cue_mtime {
            seen.insert(key);
            report.skipped += 1;
            return Ok(());
        }
    }
    let tags = read_tags(audio);
    let parent = audio.parent().unwrap_or(Path::new("."));
    let art = art_for(parent, &tags, art_dir, covers);
    let file_duration = tags.duration_ms;
    let duration_ms = match end_ms {
        Some(end) => end.saturating_sub(start_ms),
        None => file_duration.saturating_sub(start_ms),
    };
    let title = nonempty(cue_title)
        .or_else(|| tags.title.clone())
        .unwrap_or_else(|| {
            if cue_index >= 0 {
                format!("音轨 {}", cue_index)
            } else {
                audio
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "未命名".into())
            }
        });
    let artist = nonempty(cue_artist)
        .or_else(|| tags.artist.clone())
        .unwrap_or_else(|| "未知艺术家".into());
    let album = nonempty(cue_album)
        .or_else(|| tags.album.clone())
        .or_else(|| parent.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "未知专辑".into());
    let album_artist = tags
        .album_artist
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| artist.clone());
    let year = tags.year.clone().unwrap_or_default();
    let existed = existing_stat(conn, &key.0, &key.1, key.2)?.is_some();
    conn.execute(
        "INSERT INTO tracks (
            root_id, path, cue_path, cue_index, start_ms, end_ms, title, artist, album_artist,
            album, track_number, disc_number, year, duration_ms, sample_rate, bit_depth, channels,
            format, folder, art_hash, art_mime, mtime, file_size, cue_mtime
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
            ?20, ?21, ?22, ?23, ?24
        )
        ON CONFLICT(path, cue_path, cue_index) DO UPDATE SET
            root_id = excluded.root_id,
            start_ms = excluded.start_ms,
            end_ms = excluded.end_ms,
            title = excluded.title,
            artist = excluded.artist,
            album_artist = excluded.album_artist,
            album = excluded.album,
            track_number = excluded.track_number,
            disc_number = excluded.disc_number,
            year = excluded.year,
            duration_ms = excluded.duration_ms,
            sample_rate = excluded.sample_rate,
            bit_depth = excluded.bit_depth,
            channels = excluded.channels,
            format = excluded.format,
            folder = excluded.folder,
            art_hash = excluded.art_hash,
            art_mime = excluded.art_mime,
            mtime = excluded.mtime,
            file_size = excluded.file_size,
            cue_mtime = excluded.cue_mtime",
        params![
            root_id,
            path,
            cue_path,
            cue_index,
            start_ms as i64,
            end_ms.map(|v| v as i64),
            title,
            artist,
            album_artist,
            album,
            if cue_index >= 0 {
                Some(cue_index)
            } else {
                tags.track_number.map(i64::from)
            },
            tags.disc_number.map(i64::from),
            year,
            duration_ms as i64,
            tags.sample_rate.map(i64::from),
            tags.bit_depth.map(i64::from),
            tags.channels.map(i64::from),
            format_of(audio),
            normalize_path(parent),
            art.as_ref().map(|(hash, _)| hash.clone()),
            art.as_ref().map(|(_, mime)| mime.clone()),
            mtime,
            size,
            cue_mtime,
        ],
    )
    .map_err(sql_err)?;
    seen.insert(key);
    if existed {
        report.updated += 1;
    } else {
        report.added += 1;
    }
    Ok(())
}

fn remove_missing(
    conn: &Connection,
    root_id: i64,
    seen: &HashSet<(String, String, i64)>,
    report: &mut ScanReport,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT path, cue_path, cue_index FROM tracks WHERE root_id = ?1")
        .map_err(sql_err)?;
    let rows = stmt
        .query_map(params![root_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(sql_err)?;
    let mut doomed = Vec::new();
    for row in rows {
        let key = row.map_err(sql_err)?;
        if !seen.contains(&key) {
            doomed.push(key);
        }
    }
    for (path, cue_path, cue_index) in doomed {
        conn.execute(
            "DELETE FROM tracks WHERE path = ?1 AND cue_path = ?2 AND cue_index = ?3",
            params![path, cue_path, cue_index],
        )
        .map_err(sql_err)?;
        report.removed += 1;
    }
    Ok(())
}

fn existing_stat(
    conn: &Connection,
    path: &str,
    cue_path: &str,
    cue_index: i64,
) -> Result<Option<(i64, i64, i64)>, String> {
    conn.query_row(
        "SELECT mtime, file_size, cue_mtime FROM tracks WHERE path = ?1 AND cue_path = ?2 AND cue_index = ?3",
        params![path, cue_path, cue_index],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(sql_err)
}

pub fn albums(conn: &Connection) -> Result<Vec<Album>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT album,
                    CASE WHEN album_artist != '' THEN album_artist ELSE artist END,
                    COUNT(*),
                    COALESCE(SUM(duration_ms), 0),
                    MIN(id),
                    MAX(year)
             FROM tracks
             GROUP BY album, CASE WHEN album_artist != '' THEN album_artist ELSE artist END
             ORDER BY 2, 1",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Album {
                name: row.get(0)?,
                artist: row.get(1)?,
                track_count: row.get::<_, i64>(2)? as u32,
                duration_ms: row.get::<_, i64>(3)? as u64,
                art_track_id: row.get(4)?,
                year: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

pub fn album_tracks(conn: &Connection, art_dir: &Path, album: &str, artist: &str) -> Result<Vec<Track>, String> {
    query_tracks(
        conn,
        art_dir,
        "SELECT * FROM tracks
         WHERE album = ?1
           AND CASE WHEN album_artist != '' THEN album_artist ELSE artist END = ?2
         ORDER BY disc_number, track_number, title",
        params![album, artist],
    )
}

pub fn artists(conn: &Connection) -> Result<Vec<Artist>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT artist, COUNT(DISTINCT album), COUNT(*)
             FROM tracks GROUP BY artist ORDER BY artist",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Artist {
                name: row.get(0)?,
                album_count: row.get::<_, i64>(1)? as u32,
                track_count: row.get::<_, i64>(2)? as u32,
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

pub fn artist_tracks(conn: &Connection, art_dir: &Path, artist: &str) -> Result<Vec<Track>, String> {
    query_tracks(
        conn,
        art_dir,
        "SELECT * FROM tracks WHERE artist = ?1
         ORDER BY album, disc_number, track_number, title",
        params![artist],
    )
}

pub fn folders(conn: &Connection) -> Result<Vec<Folder>, String> {
    let mut stmt = conn
        .prepare("SELECT folder, COUNT(*) FROM tracks GROUP BY folder ORDER BY folder")
        .map_err(sql_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Folder {
                path: row.get(0)?,
                track_count: row.get::<_, i64>(1)? as u32,
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

pub fn folder_tracks(conn: &Connection, art_dir: &Path, folder: &str) -> Result<Vec<Track>, String> {
    query_tracks(
        conn,
        art_dir,
        "SELECT * FROM tracks WHERE folder = ?1 ORDER BY disc_number, track_number, title",
        params![folder],
    )
}

pub fn list_tracks(conn: &Connection, art_dir: &Path) -> Result<Vec<Track>, String> {
    query_tracks(
        conn,
        art_dir,
        "SELECT * FROM tracks ORDER BY artist, album, disc_number, track_number, title LIMIT ?1",
        params![TRACK_LIMIT],
    )
}

pub fn search(conn: &Connection, art_dir: &Path, query: &str) -> Result<Vec<Track>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = like_pattern(query);
    query_tracks(
        conn,
        art_dir,
        "SELECT * FROM tracks
         WHERE title LIKE ?1 ESCAPE '\\'
            OR artist LIKE ?1 ESCAPE '\\'
            OR album LIKE ?1 ESCAPE '\\'
            OR album_artist LIKE ?1 ESCAPE '\\'
         ORDER BY artist, album, disc_number, track_number, title
         LIMIT 300",
        params![pattern],
    )
}

pub fn tracks_by_ids(conn: &Connection, art_dir: &Path, ids: &[i64]) -> Result<Vec<Track>, String> {
    let mut tracks = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(track) = track_by_id(conn, art_dir, *id)? {
            tracks.push(track);
        }
    }
    Ok(tracks)
}

pub fn track_by_id(conn: &Connection, art_dir: &Path, id: i64) -> Result<Option<Track>, String> {
    let mut tracks = query_tracks(
        conn,
        art_dir,
        "SELECT * FROM tracks WHERE id = ?1",
        params![id],
    )?;
    Ok(tracks.pop())
}

pub fn track_art_data_url(conn: &Connection, art_dir: &Path, id: i64) -> Result<Option<String>, String> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT art_hash, art_mime FROM tracks WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_err)?;
    let Some((Some(hash), mime)) = row else {
        return Ok(None);
    };
    let bytes = match std::fs::read(art_dir.join(&hash)) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let mime = mime.unwrap_or_else(|| "image/jpeg".into());
    Ok(Some(format!("data:{mime};base64,{encoded}")))
}

pub fn list_playlists(conn: &Connection) -> Result<Vec<Playlist>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, p.created_at, COUNT(i.id)
             FROM playlists p
             LEFT JOIN playlist_items i ON i.playlist_id = p.id
             GROUP BY p.id
             ORDER BY p.created_at, p.id",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                track_count: row.get::<_, i64>(3)? as u32,
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

pub fn create_playlist(conn: &Connection, name: &str) -> Result<Playlist, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("播放列表名称不能为空".into());
    }
    let created_at = unix_now();
    conn.execute(
        "INSERT INTO playlists(name, created_at) VALUES (?1, ?2)",
        params![name, created_at],
    )
    .map_err(sql_err)?;
    Ok(Playlist {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        track_count: 0,
        created_at,
    })
}

pub fn rename_playlist(conn: &Connection, id: i64, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("播放列表名称不能为空".into());
    }
    let changed = conn
        .execute(
            "UPDATE playlists SET name = ?1 WHERE id = ?2",
            params![name, id],
        )
        .map_err(sql_err)?;
    if changed == 0 {
        return Err("找不到这个播放列表".into());
    }
    Ok(())
}

pub fn delete_playlist(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", params![id])
        .map_err(sql_err)?;
    Ok(())
}

pub fn playlist_tracks(conn: &Connection, art_dir: &Path, playlist_id: i64) -> Result<Vec<PlaylistEntry>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT i.id, i.position, t.*
             FROM playlist_items i
             JOIN tracks t ON t.id = i.track_id
             WHERE i.playlist_id = ?1
             ORDER BY i.position, i.id",
        )
        .map_err(sql_err)?;
    let rows = stmt
        .query_map(params![playlist_id], |row| {
            Ok(PlaylistEntry {
                item_id: row.get(0)?,
                position: row.get(1)?,
                track: map_track_at(row, 2, Some(art_dir))?,
            })
        })
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

pub fn playlist_add(conn: &Connection, playlist_id: i64, track_ids: &[i64]) -> Result<(), String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM playlists WHERE id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;
    if exists.is_none() {
        return Err("找不到这个播放列表".into());
    }
    let mut position: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), 0) FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    for track_id in track_ids {
        let known: Option<i64> = conn
            .query_row("SELECT id FROM tracks WHERE id = ?1", params![track_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql_err)?;
        if known.is_none() {
            continue;
        }
        position += 1;
        conn.execute(
            "INSERT INTO playlist_items(playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
            params![playlist_id, track_id, position],
        )
        .map_err(sql_err)?;
    }
    Ok(())
}

pub fn playlist_remove_item(conn: &Connection, item_id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM playlist_items WHERE id = ?1", params![item_id])
        .map_err(sql_err)?;
    Ok(())
}

pub fn load_settings(conn: &Connection) -> Result<Settings, String> {
    let mut settings = Settings::default();
    if let Some(value) = setting(conn, "volume")? {
        if let Ok(volume) = value.parse::<f32>() {
            settings.volume = volume.clamp(0.0, 1.0);
        }
    }
    if let Some(value) = setting(conn, "shuffle")? {
        settings.shuffle = value == "1";
    }
    if let Some(value) = setting(conn, "repeat")? {
        settings.repeat_mode = RepeatMode::from_str(&value);
    }
    if let Some(value) = setting(conn, "auto_lyrics")? {
        settings.auto_online_lyrics = value != "0";
    }
    if let Some(value) = setting(conn, "auto_cover")? {
        settings.auto_online_cover = value != "0";
    }
    if let Some(value) = setting(conn, "save_lrc")? {
        settings.save_lrc_beside_file = value == "1";
    }
    if let Some(value) = setting(conn, "output_device")? {
        settings.output_device_id = value;
    }
    if let Some(value) = setting(conn, "output_mode")? {
        settings.output_mode = if value == "exclusive" {
            crate::models::OutputMode::Exclusive
        } else {
            crate::models::OutputMode::Shared
        };
    }
    if let Some(value) = setting(conn, "volume_lock")? {
        settings.exclusive_volume_lock = value == "1";
    }
    if let Some(value) = setting(conn, "dop")? {
        settings.dop = value == "1";
    }
    Ok(settings)
}

pub fn save_settings(conn: &Connection, settings: &Settings) -> Result<(), String> {
    put_setting(conn, "volume", &format!("{:.4}", settings.volume))?;
    put_setting(conn, "shuffle", if settings.shuffle { "1" } else { "0" })?;
    put_setting(conn, "repeat", settings.repeat_mode.as_str())?;
    put_setting(conn, "auto_lyrics", if settings.auto_online_lyrics { "1" } else { "0" })?;
    put_setting(conn, "auto_cover", if settings.auto_online_cover { "1" } else { "0" })?;
    put_setting(conn, "save_lrc", if settings.save_lrc_beside_file { "1" } else { "0" })?;
    put_setting(conn, "output_device", &settings.output_device_id)?;
    put_setting(
        conn,
        "output_mode",
        if settings.output_mode == crate::models::OutputMode::Exclusive {
            "exclusive"
        } else {
            "shared"
        },
    )?;
    put_setting(
        conn,
        "volume_lock",
        if settings.exclusive_volume_lock { "1" } else { "0" },
    )?;
    put_setting(conn, "dop", if settings.dop { "1" } else { "0" })?;
    Ok(())
}

pub fn store_downloaded_art(
    conn: &Connection,
    art_dir: &Path,
    album: &str,
    artist: &str,
    bytes: &[u8],
) -> Result<Option<String>, String> {
    let Some((hash, _)) = store_art(art_dir, "image/jpeg", bytes) else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE tracks SET art_hash = ?1, art_mime = ?2
         WHERE album = ?3 AND (artist = ?4 OR album_artist = ?4) AND art_hash IS NULL",
        params![hash, "image/jpeg", album, artist],
    )
    .map_err(sql_err)?;
    Ok(Some(art_dir.join(&hash).to_string_lossy().into_owned()))
}

fn setting(conn: &Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql_err)
}

fn put_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn query_tracks(
    conn: &Connection,
    art_dir: &Path,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Track>, String> {
    let mut stmt = conn.prepare(sql).map_err(sql_err)?;
    let rows = stmt
        .query_map(params, |row| map_track_at(row, 0, Some(art_dir)))
        .map_err(sql_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

fn map_track_at(row: &rusqlite::Row<'_>, offset: usize, art_dir: Option<&Path>) -> rusqlite::Result<Track> {
    let cue_path: String = row.get(offset + 3)?;
    let art_hash: Option<String> = row.get(offset + 20)?;
    let art_path = art_hash.as_ref().and_then(|hash| {
        let path = art_dir?.join(hash);
        path.is_file().then(|| path.to_string_lossy().into_owned())
    });
    Ok(Track {
        id: row.get(offset)?,
        path: row.get(offset + 2)?,
        cue_path: if cue_path.is_empty() { None } else { Some(cue_path) },
        start_ms: row.get::<_, i64>(offset + 5)? as u64,
        end_ms: row.get::<_, Option<i64>>(offset + 6)?.map(|value| value as u64),
        title: row.get(offset + 7)?,
        artist: row.get(offset + 8)?,
        album_artist: row.get(offset + 9)?,
        album: row.get(offset + 10)?,
        track_number: row.get::<_, Option<i64>>(offset + 11)?.map(|value| value as u32),
        disc_number: row.get::<_, Option<i64>>(offset + 12)?.map(|value| value as u32),
        year: row.get(offset + 13)?,
        duration_ms: row.get::<_, i64>(offset + 14)? as u64,
        sample_rate: row.get::<_, Option<i64>>(offset + 15)?.map(|value| value as u32),
        bit_depth: row.get::<_, Option<i64>>(offset + 16)?.map(|value| value as u32),
        channels: row.get::<_, Option<i64>>(offset + 17)?.map(|value| value as u32),
        format: row.get(offset + 18)?,
        folder: row.get(offset + 19)?,
        has_art: art_hash.is_some(),
        art_path,
    })
}

fn collect_files(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut audio = Vec::new();
    let mut cues = Vec::new();
    for entry in WalkDir::new(root).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if skipped_dir(entry.path()) {
            continue;
        }
        let Some(ext) = entry.path().extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if ext.eq_ignore_ascii_case("cue") {
            cues.push(entry.path().to_path_buf());
        } else if AUDIO_EXTS.iter().any(|known| ext.eq_ignore_ascii_case(known)) {
            audio.push(entry.path().to_path_buf());
        }
    }
    (audio, cues)
}

fn skipped_dir(path: &Path) -> bool {
    path.components().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        matches!(
            text.as_ref(),
            "$RECYCLE.BIN" | "System Volume Information" | "node_modules" | ".git"
        )
    })
}

fn art_for(
    folder: &Path,
    tags: &crate::metadata::FileTags,
    art_dir: &Path,
    covers: &mut HashMap<PathBuf, Option<(String, String)>>,
) -> Option<(String, String)> {
    if let Some((mime, bytes)) = &tags.art {
        if let Some(stored) = store_art(art_dir, mime, bytes) {
            return Some(stored);
        }
    }
    if let Some(cached) = covers.get(folder) {
        return cached.clone();
    }
    let stored = folder_cover(folder).and_then(|(mime, bytes)| store_art(art_dir, &mime, &bytes));
    covers.insert(folder.to_path_buf(), stored.clone());
    stored
}

fn store_art(art_dir: &Path, mime: &str, bytes: &[u8]) -> Option<(String, String)> {
    if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
        return None;
    }
    let digest = Sha256::digest(bytes);
    let hash: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    let dest = art_dir.join(&hash);
    if !dest.is_file() {
        std::fs::write(&dest, bytes).ok()?;
    }
    Some((hash, mime.to_string()))
}

fn file_stat(path: &Path) -> (i64, i64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    (mtime, meta.len() as i64)
}

fn format_of(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "flac" => "FLAC".to_string(),
        "mp3" => "MP3".to_string(),
        "wav" | "wave" => "WAV".to_string(),
        "dsf" => "DSF".to_string(),
        "dff" => "DFF".to_string(),
        "ape" => "APE".to_string(),
        "m4a" | "aac" | "alac" => "M4A".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

fn normalize_path(path: &Path) -> String {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let text = canon.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn like_pattern(query: &str) -> String {
    let mut pattern = String::from('%');
    for ch in query.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn push_error(report: &mut ScanReport, message: String) {
    if report.errors.len() < 40 {
        report.errors.push(message);
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn sql_err(err: rusqlite::Error) -> String {
    format!("资料库错误：{err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, ms: u32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let frames = 8 * ms;
        for index in 0..frames {
            let sample = ((index as f32) * 0.05).sin();
            writer.write_sample((sample * 4000.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn scan_splits_cue_and_keeps_loose_files() {
        let dir = std::env::temp_dir().join(format!(
            "chengyin-lib-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_wav(&dir.join("album.wav"), 2000);
        write_wav(&dir.join("single.wav"), 500);
        let cue = "PERFORMER \"林深\"\nTITLE \"夜航\"\nFILE \"album.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"启程\"\n    PERFORMER \"林深\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"靠岸\"\n    PERFORMER \"林深\"\n    INDEX 01 00:00:75\n";
        std::fs::write(dir.join("album.cue"), cue).unwrap();
        let db = dir.join("library.db");
        let art = dir.join("art");
        let conn = open_db(&db).unwrap();
        add_root(&conn, &dir).unwrap();
        let report = scan_database(&db, &art, |_| {}).unwrap();
        assert_eq!(report.added, 3, "{report:?}");
        let tracks = list_tracks(&conn, &art).unwrap();
        assert_eq!(tracks.len(), 3);
        let titles: Vec<_> = tracks.iter().map(|track| track.title.as_str()).collect();
        assert!(titles.contains(&"启程"));
        assert!(titles.contains(&"靠岸"));
        assert!(titles.contains(&"single"));
        let second = tracks.iter().find(|track| track.title == "靠岸").unwrap();
        assert_eq!(second.start_ms, 1000);
        assert_eq!(second.album, "夜航");
        assert!(tracks.iter().all(|track| track.title != "album"));
        let again = scan_database(&db, &art, |_| {}).unwrap();
        assert_eq!(again.skipped, 3);
        assert_eq!(again.added, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
