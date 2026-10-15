//! Windows system media transport controls. Media keys and the volume-OSD
//! card call back into the player. Other platforms leave this as a no-op.

use crate::audio::AudioHandle;
#[cfg(windows)]
use std::time::Duration;

pub fn start(engine: AudioHandle, hwnd: Option<*mut std::ffi::c_void>) {
    #[cfg(windows)]
    {
        let Some(hwnd) = hwnd else {
            return;
        };
        // Raw pointers are not Send. The handle is only an address the media
        // thread stores; the window stays owned by the UI thread.
        let hwnd_addr = hwnd as usize;
        let _ = std::thread::Builder::new()
            .name("chengyin-smtc".into())
            .spawn(move || windows_loop(engine, Some(hwnd_addr as *mut std::ffi::c_void)));
    }
    #[cfg(not(windows))]
    {
        let _ = (engine, hwnd);
    }
}

#[cfg(windows)]
fn windows_loop(engine: AudioHandle, hwnd: Option<*mut std::ffi::c_void>) {
    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

    let config = PlatformConfig {
        dbus_name: "chengyin",
        display_name: "澄音",
        hwnd,
    };
    let Ok(mut controls) = MediaControls::new(config) else {
        return;
    };
    let hooked = engine.clone();
    if controls
        .attach(move |event| handle_event(&hooked, event))
        .is_err()
    {
        return;
    }
    let mut shown_id: Option<i64> = None;
    loop {
        let state = engine.snapshot();
        let progress = Some(MediaPosition(Duration::from_millis(state.position_ms)));
        let playback = if state.current.is_none() {
            MediaPlayback::Stopped
        } else if state.playing {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        };
        let _ = controls.set_playback(playback);
        let current_id = state.current.as_ref().map(|track| track.id);
        if current_id != shown_id {
            shown_id = current_id;
            if let Some(track) = state.current.as_ref() {
                let cover = track.art_path.as_deref().map(file_url);
                let metadata = MediaMetadata {
                    title: Some(track.title.as_str()),
                    artist: Some(track.artist.as_str()),
                    album: Some(track.album.as_str()),
                    cover_url: cover.as_deref(),
                    duration: Some(Duration::from_millis(track.duration_ms)),
                };
                let _ = controls.set_metadata(metadata);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    fn handle_event(engine: &AudioHandle, event: MediaControlEvent) {
        let _ = match event {
            MediaControlEvent::Play => engine.play(),
            MediaControlEvent::Pause | MediaControlEvent::Stop => engine.pause(),
            MediaControlEvent::Toggle => engine.toggle(),
            MediaControlEvent::Next => engine.next(),
            MediaControlEvent::Previous => engine.prev(),
            MediaControlEvent::Seek(SeekDirection::Forward) => {
                let pos = engine.snapshot().position_ms.saturating_add(10_000);
                engine.seek(pos)
            }
            MediaControlEvent::Seek(SeekDirection::Backward) => {
                let pos = engine.snapshot().position_ms.saturating_sub(10_000);
                engine.seek(pos)
            }
            MediaControlEvent::SeekBy(SeekDirection::Forward, delta) => {
                let pos = engine.snapshot().position_ms.saturating_add(delta.as_millis() as u64);
                engine.seek(pos)
            }
            MediaControlEvent::SeekBy(SeekDirection::Backward, delta) => {
                let pos = engine.snapshot().position_ms.saturating_sub(delta.as_millis() as u64);
                engine.seek(pos)
            }
            MediaControlEvent::SetPosition(position) => engine.seek(position.0.as_millis() as u64),
            MediaControlEvent::SetVolume(volume) => engine.volume(volume as f32),
            MediaControlEvent::Raise | MediaControlEvent::Quit | MediaControlEvent::OpenUri(_) => Ok(()),
        };
    }

    fn file_url(path: &str) -> String {
        let path = path.replace('\\', "/");
        if let Some(rest) = path.strip_prefix("//") {
            format!("file://{rest}")
        } else if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }
}
