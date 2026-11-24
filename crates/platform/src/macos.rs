//! Implementaciones para macOS.

use std::path::PathBuf;

/// Carpeta «Descargas» del usuario: `~/Downloads`, o `None` si no hay `HOME`.
///
/// Fuente: Apple, File System Programming Guide («File System Basics»), carpeta personal del
/// usuario: «Downloads—Contains files downloaded from the Internet». Equivale a
/// `FileManager.SearchPathDirectory.downloadsDirectory` en el dominio del usuario; se usa la ruta
/// para no añadir enlaces a Objective-C solo para esto.
pub fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Downloads"))
}
