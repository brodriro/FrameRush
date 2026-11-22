//! Planes de energía (R5) con la API de `powrprof`: `PowerEnumerate` (`ACCESS_SCHEME`),
//! `PowerReadFriendlyName`, `PowerGetActiveScheme` y `PowerSetActiveScheme`.
//!
//! El id de un plan es su GUID en minúsculas y sin llaves; el nombre es el que da el SO, en el
//! idioma del usuario.
#![allow(unsafe_code)]

use crate::{PlatformError, PowerPlan, Result};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, LocalFree, WIN32_ERROR};
use windows_sys::Win32::System::Power::{
    ACCESS_SCHEME, PowerEnumerate, PowerGetActiveScheme, PowerReadFriendlyName,
    PowerSetActiveScheme,
};
use windows_sys::core::GUID;

fn check(code: WIN32_ERROR, what: &str) -> Result<()> {
    if code == ERROR_SUCCESS {
        return Ok(());
    }
    let e = std::io::Error::from_raw_os_error(code as i32);
    Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
        PlatformError::AccessDenied(format!("{what}: {e}"))
    } else {
        PlatformError::Os(format!("{what}: {e}"))
    })
}

/// `8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c`.
fn format_guid(g: &GUID) -> String {
    let d = g.data4;
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// Acepta el formato de [`format_guid`], con llaves o en mayúsculas.
fn parse_guid(s: &str) -> Option<GUID> {
    let s = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if parts.len() != 5
        || parts
            .iter()
            .zip(lens)
            .any(|(p, n)| p.len() != n || !p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    let hex: String = parts.concat();
    u128::from_str_radix(&hex, 16).ok().map(GUID::from_u128)
}

/// GUIDs de todos los planes del equipo.
fn scheme_guids() -> Result<Vec<GUID>> {
    let mut guids = vec![];
    for index in 0.. {
        let mut guid = GUID::from_u128(0);
        let mut size = size_of::<GUID>() as u32;
        // SAFETY: `guid` es un buffer de `size` bytes que vive durante la llamada; los punteros
        // nulos piden la lista de planes.
        let code = unsafe {
            PowerEnumerate(
                null_mut(),
                null(),
                null(),
                ACCESS_SCHEME,
                index,
                (&raw mut guid).cast(),
                &mut size,
            )
        };
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        check(code, "enumerar los planes de energía")?;
        guids.push(guid);
    }
    Ok(guids)
}

/// Nombre del plan que da el SO.
fn friendly_name(guid: &GUID) -> Result<String> {
    let mut size = 0u32;
    // SAFETY: con buffer nulo solo escribe en `size` el tamaño necesario, en bytes.
    let code =
        unsafe { PowerReadFriendlyName(null_mut(), guid, null(), null(), null_mut(), &mut size) };
    check(code, "leer el nombre del plan de energía")?;
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` tiene al menos `size` bytes y vive durante la llamada.
    let code = unsafe {
        PowerReadFriendlyName(
            null_mut(),
            guid,
            null(),
            null(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    check(code, "leer el nombre del plan de energía")?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

fn plan(guid: &GUID) -> Result<PowerPlan> {
    Ok(PowerPlan {
        id: format_guid(guid),
        name: friendly_name(guid)?,
    })
}

pub(super) fn power_plans() -> Result<Vec<PowerPlan>> {
    scheme_guids()?.iter().map(plan).collect()
}

pub(super) fn active_power_plan() -> Result<PowerPlan> {
    let mut raw: *mut GUID = null_mut();
    // SAFETY: `raw` recibe un GUID reservado por el sistema que se libera abajo.
    check(
        unsafe { PowerGetActiveScheme(null_mut(), &mut raw) },
        "leer el plan de energía activo",
    )?;
    // SAFETY: la llamada tuvo éxito, así que `raw` apunta a un GUID válido.
    let guid = unsafe { *raw };
    // SAFETY: el GUID se reservó con LocalAlloc (documentado) y ya se copió.
    unsafe { LocalFree(raw.cast()) };
    plan(&guid)
}

/// Solo activa un plan que existe en el equipo.
pub(super) fn set_active_power_plan(id: &str) -> Result<()> {
    let guid = parse_guid(id)
        .ok_or_else(|| PlatformError::Os(format!("id de plan de energía no válido: `{id}`")))?;
    // `GUID` de windows-sys no implementa `PartialEq`: se compara su forma de texto.
    let wanted = format_guid(&guid);
    if !scheme_guids()?.iter().any(|g| format_guid(g) == wanted) {
        return Err(PlatformError::Os(format!(
            "el plan de energía `{id}` no existe en este equipo"
        )));
    }
    // SAFETY: `guid` es un local vivo durante la llamada.
    check(
        unsafe { PowerSetActiveScheme(null_mut(), &guid) },
        "activar el plan de energía",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_round_trip() {
        let id = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
        let text = |s: &str| parse_guid(s).map(|g| format_guid(&g));
        assert_eq!(text(id).as_deref(), Some(id));
        assert_eq!(
            text("{8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C}").as_deref(),
            Some(id)
        );
        assert_eq!(text("8c5e7fda-e8bf-4a96-9a85"), None);
        assert_eq!(text("zc5e7fda-e8bf-4a96-9a85-a6e23a8c635c"), None);
    }

    /// No cambia nada: el plan activo es uno de los del equipo, todos tienen nombre y volver a
    /// activar el que ya lo está funciona sin elevar.
    #[test]
    fn the_active_plan_is_listed() {
        let plans = power_plans().unwrap();
        assert!(!plans.is_empty());
        assert!(plans.iter().all(|p| !p.name.is_empty()));
        let active = active_power_plan().unwrap();
        assert!(plans.contains(&active), "{active:?} no está en {plans:?}");
        set_active_power_plan(&active.id).unwrap();
        assert_eq!(active_power_plan().unwrap(), active);
    }

    #[test]
    fn refuses_a_plan_that_does_not_exist() {
        assert!(set_active_power_plan("00000000-0000-0000-0000-000000000001").is_err());
        assert!(set_active_power_plan("alto").is_err());
    }
}
