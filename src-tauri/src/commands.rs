use crate::audio::AudioHandle;
use crate::cover_net;
use crate::library;
use crate::lyric_net;
use crate::lyrics;
use crate::models::{
    Album, Artist, Folder, PlaybackState, Playlist, PlaylistEntry, RepeatMode, Root, ScanReport,
    Settings, Track,
};
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub struct AppState {
    pub db_path: PathBuf,
    pub art_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub conn: Mutex<Connection>,
    pub engine: AudioHandle,
    pub scan_lock: Arc<Mutex<()>>,
}

#[tauri::command]
pub fn library_roots(state: State<'_, AppState>) -> Result<Vec<Root>, String> {
    let conn = state.conn.lock();
    library::list_roots(&conn)
}

#[tauri::command]
pub fn library_add_root(state: State<'_, AppState>, path: String) -> Result<Root, String> {
    let conn = state.conn.lock();
    library::add_root(&conn, PathBuf::from(path).as_path())
}

#[tauri::command]
pub fn library_remove_root(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.conn.lock();
    library::remove_root(&conn, id)
}

#[tauri::command]
pub fn library_scan(app: AppHandle, state: State<'_, AppState>) -> Result<ScanReport, String> {
    let _guard = state
        .scan_lock
        .try_lock()
        .ok_or_else(|| "正在扫描资料库".to_string())?;
    let db_path = state.db_path.clone();
    let art_dir = state.art_dir.clone();
    library::scan_database(&db_path, &art_dir, |progress| {
        let _ = app.emit("scan-progress", &progress);
    })
}

#[tauri::command]
pub fn library_albums(state: State<'_, AppState>) -> Result<Vec<Album>, String> {
    let conn = state.conn.lock();
    library::albums(&conn)
}

#[tauri::command]
pub fn library_album_tracks(
    state: State<'_, AppState>,
    album: String,
    artist: String,
) -> Result<Vec<Track>, String> {
    let conn = state.conn.lock();
    library::album_tracks(&conn, &state.art_dir, &album, &artist)
}

#[tauri::command]
pub fn library_artists(state: State<'_, AppState>) -> Result<Vec<Artist>, String> {
    let conn = state.conn.lock();
    library::artists(&conn)
}

#[tauri::command]
pub fn library_artist_tracks(state: State<'_, AppState>, artist: String) -> Result<Vec<Track>, String> {
    let conn = state.conn.lock();
    library::artist_tracks(&conn, &state.art_dir, &artist)
}

#[tauri::command]
pub fn library_folders(state: State<'_, AppState>) -> Result<Vec<Folder>, String> {
    let conn = state.conn.lock();
    library::folders(&conn)
}

#[tauri::command]
pub fn library_folder_tracks(state: State<'_, AppState>, folder: String) -> Result<Vec<Track>, String> {
    let conn = state.conn.lock();
    library::folder_tracks(&conn, &state.art_dir, &folder)
}

#[tauri::command]
pub fn library_tracks(state: State<'_, AppState>) -> Result<Vec<Track>, String> {
    let conn = state.conn.lock();
    library::list_tracks(&conn, &state.art_dir)
}

#[tauri::command]
pub fn library_search(state: State<'_, AppState>, query: String) -> Result<Vec<Track>, String> {
    let conn = state.conn.lock();
    library::search(&conn, &state.art_dir, &query)
}

#[tauri::command]
pub fn track_art(state: State<'_, AppState>, id: i64) -> Result<Option<String>, String> {
    let conn = state.conn.lock();
    library::track_art_data_url(&conn, &state.art_dir, id)
}

#[tauri::command]
pub fn track_lyrics(state: State<'_, AppState>, id: i64) -> Result<lyrics::Lyrics, String> {
    let track = {
        let conn = state.conn.lock();
        library::tracks_by_ids(&conn, &state.art_dir, &[id])?
            .into_iter()
            .next()
    };
    let Some(track) = track else {
        return Err("找不到这首曲目".into());
    };
    Ok(lyrics::load(&track))
}

#[tauri::command]
pub async fn lyrics_match(state: State<'_, AppState>, id: i64) -> Result<lyrics::Lyrics, String> {
    let (track, save, cache) = {
        let conn = state.conn.lock();
        let settings = library::load_settings(&conn)?;
        if !settings.auto_online_lyrics {
            return Ok(lyrics::Lyrics::empty_public());
        }
        let track = library::track_by_id(&conn, &state.art_dir, id)?.ok_or_else(|| "找不到这首曲目".to_string())?;
        (track, settings.save_lrc_beside_file, state.cache_dir.clone())
    };
    tauri::async_runtime::spawn_blocking(move || lyric_net::auto_match(&track, &cache, save))
        .await
        .map_err(|err| format!("歌词匹配中断：{err}"))
}

#[tauri::command]
pub async fn lyrics_search(state: State<'_, AppState>, id: i64) -> Result<Vec<lyric_net::LyricHit>, String> {
    let track = require_track(&state, id)?;
    let cache = state.cache_dir.clone();
    tauri::async_runtime::spawn_blocking(move || lyric_net::search(&track, &cache))
        .await
        .map_err(|err| format!("歌词搜索中断：{err}"))?
}

#[tauri::command]
pub async fn lyrics_choose(state: State<'_, AppState>, id: i64, key: String) -> Result<lyrics::Lyrics, String> {
    let (track, save, cache) = {
        let conn = state.conn.lock();
        let settings = library::load_settings(&conn)?;
        let track = library::track_by_id(&conn, &state.art_dir, id)?.ok_or_else(|| "找不到这首曲目".to_string())?;
        (track, settings.save_lrc_beside_file, state.cache_dir.clone())
    };
    tauri::async_runtime::spawn_blocking(move || lyric_net::choose(&track, &cache, &key, save))
        .await
        .map_err(|err| format!("歌词选择中断：{err}"))?
}

#[tauri::command]
pub async fn cover_ensure(state: State<'_, AppState>, id: i64) -> Result<Option<String>, String> {
    let (track, art_dir, cache) = {
        let conn = state.conn.lock();
        let settings = library::load_settings(&conn)?;
        let track = library::track_by_id(&conn, &state.art_dir, id)?.ok_or_else(|| "找不到这首曲目".to_string())?;
        if track.has_art || !settings.auto_online_cover {
            return library::track_art_data_url(&conn, &state.art_dir, id);
        }
        (track, state.art_dir.clone(), state.cache_dir.clone())
    };
    let album = track.album.clone();
    let artist = if track.album_artist.trim().is_empty() {
        track.artist.clone()
    } else {
        track.album_artist.clone()
    };
    let fetched = tauri::async_runtime::spawn_blocking(move || cover_net::fetch(&track, &cache))
        .await
        .map_err(|err| format!("封面任务中断：{err}"))?;
    let bytes = match fetched {
        cover_net::CoverFetch::Image(bytes) => bytes,
        cover_net::CoverFetch::Miss | cover_net::CoverFetch::Offline => return Ok(None),
    };
    let stored = {
        let conn = state.conn.lock();
        library::store_downloaded_art(&conn, &art_dir, &album, &artist, &bytes)?
    };
    if let Some(path) = &stored {
        let _ = state.engine.update_art(&album, &artist, path);
    }
    Ok(Some(jpeg_data_url(&bytes)))
}

fn require_track(state: &AppState, id: i64) -> Result<crate::models::Track, String> {
    let conn = state.conn.lock();
    library::track_by_id(&conn, &state.art_dir, id)?.ok_or_else(|| "找不到这首曲目".to_string())
}

fn jpeg_data_url(bytes: &[u8]) -> String {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    format!("data:image/jpeg;base64,{encoded}")
}

#[tauri::command]
pub fn playlists_list(state: State<'_, AppState>) -> Result<Vec<Playlist>, String> {
    let conn = state.conn.lock();
    library::list_playlists(&conn)
}

#[tauri::command]
pub fn playlist_create(state: State<'_, AppState>, name: String) -> Result<Playlist, String> {
    let conn = state.conn.lock();
    library::create_playlist(&conn, &name)
}

#[tauri::command]
pub fn playlist_rename(state: State<'_, AppState>, id: i64, name: String) -> Result<(), String> {
    let conn = state.conn.lock();
    library::rename_playlist(&conn, id, &name)
}

#[tauri::command]
pub fn playlist_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.conn.lock();
    library::delete_playlist(&conn, id)
}

#[tauri::command]
pub fn playlist_tracks(state: State<'_, AppState>, playlist_id: i64) -> Result<Vec<PlaylistEntry>, String> {
    let conn = state.conn.lock();
    library::playlist_tracks(&conn, &state.art_dir, playlist_id)
}

#[tauri::command]
pub fn playlist_add(state: State<'_, AppState>, playlist_id: i64, track_ids: Vec<i64>) -> Result<(), String> {
    let conn = state.conn.lock();
    library::playlist_add(&conn, playlist_id, &track_ids)
}

#[tauri::command]
pub fn playlist_remove_item(state: State<'_, AppState>, item_id: i64) -> Result<(), String> {
    let conn = state.conn.lock();
    library::playlist_remove_item(&conn, item_id)
}

#[tauri::command]
pub fn output_devices() -> Vec<crate::models::OutputDevice> {
    crate::audio::list_output_devices()
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Result<Settings, String> {
    let conn = state.conn.lock();
    library::load_settings(&conn)
}

#[tauri::command]
pub fn settings_set(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    {
        let conn = state.conn.lock();
        library::save_settings(&conn, &settings)?;
    }
    state.engine.apply_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
pub fn playback_state(state: State<'_, AppState>) -> PlaybackState {
    state.engine.snapshot()
}

#[tauri::command]
pub fn playback_set_queue(state: State<'_, AppState>, ids: Vec<i64>, index: usize) -> Result<PlaybackState, String> {
    let tracks = load_tracks(&state, &ids)?;
    if tracks.is_empty() {
        return Err("没有可播放的曲目".into());
    }
    state.engine.set_queue(tracks, index)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_toggle(state: State<'_, AppState>) -> Result<PlaybackState, String> {
    state.engine.toggle()?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_play(state: State<'_, AppState>) -> Result<PlaybackState, String> {
    state.engine.play()?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_pause(state: State<'_, AppState>) -> Result<PlaybackState, String> {
    state.engine.pause()?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_next(state: State<'_, AppState>) -> Result<PlaybackState, String> {
    state.engine.next()?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_prev(state: State<'_, AppState>) -> Result<PlaybackState, String> {
    state.engine.prev()?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_seek(state: State<'_, AppState>, position_ms: u64) -> Result<PlaybackState, String> {
    state.engine.seek(position_ms)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_volume(state: State<'_, AppState>, volume: f32) -> Result<PlaybackState, String> {
    state.engine.volume(volume)?;
    let mut settings = {
        let conn = state.conn.lock();
        library::load_settings(&conn)?
    };
    settings.volume = volume.clamp(0.0, 1.0);
    let conn = state.conn.lock();
    library::save_settings(&conn, &settings)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_shuffle(state: State<'_, AppState>, shuffle: bool) -> Result<PlaybackState, String> {
    state.engine.shuffle(shuffle)?;
    persist_transport(&state)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_repeat(state: State<'_, AppState>, repeat_mode: RepeatMode) -> Result<PlaybackState, String> {
    state.engine.repeat(repeat_mode)?;
    persist_transport(&state)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_enqueue(state: State<'_, AppState>, ids: Vec<i64>) -> Result<PlaybackState, String> {
    let tracks = load_tracks(&state, &ids)?;
    state.engine.enqueue(tracks)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_play_next(state: State<'_, AppState>, id: i64) -> Result<PlaybackState, String> {
    let tracks = load_tracks(&state, &[id])?;
    let Some(track) = tracks.into_iter().next() else {
        return Err("找不到这首曲目".into());
    };
    state.engine.play_next(track)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_jump(state: State<'_, AppState>, index: usize) -> Result<PlaybackState, String> {
    state.engine.jump(index)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_remove(state: State<'_, AppState>, index: usize) -> Result<PlaybackState, String> {
    state.engine.remove(index)?;
    Ok(state.engine.snapshot())
}

#[tauri::command]
pub fn playback_clear(state: State<'_, AppState>) -> Result<PlaybackState, String> {
    state.engine.clear()?;
    Ok(state.engine.snapshot())
}

fn load_tracks(state: &AppState, ids: &[i64]) -> Result<Vec<Track>, String> {
    let conn = state.conn.lock();
    library::tracks_by_ids(&conn, &state.art_dir, ids)
}

fn persist_transport(state: &AppState) -> Result<(), String> {
    let snap = state.engine.snapshot();
    let mut settings = {
        let conn = state.conn.lock();
        library::load_settings(&conn)?
    };
    settings.shuffle = snap.shuffle;
    settings.repeat_mode = snap.repeat_mode;
    settings.volume = snap.volume;
    let conn = state.conn.lock();
    library::save_settings(&conn, &settings)
}
