//! Estado actual del equipo para la vista Estado (PLAN.md It5.6): lo que hay ahora, no lo que
//! se pidió. Lo de la sesión lo da [`crate::status_lines`]. Lo que no se pudo leer sale como
//! "No disponible", nunca como un valor inventado.

use crate::Lang;
use framerush_device::DeviceProfile;
use framerush_platform::GameFiles;
use std::path::PathBuf;

fn unavailable(lang: Lang) -> &'static str {
    lang.pick("No disponible", "Not available")
}

/// Una línea de estado: qué y cómo está.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StateLine {
    pub label: String,
    pub value: String,
}

fn line(label: impl Into<String>, value: impl Into<String>) -> StateLine {
    StateLine {
        label: label.into(),
        value: value.into(),
    }
}

/// VBS/HVCI, plan de energía activo y frecuencia de cada monitor, según `device`, en `lang`.
pub fn system_state(device: &DeviceProfile, lang: Lang) -> Vec<StateLine> {
    let na = unavailable(lang);
    let vbs = match device.vbs_hvci_enabled {
        Some(true) => lang.pick("en ejecución", "running"),
        Some(false) => lang.pick("desactivado", "off"),
        None => na,
    };
    let plan = device
        .active_power_plan
        .as_deref()
        .and_then(|id| device.power_plans.iter().find(|p| p.id == id))
        .map_or(na.to_owned(), |p| p.name.clone());
    let mut lines = vec![
        line("VBS/HVCI", vbs),
        line(lang.pick("Plan de energía", "Power plan"), plan),
    ];
    if device.displays.is_empty() {
        lines.push(line(lang.pick("Monitores", "Displays"), na));
    }
    for d in &device.displays {
        let m = d.current;
        let max = d
            .max_hz_at_current_resolution()
            .filter(|max| *max != m.hz)
            .map_or_else(
                || lang.pick(" (máximo)", " (maximum)").to_owned(),
                |max| lang.pick(format!(" (máx. {max} Hz)"), format!(" (max {max} Hz)")),
            );
        let primary = if d.primary {
            lang.pick(" (principal)", " (primary)")
        } else {
            ""
        };
        let (w, h, hz) = (m.width, m.height, m.hz);
        lines.push(line(
            lang.pick(
                format!("Monitor {}{primary}", d.id),
                format!("Display {}{primary}", d.id),
            ),
            lang.pick(
                format!("{w}×{h} a {hz} Hz{max}"),
                format!("{w}×{h} at {hz} Hz{max}"),
            ),
        ));
    }
    lines
}

/// Config del juego `game`: si cada archivo existe y si está bloqueado (solo lectura,
/// `game-config-lock`), en `lang`. Si dos archivos se llaman igual (uno por cuenta de Steam),
/// se nombran por su ruta completa.
pub fn game_config_state(
    platform: &impl GameFiles,
    game: &str,
    files: &[PathBuf],
    lang: Lang,
) -> Vec<StateLine> {
    let short = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned());
    files
        .iter()
        .map(|path| {
            let shared = files
                .iter()
                .filter(|other| short(other) == short(path))
                .count()
                > 1;
            let name = short(path)
                .filter(|_| !shared)
                .unwrap_or_else(|| path.display().to_string());
            let value = match platform.read_game_file(path) {
                Ok(Some((_, true))) => lang
                    .pick("bloqueada (solo lectura)", "locked (read-only)")
                    .to_owned(),
                Ok(Some((_, false))) => "editable".to_owned(),
                Ok(None) => lang.pick("no existe", "missing").to_owned(),
                Err(e) => format!("{} ({e})", unavailable(lang)),
            };
            line(format!("{game}: {name}"), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_device::samples;
    use framerush_platform::mock::MockPlatform;
    use framerush_platform::{Display, Mode, Os, PowerPlan};

    #[test]
    fn missing_data_is_unavailable_never_invented() {
        let lines = system_state(&DeviceProfile::undetected(Os::Windows), Lang::Es);
        assert!(
            lines.iter().all(|l| l.value == unavailable(Lang::Es)),
            "{lines:?}"
        );
    }

    #[test]
    fn reads_vbs_plan_and_hz_from_the_device() {
        let mut d = samples::hybrid_laptop();
        d.vbs_hvci_enabled = Some(false);
        d.power_plans = vec![PowerPlan {
            id: "a".into(),
            name: "Equilibrado".into(),
        }];
        d.active_power_plan = Some("a".into());
        let mode = |hz| Mode {
            width: 1920,
            height: 1080,
            hz,
        };
        d.displays = vec![Display {
            id: "DISPLAY1".into(),
            current: mode(60),
            modes: vec![mode(60), mode(144)],
            primary: true,
        }];
        let lines = system_state(&d, Lang::Es);
        assert_eq!(lines[0].value, "desactivado");
        assert_eq!(lines[1].value, "Equilibrado");
        assert_eq!(lines[2].label, "Monitor DISPLAY1 (principal)");
        assert_eq!(lines[2].value, "1920×1080 a 60 Hz (máx. 144 Hz)");
        let en = system_state(&d, Lang::En);
        assert_eq!(en[0].value, "off");
        assert_eq!(en[2].label, "Display DISPLAY1 (primary)");
        assert_eq!(en[2].value, "1920×1080 at 60 Hz (max 144 Hz)");
    }

    #[test]
    fn game_config_says_if_it_is_locked() {
        let m = MockPlatform::new(4);
        let locked = PathBuf::from("/game/video.txt");
        let free = PathBuf::from("/game/framerush.cfg");
        m.write_game_file(&locked, "x", true).unwrap();
        m.write_game_file(&free, "y", false).unwrap();
        let lines = game_config_state(
            &m,
            "CS2",
            &[locked, free, PathBuf::from("/game/no.cfg")],
            Lang::Es,
        );
        let values: Vec<&str> = lines.iter().map(|l| l.value.as_str()).collect();
        assert_eq!(
            values,
            ["bloqueada (solo lectura)", "editable", "no existe"]
        );
        assert_eq!(lines[0].label, "CS2: video.txt");

        let a = PathBuf::from("/u/1/video.txt");
        let b = PathBuf::from("/u/2/video.txt");
        let lines = game_config_state(&m, "CS2", &[a.clone(), b], Lang::Es);
        assert_eq!(lines[0].label, format!("CS2: {}", a.display()));
    }
}
