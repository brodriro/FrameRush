//! Exclusiones de Microsoft Defender (`defender-exclusions`, PLAN.md It5.9) con WMI, en
//! `root\Microsoft\Windows\Defender` (lo mismo que usan `Get-MpPreference`, `Add-MpPreference`
//! y `Remove-MpPreference`):
//!
//! - `MSFT_MpComputerStatus.AMRunningMode`: `Normal` es Defender como antivirus activo; se lee
//!   sin admin (learn.microsoft.com/defender-endpoint/microsoft-defender-antivirus-windows).
//! - `MSFT_MpPreference.ExclusionPath` / `ExclusionProcess`: sin admin devuelven un texto
//!   («N/A: Must be an administrator to view exclusions», comprobado en Windows 11), así que
//!   sin elevar se responde `AccessDenied` sin preguntar.
//! - `MSFT_MpPreference.Add` / `Remove` (estáticos): `ExclusionPath[]` y `ExclusionProcess[]`.
//!
//! Tamper Protection solo bloquea las exclusiones que gestiona una organización (Intune o
//! Configuration Manager); en un equipo personal un administrador puede añadirlas
//! (learn.microsoft.com/defender-endpoint/tamper-protection-antivirus-exclusions).

use super::wmi::{self, Arg, Com, On};
use crate::{DefenderExclusion, PlatformError, Result};

const NAMESPACE: &str = r"ROOT\Microsoft\Windows\Defender";
const PREFERENCE: &str = "MSFT_MpPreference";
/// `SECURITY_MANDATORY_HIGH_RID`: un proceso elevado.
const HIGH_INTEGRITY: u32 = 0x3000;

/// `Some(true)` si Defender es el antivirus activo (`AMRunningMode = Normal`); `None` si no
/// hay datos (Defender desinstalado o sin la clase).
pub(super) fn active() -> Result<Option<bool>> {
    let mode = wmi::first(
        NAMESPACE,
        "SELECT AMRunningMode FROM MSFT_MpComputerStatus",
        |o| wmi::string(o, "AMRunningMode"),
    )?
    .flatten();
    Ok(mode.map(|m| m.eq_ignore_ascii_case("Normal")))
}

fn require_admin(what: &str) -> Result<()> {
    if super::integrity_level(std::process::id()).is_some_and(|rid| rid >= HIGH_INTEGRITY) {
        return Ok(());
    }
    Err(PlatformError::AccessDenied(format!(
        "{what}: las exclusiones de Defender piden admin"
    )))
}

fn property(ex: &DefenderExclusion) -> (&'static str, String) {
    match ex {
        DefenderExclusion::Path(p) => ("ExclusionPath", p.display().to_string()),
        DefenderExclusion::Process(p) => ("ExclusionProcess", p.display().to_string()),
    }
}

/// Misma entrada para Defender: sin distinguir mayúsculas ni la barra final.
fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\')
        .eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

pub(super) fn has(ex: &DefenderExclusion) -> Result<bool> {
    require_admin("leer las exclusiones")?;
    let (name, value) = property(ex);
    let list = wmi::first(
        NAMESPACE,
        &format!("SELECT {name} FROM {PREFERENCE}"),
        |o| wmi::strings(o, name),
    )?
    .unwrap_or_default();
    Ok(list.iter().any(|item| same(item, &value)))
}

fn change(method: &str, ex: &DefenderExclusion) -> Result<()> {
    require_admin(method)?;
    let (name, value) = property(ex);
    let values = [value];
    let _com = Com::init()?;
    wmi::call(
        NAMESPACE,
        PREFERENCE,
        On::Class(PREFERENCE),
        method,
        &[(name, Arg::Strings(&values))],
    )
    .map(drop)
}

pub(super) fn add(ex: &DefenderExclusion) -> Result<()> {
    change("Add", ex)
}

pub(super) fn remove(ex: &DefenderExclusion) -> Result<()> {
    change("Remove", ex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn same_ignores_case_and_the_trailing_backslash() {
        assert!(same(r"C:\Games\CS2\", r"c:\games\cs2"));
        assert!(!same(r"C:\Games\CS2", r"C:\Games\CS"));
    }

    /// Sin admin no se pregunta a WMI (devolvería un texto en vez de la lista).
    #[test]
    fn reading_without_admin_is_access_denied() {
        if super::super::integrity_level(std::process::id()).is_some_and(|r| r >= HIGH_INTEGRITY) {
            return;
        }
        let ex = DefenderExclusion::Path(PathBuf::from(r"C:\framerush-no-existe"));
        assert!(matches!(has(&ex), Err(PlatformError::AccessDenied(_))));
        assert!(matches!(add(&ex), Err(PlatformError::AccessDenied(_))));
    }

    /// En este equipo se puede leer sin admin; en otro puede no haber Defender.
    #[test]
    fn reads_the_running_mode() {
        let _ = active();
    }

    /// Como administrador: añade una carpeta y un exe que no existen, comprueba que están y
    /// los quita. `cargo test -p framerush-platform -- --ignored` desde una consola elevada.
    #[test]
    #[ignore = "necesita admin y cambia Defender (lo deja como estaba)"]
    fn adds_and_removes_exclusions() {
        for ex in [
            DefenderExclusion::Path(PathBuf::from(r"C:\framerush-prueba-exclusion")),
            DefenderExclusion::Process(PathBuf::from(r"C:\framerush-prueba-exclusion\juego.exe")),
        ] {
            assert!(!has(&ex).unwrap(), "{ex:?} ya estaba");
            add(&ex).unwrap();
            assert!(has(&ex).unwrap(), "{ex:?} no se añadió");
            remove(&ex).unwrap();
            assert!(!has(&ex).unwrap(), "{ex:?} no se quitó");
        }
    }
}
