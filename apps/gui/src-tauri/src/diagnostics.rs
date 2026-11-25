//! Diagnóstico (It4.5): logs `tracing` locales y un informe JSON para adjuntar a mano.
//!
//! Nada sale del equipo (sin telemetría). El log de cada ejecución va a
//! `<datos>/logs/framerush-gui.log` y el de la anterior se guarda como `framerush-gui.old.log`.
//! El informe junta versión, `DeviceProfile`, vista Estado, journal, los dos logs de la GUI y
//! el de la CLI, con la carpeta del usuario sustituida por `%USERPROFILE%` (Windows) o `$HOME`
//! (Linux y macOS), y se guarda en Descargas.

use crate::snapshot::{data_dir, native_device, status_of};
use anyhow::{Context, Result};
use framerush_core::Lang;
use framerush_platform::Os;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG: &str = "framerush-gui.log";
const OLD_LOG: &str = "framerush-gui.old.log";
/// Log de la CLI (`framerush`), en la misma carpeta.
const CLI_LOG: &str = "framerush-cli.log";
/// Variable de la carpeta del usuario y el texto que la sustituye en el informe.
#[cfg(windows)]
const HOME: (&str, &str) = ("USERPROFILE", "%USERPROFILE%");
#[cfg(not(windows))]
const HOME: (&str, &str) = ("HOME", "$HOME");
/// Tope de líneas por log en el informe (las últimas).
const LOG_LINES: usize = 5000;

fn logs_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("logs"))
}

/// Abre el log de esta ejecución (`RUST_LOG` o `info`). Sin carpeta de datos no hay log.
pub fn init_logs() -> Option<PathBuf> {
    use tracing_subscriber::EnvFilter;
    let dir = logs_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(LOG);
    // Sin log anterior no hay nada que guardar.
    let _ = std::fs::rename(&path, dir.join(OLD_LOG));
    let file = std::fs::File::create(&path).ok()?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .try_init()
        .ok()?;
    Some(path)
}

/// Escribe el informe y devuelve dónde quedó. `lang` es el de los errores; el informe va en
/// español.
pub fn export(lang: Lang) -> Result<PathBuf> {
    let os = Os::current().context(lang.pick(
        "sistema operativo no admitido",
        "unsupported operating system",
    ))?;
    let device = native_device(os);
    // El informe de diagnóstico es para depurar: va en español, como los logs.
    let status = status_of(&device, Lang::Es).map_err(|e| crate::error::message(&e, Lang::Es));
    let journal = data_dir().and_then(|d| read_json(&d.join("session.json")));
    let logs = logs_dir();
    let log = |name| {
        logs.as_ref()
            .map(|d| tail(&d.join(name)))
            .unwrap_or_default()
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context(lang.pick(
            "reloj del sistema antes de 1970",
            "system clock before 1970",
        ))?
        .as_secs();
    let report = serde_json::json!({
        "app": "FrameRush",
        "version": env!("CARGO_PKG_VERSION"),
        "created_utc": stamp(now, true),
        "device": device,
        "status": status,
        "journal": journal,
        "log": log(LOG),
        "previous_log": log(OLD_LOG),
        "cli_log": log(CLI_LOG),
    });
    let text = serde_json::to_string_pretty(&report).context(lang.pick(
        "no se pudo generar el informe",
        "could not generate the report",
    ))?;
    let (var, placeholder) = HOME;
    let text = match std::env::var(var) {
        Ok(home) => redact(&text, &home, placeholder),
        Err(_) => text,
    };
    let dir = framerush_platform::downloads_dir()
        .or_else(data_dir)
        .context(lang.pick("no hay carpeta donde guardar", "no folder to save to"))?;
    std::fs::create_dir_all(&dir).with_context(|| {
        let d = dir.display();
        lang.pick(
            format!("no se pudo crear {d}"),
            format!("could not create {d}"),
        )
    })?;
    let path = dir.join(format!("framerush-diagnostico-{}.json", stamp(now, false)));
    std::fs::write(&path, text).with_context(|| {
        let p = path.display();
        lang.pick(
            format!("no se pudo escribir {p}"),
            format!("could not write {p}"),
        )
    })?;
    tracing::info!(path = %path.display(), "diagnóstico exportado");
    Ok(path)
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Las últimas `LOG_LINES` líneas del log, o nada si no existe.
fn tail(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(LOG_LINES);
    lines[skip..].iter().map(|l| (*l).to_owned()).collect()
}

/// Cambia la carpeta del usuario por `placeholder` sin distinguir mayúsculas, tal cual, con
/// barras `/` y escapada en JSON (`\\`).
fn redact(text: &str, home: &str, placeholder: &str) -> String {
    let home = home.trim_end_matches(['\\', '/']);
    if home.is_empty() {
        return text.to_owned();
    }
    let forms = [
        home.replace('\\', "\\\\"),
        home.replace('\\', "/"),
        home.to_owned(),
    ];
    forms.iter().fold(text.to_owned(), |t, from| {
        replace_ignore_case(&t, from, placeholder)
    })
}

fn replace_ignore_case(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if rest
            .get(..from.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(from))
        {
            out.push_str(to);
            rest = &rest[from.len()..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Fecha UTC de `secs` (Unix): `YYYYMMDD-HHMMSS` para el nombre o ISO 8601 con `Z`.
fn stamp(secs: u64, iso: bool) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil(days);
    let (h, min, s) = (rest / 3600, rest % 3600 / 60, rest % 60);
    if iso {
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
    } else {
        format!("{y:04}{m:02}{d:02}-{h:02}{min:02}{s:02}")
    }
}

/// Días desde 1970-01-01 a fecha civil (algoritmo `civil_from_days` de Howard Hinnant).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_every_form_of_the_user_folder() {
        let text = r#"{"a":"C:\\Users\\Brian\\x","b":"c:/users/brian/y","c":"C:\Users\brian"}"#;
        let out = redact(text, r"C:\Users\brian", "%USERPROFILE%");
        assert!(!out.to_lowercase().contains("brian"), "{out}");
        assert_eq!(
            out,
            r#"{"a":"%USERPROFILE%\\x","b":"%USERPROFILE%/y","c":"%USERPROFILE%"}"#
        );
    }

    #[test]
    fn redact_keeps_text_without_the_folder_and_non_ascii() {
        assert_eq!(
            redact("ñandú C:\\Users\\otro", r"C:\Users\brian", "%USERPROFILE%"),
            "ñandú C:\\Users\\otro"
        );
        assert_eq!(redact("abc", "", "%USERPROFILE%"), "abc");
    }

    #[test]
    fn redact_hides_a_unix_home() {
        let text = r#"{"log":"/home/ana/.local/share/framerush/logs","otro":"/home/luis"}"#;
        assert_eq!(
            redact(text, "/home/ana/", "$HOME"),
            r#"{"log":"$HOME/.local/share/framerush/logs","otro":"/home/luis"}"#
        );
    }

    #[test]
    fn stamp_formats_utc() {
        assert_eq!(stamp(0, false), "19700101-000000");
        // 2024-02-29 23:59:59 UTC (año bisiesto).
        assert_eq!(stamp(1_709_251_199, true), "2024-02-29T23:59:59Z");
        assert_eq!(stamp(1_709_251_200, false), "20240301-000000");
    }
}
