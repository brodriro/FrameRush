//! Carpeta de datos de FrameRush (journal, logs, perfiles), la misma para la CLI y la GUI.

use std::path::PathBuf;

/// Variable que la fija a mano (tests, varias instalaciones).
const OVERRIDE: &str = "FRAMERUSH_DATA_DIR";

/// `FRAMERUSH_DATA_DIR` o la del SO, o `None` si no se puede saber:
/// - Windows: `%LOCALAPPDATA%/framerush`.
/// - Linux: `$XDG_DATA_HOME/framerush` (`~/.local/share/framerush` por defecto).
/// - macOS: `~/Library/Application Support/io.github.brodriro.framerush`: Apple pide una
///   subcarpeta con el identificador del bundle (el `identifier` de `tauri.conf.json`).
pub fn data_dir() -> Option<PathBuf> {
    match std::env::var_os(OVERRIDE).filter(|d| !d.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => native(),
    }
}

#[cfg(windows)]
fn native() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(base).join("framerush"))
}

#[cfg(target_os = "linux")]
fn native() -> Option<PathBuf> {
    Some(crate::linux::data_home()?.join("framerush"))
}

#[cfg(target_os = "macos")]
fn native() -> Option<PathBuf> {
    Some(crate::macos::data_home()?.join("io.github.brodriro.framerush"))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn native() -> Option<PathBuf> {
    None
}
