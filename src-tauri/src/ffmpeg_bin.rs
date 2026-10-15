//! Locate the bundled FFmpeg sidecar. The Windows installer places it beside
//! the app; development can point at it with CHENGYIN_FFMPEG.

use std::path::{Path, PathBuf};

const NAMES: &[&str] = &[
    "ffmpeg-x86_64-pc-windows-msvc.exe",
    "ffmpeg.exe",
    "ffmpeg",
];

pub fn resolve(resource_dir: Option<&Path>, exe_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = env_path("CHENGYIN_FFMPEG") {
        return Some(path);
    }
    let mut dirs = Vec::new();
    if let Some(dir) = resource_dir {
        dirs.push(dir.to_path_buf());
        dirs.push(dir.join("binaries"));
    }
    if let Some(dir) = exe_dir {
        dirs.push(dir.to_path_buf());
        dirs.push(dir.join("binaries"));
    }
    dirs.push(PathBuf::from("src-tauri/binaries"));
    dirs.push(PathBuf::from("binaries"));
    for dir in dirs {
        if let Some(found) = find_in(&dir) {
            return Some(found);
        }
    }
    which_on_path()
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var(key).ok()?;
    let path = PathBuf::from(value);
    path.is_file().then_some(path)
}

fn find_in(dir: &Path) -> Option<PathBuf> {
    for name in NAMES {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn which_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if let Some(found) = find_in(&dir) {
            return Some(found);
        }
    }
    None
}

