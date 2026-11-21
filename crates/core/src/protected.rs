//! Lista de procesos protegidos (PLAN.md, It1.2).
//!
//! Es inmutable: vive en el código y no en `rules/*.toml`, así que ni las reglas ni la UI pueden
//! anularla. A un proceso protegido no se le suspende, no se le cierra y no se le cambia la
//! prioridad. Los nombres que faltan por confirmar están en PLAN.md §6, V19.

/// Por qué un proceso está protegido; la UI lo usa para explicar el candado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtectedReason {
    /// Proceso crítico del sistema operativo.
    OsCritical,
    /// Compositor o servidor gráfico.
    Compositor,
    /// Servicio de audio.
    Audio,
    /// Proceso de usuario del driver de la GPU.
    GpuDriver,
    /// Cualquier anticheat.
    AntiCheat,
    /// La propia app o su helper: si se congelan, nadie podría reanudar el resto (V17).
    SelfApp,
}

#[derive(Debug)]
enum Match {
    /// Nombre completo, sin `.exe` y en minúsculas.
    Exact(&'static str),
    /// Prefijo, en minúsculas.
    Prefix(&'static str),
}

use Match::{Exact, Prefix};
use ProtectedReason::*;

const LIST: &[(Match, ProtectedReason)] = &[
    // Windows
    (Exact("system"), OsCritical),
    (Exact("registry"), OsCritical),
    (Exact("memory compression"), OsCritical),
    (Exact("smss"), OsCritical),
    (Exact("csrss"), OsCritical),
    (Exact("wininit"), OsCritical),
    (Exact("winlogon"), OsCritical),
    (Exact("lsass"), OsCritical),
    (Exact("services"), OsCritical),
    (Exact("svchost"), OsCritical),
    (Exact("fontdrvhost"), OsCritical),
    (Exact("explorer"), OsCritical),
    (Exact("dwm"), Compositor),
    (Exact("audiodg"), Audio),
    (Exact("nvdisplay.container"), GpuDriver),
    (Exact("atiesrxx"), GpuDriver),
    (Exact("atieclxx"), GpuDriver),
    // Linux
    (Prefix("systemd"), OsCritical),
    (Exact("dbus-daemon"), OsCritical),
    (Exact("xorg"), Compositor),
    (Exact("xwayland"), Compositor),
    (Exact("gnome-shell"), Compositor),
    (Prefix("kwin_"), Compositor),
    (Exact("pipewire"), Audio),
    (Exact("pipewire-pulse"), Audio),
    (Exact("wireplumber"), Audio),
    (Exact("pulseaudio"), Audio),
    // macOS
    (Exact("kernel_task"), OsCritical),
    (Exact("launchd"), OsCritical),
    (Exact("loginwindow"), OsCritical),
    (Exact("windowserver"), Compositor),
    (Exact("coreaudiod"), Audio),
    // Anticheats (todas las plataformas)
    (Prefix("easyanticheat"), AntiCheat),
    (Prefix("beservice"), AntiCheat),
    (Exact("vgc"), AntiCheat),
    (Prefix("faceit"), AntiCheat),
    (Prefix("eaanticheat"), AntiCheat),
    // FPS Boost
    (Exact("fpsboost"), SelfApp),
    (Exact("fpsboost-helper"), SelfApp),
];

/// Normaliza un nombre o ruta de proceso: último componente (con `/` o `\`), minúsculas y sin
/// `.exe`. Se separa a mano para que una ruta de Windows se lea igual en cualquier SO.
fn normalize(name_or_path: &str) -> String {
    let file = name_or_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name_or_path);
    let lower = file.trim().to_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

/// Motivo de protección de un proceso, o `None` si se puede tocar.
pub fn protected_reason(name_or_path: &str) -> Option<ProtectedReason> {
    let name = normalize(name_or_path);
    LIST.iter()
        .find(|(m, _)| match m {
            Exact(n) => name == *n,
            Prefix(p) => name.starts_with(p),
        })
        .map(|(_, reason)| *reason)
}

/// `true` si el proceso no se puede suspender, cerrar ni cambiar de prioridad.
pub fn is_protected(name_or_path: &str) -> bool {
    protected_reason(name_or_path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_names_and_paths_case_insensitively() {
        assert_eq!(
            protected_reason(r"C:\Windows\System32\csrss.exe"),
            Some(OsCritical)
        );
        assert_eq!(protected_reason("DWM.EXE"), Some(Compositor));
        assert_eq!(protected_reason("/usr/bin/Xorg"), Some(Compositor));
        assert_eq!(protected_reason("kwin_wayland"), Some(Compositor));
        assert_eq!(protected_reason("systemd-journald"), Some(OsCritical));
        assert_eq!(protected_reason("WindowServer"), Some(Compositor));
    }

    #[test]
    fn every_anticheat_family_is_protected() {
        for exe in [
            "EasyAntiCheat.exe",
            "EasyAntiCheat_EOS.exe",
            "BEService.exe",
            "vgc.exe",
            "faceit.exe",
        ] {
            assert_eq!(protected_reason(exe), Some(AntiCheat), "{exe}");
        }
    }

    #[test]
    fn the_app_and_its_helper_are_protected() {
        assert_eq!(protected_reason("fpsboost.exe"), Some(SelfApp));
        assert_eq!(protected_reason("/usr/bin/fpsboost-helper"), Some(SelfApp));
    }

    #[test]
    fn ordinary_processes_are_not_protected() {
        for exe in [
            "chrome.exe",
            "OneDrive.exe",
            "Discord.exe",
            "cs2.exe",
            "explorer2.exe",
        ] {
            assert!(!is_protected(exe), "{exe}");
        }
    }

    #[test]
    fn every_reason_has_an_entry() {
        for reason in [OsCritical, Compositor, Audio, GpuDriver, AntiCheat, SelfApp] {
            assert!(LIST.iter().any(|(_, r)| *r == reason), "{reason:?}");
        }
    }
}
