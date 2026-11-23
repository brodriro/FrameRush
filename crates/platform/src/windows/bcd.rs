//! `hypervisorlaunchtype` del cargador que arrancó (`{current}`) para `vbs-hvci-off`, con
//! `bcdedit.exe` de System32 (Microsoft Learn, "BCDEdit /set": `hypervisorlaunchtype [Off |
//! Auto]`). bcdedit pide admin incluso para leer. Los nombres y valores de los elementos salen
//! en inglés aunque la cabecera esté traducida (comprobado en un Windows 11 en español,
//! 2026-11-23: `hypervisorlaunchtype    Auto`).
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use std::path::PathBuf;
use std::process::Command;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

/// Dónde se anota en el journal (como la ruta de un valor de registro).
pub(super) const LOCATION: &str = r"BCD\{current}\hypervisorlaunchtype";
const ELEMENT: &str = "hypervisorlaunchtype";
/// Los dos valores documentados. Solo se escriben estos: lo que llega del journal o del
/// cliente del helper se comprueba antes.
const VALUES: [&str; 2] = ["Off", "Auto"];
/// `SECURITY_MANDATORY_HIGH_RID`: un proceso elevado.
const HIGH_INTEGRITY: u32 = 0x3000;

/// System32 real (`GetSystemDirectoryW`), no lo que diga `PATH` o `%SystemRoot%`.
fn bcdedit() -> Result<PathBuf> {
    let mut buf = [0u16; 260];
    // SAFETY: `buf` es un búfer vivo de `buf.len()` caracteres.
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if len == 0 || len >= buf.len() {
        return Err(PlatformError::Os(format!(
            "carpeta del sistema: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])).join("bcdedit.exe"))
}

fn require_admin() -> Result<()> {
    if super::integrity_level(std::process::id()).is_some_and(|rid| rid >= HIGH_INTEGRITY) {
        return Ok(());
    }
    Err(PlatformError::AccessDenied(
        "bcdedit pide admin incluso para leer".into(),
    ))
}

fn run(args: &[&str]) -> Result<String> {
    require_admin()?;
    let out = Command::new(bcdedit()?)
        .args(args)
        .output()
        .map_err(|e| PlatformError::Os(format!("bcdedit: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(PlatformError::Os(format!(
            "bcdedit {}: {} {}",
            args.join(" "),
            text.trim(),
            err.trim()
        )))
    }
}

/// Valor del elemento en la salida de `bcdedit /enum`, o `None` si no aparece (no está
/// puesto, así que manda el valor por defecto de Windows).
fn parse(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next()?.eq_ignore_ascii_case(ELEMENT))
            .then(|| parts.next().map(str::to_owned))
            .flatten()
    })
}

/// `Some("Off" | "Auto")` o `None` si no está puesto.
pub(super) fn read() -> Result<Option<String>> {
    run(&["/enum", "{current}"]).map(|out| parse(&out))
}

/// Pone uno de los valores documentados (sin distinguir mayúsculas).
pub(super) fn set(value: &str) -> Result<()> {
    let Some(value) = VALUES.iter().find(|v| v.eq_ignore_ascii_case(value)) else {
        return Err(PlatformError::Os(format!(
            "{ELEMENT}: valor no admitido «{value}»"
        )));
    };
    run(&["/set", "{current}", ELEMENT, value]).map(drop)
}

/// Quita el elemento: vuelve a mandar el valor por defecto de Windows.
pub(super) fn delete() -> Result<()> {
    run(&["/deletevalue", "{current}", ELEMENT]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_element_from_a_localized_listing() {
        let listing = "Cargador de arranque de Windows\n\
                       -----------------------------\n\
                       Identificador           {current}\n\
                       description             Windows 11\n\
                       hypervisorlaunchtype    Auto\n";
        assert_eq!(parse(listing).as_deref(), Some("Auto"));
        assert_eq!(parse("Identificador {current}\nnx OptIn\n"), None);
    }

    #[test]
    fn only_documented_values_are_written() {
        assert!(set("Off; del").is_err());
        assert!(set("Raw").is_err());
    }

    /// Solo lectura y con admin: `cargo test -- --ignored` desde una consola elevada.
    #[test]
    #[ignore = "pide admin"]
    fn reads_the_current_value() {
        let value = read().unwrap();
        assert!(
            value.as_deref().is_none_or(|v| VALUES.contains(&v)),
            "{value:?}"
        );
    }
}
