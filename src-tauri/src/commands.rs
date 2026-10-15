use crate::audio::AudioHandle;
use crate::library;
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
