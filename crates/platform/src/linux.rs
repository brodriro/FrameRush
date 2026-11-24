//! Implementaciones para Linux.

use crate::xdg::user_dir;
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
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    user_dir(&text, "XDG_DOWNLOAD_DIR", &home)
}
