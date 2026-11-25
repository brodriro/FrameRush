//! Carpeta de FrameRush en el `PATH` del sistema, para el instalador.
//!
//! El `PATH` del sistema es el valor `Path` de
//! `HKLM\System\CurrentControlSet\Control\Session Manager\Environment`. Tras cambiarlo se
//! difunde `WM_SETTINGCHANGE` con `"Environment"` para que el shell lo recoja (Microsoft Learn,
//! «Environment Variables»). Se lee sin expandir (`RRF_NOEXPAND`) y se reescribe con su tipo,
//! para no convertir `%SystemRoot%` en rutas fijas. Escribir en HKLM pide admin.
#![allow(unsafe_code)]

use super::registry::{self, Hive};
use crate::{PlatformError, Result};
use std::path::Path;
use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::System::Registry::REG_EXPAND_SZ;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};

const KEY: &str = r"System\CurrentControlSet\Control\Session Manager\Environment";
const VALUE: &str = "Path";
/// Espera máxima por ventana al difundir el cambio.
const BROADCAST_MS: u32 = 5000;

/// Añade `dir` al final del `PATH` del sistema. `false` si ya estaba.
pub fn add_to_system_path(dir: &Path) -> Result<bool> {
    update(dir, with_dir)
}

/// Quita `dir` del `PATH` del sistema. `false` si no estaba.
pub fn remove_from_system_path(dir: &Path) -> Result<bool> {
    update(dir, without_dir)
}

fn update(dir: &Path, change: fn(&str, &str) -> Option<String>) -> Result<bool> {
    let dir = dir
        .to_str()
        .ok_or_else(|| PlatformError::Os(format!("ruta no válida: {}", dir.display())))?;
    let (current, kind) =
        registry::read_raw_text(Hive::LocalMachine, KEY, VALUE)?.unwrap_or_default();
    let Some(next) = change(&current, dir) else {
        return Ok(false);
    };
    let kind = if kind == 0 { REG_EXPAND_SZ } else { kind };
    registry::write_text(Hive::LocalMachine, KEY, VALUE, kind, &next)?;
    broadcast();
    Ok(true)
}

/// Avisa a las ventanas de que cambió el entorno. Si alguna no responde, se sigue.
fn broadcast() {
    let what: Vec<u16> = "Environment".encode_utf16().chain([0]).collect();
    let mut result = 0usize;
    // SAFETY: `what` termina en nulo y vive durante la llamada, que espera como mucho
    // `BROADCAST_MS` por ventana; `result` es un local vivo.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            what.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            BROADCAST_MS,
            &mut result,
        )
    };
}

/// Misma carpeta, sin distinguir mayúsculas ni la barra final.
fn same(entry: &str, dir: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_owned();
    norm(entry).eq_ignore_ascii_case(&norm(dir))
}

/// `path` con `dir` al final, o `None` si ya estaba.
fn with_dir(path: &str, dir: &str) -> Option<String> {
    if path.split(';').any(|e| same(e, dir)) {
        return None;
    }
    let base = path.trim_end_matches(';');
    Some(if base.is_empty() {
        dir.to_owned()
    } else {
        format!("{base};{dir}")
    })
}

/// `path` sin ninguna entrada igual a `dir`, o `None` si no estaba.
fn without_dir(path: &str, dir: &str) -> Option<String> {
    let entries: Vec<&str> = path.split(';').collect();
    let kept: Vec<&str> = entries.iter().copied().filter(|e| !same(e, dir)).collect();
    (kept.len() != entries.len()).then(|| kept.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Program Files\FrameRush";

    #[test]
    fn adds_once_at_the_end() {
        assert_eq!(
            with_dir(r"%SystemRoot%\system32;C:\x;", DIR).as_deref(),
            Some(r"%SystemRoot%\system32;C:\x;C:\Program Files\FrameRush")
        );
        assert_eq!(with_dir("", DIR).as_deref(), Some(DIR));
        assert_eq!(with_dir(r"C:\x;c:\program files\framerush\", DIR), None);
    }

    #[test]
    fn removes_every_copy_and_keeps_the_rest() {
        assert_eq!(
            without_dir(
                r"%SystemRoot%;C:\Program Files\FrameRush;C:\x;C:\PROGRAM FILES\FrameRush\",
                DIR
            )
            .as_deref(),
            Some(r"%SystemRoot%;C:\x")
        );
        assert_eq!(without_dir(r"%SystemRoot%;C:\x", DIR), None);
    }
}
