mod audio;
mod commands;
mod cue;
mod ffmpeg_bin;
mod library;
mod lyrics;
mod metadata;
mod models;
mod smtc;

use commands::AppState;
use parking_lot::Mutex;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Err(err) = setup(app) {
                return Err(Box::new(std::io::Error::other(err)));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::library_roots,
            commands::library_add_root,
            commands::library_remove_root,
            commands::library_scan,
            commands::library_albums,
            commands::library_album_tracks,
            commands::library_artists,
            commands::library_artist_tracks,
            commands::library_folders,
            commands::library_folder_tracks,
            commands::library_tracks,
            commands::library_search,
            commands::track_art,
            commands::track_lyrics,
            commands::playlists_list,
            commands::playlist_create,
            commands::playlist_rename,
            commands::playlist_delete,
            commands::playlist_tracks,
            commands::playlist_add,
            commands::playlist_remove_item,
            commands::settings_get,
            commands::settings_set,
            commands::playback_state,
            commands::playback_set_queue,
            commands::playback_toggle,
            commands::playback_play,
            commands::playback_pause,
            commands::playback_next,
            commands::playback_prev,
            commands::playback_seek,
            commands::playback_volume,
            commands::playback_shuffle,
            commands::playback_repeat,
            commands::playback_enqueue,
            commands::playback_play_next,
            commands::playback_jump,
            commands::playback_remove,
            commands::playback_clear,
        ])
        .run(tauri::generate_context!())
        .expect("澄音启动失败");
}

fn setup(app: &mut tauri::App) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("无法定位数据目录：{err}"))?;
    std::fs::create_dir_all(&dir).map_err(|err| format!("无法创建数据目录：{err}"))?;
    let art_dir = dir.join("art");
    std::fs::create_dir_all(&art_dir).map_err(|err| format!("无法创建封面目录：{err}"))?;
    let db_path = dir.join("library.db");
    let conn = library::open_db(&db_path)?;
    let settings = library::load_settings(&conn)?;
    let resource = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()));
    let ffmpeg = ffmpeg_bin::resolve(resource.as_deref(), exe_dir.as_deref());
    let engine = audio::AudioHandle::start(ffmpeg);
    engine.apply_settings(&settings)?;
    smtc::start(engine.clone(), native_hwnd(app));
    app.manage(AppState {
        db_path,
        art_dir,
        conn: Mutex::new(conn),
        engine,
        scan_lock: Arc::new(Mutex::new(())),
    });
    Ok(())
}

fn native_hwnd(app: &tauri::App) -> Option<*mut std::ffi::c_void> {
    #[cfg(windows)]
    {
        app.get_webview_window("main")
            .and_then(|window| window.hwnd().ok())
            .map(|hwnd| hwnd.0 as *mut std::ffi::c_void)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        None
    }
}
