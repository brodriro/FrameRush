//! Implementaciones para Linux.

mod dbus;
mod gamemode;
mod helper;
mod power;
mod process;
mod services;
mod steam;
mod system;

pub use helper::{Helper, serve_helper};
pub use process::LinuxPlatform;
pub use steam::{steam_root, steam_running_app};

use crate::xdg::{base_dir, user_dir};
use std::path::PathBuf;

/// Carpeta «Descargas» del usuario según xdg-user-dirs (`XDG_DOWNLOAD_DIR` de
/// `$XDG_CONFIG_HOME/user-dirs.dirs`), o `None` si no hay `HOME`, falta el archivo o la entrada,
/// o está desactivada (ver `user_dir`).
///
/// Ubicación del archivo: wiki de xdg-user-dirs (freedesktop.org), «XDG_CONFIG_HOME defaults to
/// ~/.config». Según la XDG Base Directory Specification, si `XDG_CONFIG_HOME` no está o está
/// vacía se usa `$HOME/.config`, y una ruta relativa no es válida y se ignora.
pub fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    let config = base_dir(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        &home,
        ".config",
    );
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    user_dir(&text, "XDG_DOWNLOAD_DIR", &home)
}

/// Base de la carpeta de datos: `$XDG_DATA_HOME` o `$HOME/.local/share` (XDG Base Directory
/// Specification, «$XDG_DATA_HOME defines the base directory relative to which user-specific data
/// files should be stored»).
pub(crate) fn data_home() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(base_dir(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        &home,
        ".local/share",
    ))
}
