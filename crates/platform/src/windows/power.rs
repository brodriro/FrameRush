//! Planes de energía (R5) con la API de `powrprof`: `PowerEnumerate` (`ACCESS_SCHEME`),
//! `PowerReadFriendlyName`, `PowerGetActiveScheme` y `PowerSetActiveScheme`. El plan propio
//! (`custom-power-plan`) se crea con `PowerDuplicateScheme`, `PowerWriteFriendlyName` y
//! `PowerWriteACValueIndex`, y se borra con `PowerDeleteScheme`. Nada de esto pide elevación
//! (probado sin admin el 2026-11-22).
//!
//! El id de un plan es su GUID en minúsculas y sin llaves; el nombre es el que da el SO, en el
//! idioma del usuario.
#![allow(unsafe_code)]

use crate::{PlatformError, PowerPlan, Result};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, LocalFree, WIN32_ERROR};
use windows_sys::Win32::System::Power::{
    ACCESS_SCHEME, PowerDeleteScheme, PowerDuplicateScheme, PowerEnumerate, PowerGetActiveScheme,
    PowerReadFriendlyName, PowerSetActiveScheme, PowerWriteACValueIndex, PowerWriteFriendlyName,
};
use windows_sys::Win32::System::SystemServices::{
    GUID_PCIEXPRESS_ASPM_POLICY, GUID_PCIEXPRESS_SETTINGS_SUBGROUP,
    GUID_PROCESSOR_CORE_PARKING_MAX_CORES, GUID_PROCESSOR_CORE_PARKING_MIN_CORES,
    GUID_PROCESSOR_IDLE_DISABLE, GUID_PROCESSOR_PERF_BOOST_MODE, GUID_PROCESSOR_SETTINGS_SUBGROUP,
    GUID_PROCESSOR_THROTTLE_MAXIMUM, GUID_PROCESSOR_THROTTLE_MINIMUM,
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
    let guid = existing_guid(id)?;
    // SAFETY: `guid` es un local vivo durante la llamada.
    check(
        unsafe { PowerSetActiveScheme(null_mut(), &guid) },
        "activar el plan de energía",
    )
}

/// GUID de un plan que existe en el equipo.
fn existing_guid(id: &str) -> Result<GUID> {
    let guid = parse_guid(id)
        .ok_or_else(|| PlatformError::Os(format!("id de plan de energía no válido: `{id}`")))?;
    // `GUID` de windows-sys no implementa `PartialEq`: se compara su forma de texto.
    let wanted = format_guid(&guid);
    if !scheme_guids()?.iter().any(|g| format_guid(g) == wanted) {
        return Err(PlatformError::Os(format!(
            "el plan de energía `{id}` no existe en este equipo"
        )));
    }
    Ok(guid)
}

/// Subgrupo "Configuración de USB". El SDK no tiene constante: GUID y valores salen de la base
/// de datos de energía del SO (`powercfg /qh SCHEME_CURRENT <subgrupo> <ajuste>`) y coinciden
/// con la referencia CS2-FPS-Boost.
const USB_SETTINGS_SUBGROUP: GUID = GUID::from_u128(0x2a737441_1930_4402_8d77_b2bebba308a3);
/// "Suspensión selectiva de USB": 0 = deshabilitada, 1 = habilitada.
const USB_SELECTIVE_SUSPEND: GUID = GUID::from_u128(0x48e6b7a6_50f5_4782_a5d4_53bb8f07e226);

/// Valores de CA del plan propio (design/SETTINGS.md `custom-power-plan`, valores de la
/// referencia). Significados: Microsoft Learn, "Processor power management options"
/// (PROCTHROTTLEMIN/MAX en %, CPMINCORES = 100 % desactiva el core parking, PERFBOOSTMODE 2 =
/// Aggressive) y `powercfg /q` (ASPM 0 = desactivado).
const CUSTOM_AC_VALUES: &[(GUID, GUID, u32)] = &[
    (
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MINIMUM,
        100,
    ),
    (
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_THROTTLE_MAXIMUM,
        100,
    ),
    (
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_PERF_BOOST_MODE,
        2,
    ),
    (
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_CORE_PARKING_MIN_CORES,
        100,
    ),
    (
        GUID_PROCESSOR_SETTINGS_SUBGROUP,
        GUID_PROCESSOR_CORE_PARKING_MAX_CORES,
        100,
    ),
    (
        GUID_PCIEXPRESS_SETTINGS_SUBGROUP,
        GUID_PCIEXPRESS_ASPM_POLICY,
        0,
    ),
    (USB_SETTINGS_SUBGROUP, USB_SELECTIVE_SUSPEND, 0),
];

/// `cstates-off`: "Inactividad de procesador deshabilitada" (`IDLEDISABLE`, 1 = deshabilitar la
/// inactividad, es decir, los C-states). Microsoft lo usa así en "How to set up a Device for
/// Real-Time Performance" (learn.microsoft.com/windows/iot/iot-enterprise/soft-real-time),
/// sin reinicio; la referencia CS2-FPS-Boost lo pone en su plan propio en Extremo.
const CPU_IDLE_OFF: (GUID, GUID, u32) = (
    GUID_PROCESSOR_SETTINGS_SUBGROUP,
    GUID_PROCESSOR_IDLE_DISABLE,
    1,
);

/// Si falla a medias, borra el duplicado para no dejar un plan a medio configurar.
pub(super) fn create_custom_power_plan(
    base_id: &str,
    name: &str,
    cpu_idle_off: bool,
) -> Result<PowerPlan> {
    let base = existing_guid(base_id)?;
    let mut raw: *mut GUID = null_mut();
    // SAFETY: `raw` es nulo, así que el sistema reserva el GUID nuevo (se libera abajo).
    check(
        unsafe { PowerDuplicateScheme(null_mut(), &base, &mut raw) },
        "duplicar el plan de energía",
    )?;
    // SAFETY: la llamada tuvo éxito, así que `raw` apunta a un GUID válido.
    let guid = unsafe { *raw };
    // SAFETY: el GUID se reservó con LocalAlloc (documentado) y ya se copió.
    unsafe { LocalFree(raw.cast()) };
    if let Err(e) = configure(&guid, name, cpu_idle_off) {
        // SAFETY: `guid` es un local vivo durante la llamada.
        unsafe { PowerDeleteScheme(null_mut(), &guid) };
        return Err(e);
    }
    plan(&guid)
}

fn configure(guid: &GUID, name: &str, cpu_idle_off: bool) -> Result<()> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // El tamaño incluye el NUL final (documentado) y va en bytes, como en PowerReadFriendlyName.
    let bytes = (wide.len() * size_of::<u16>()) as u32;
    // SAFETY: `wide` vive durante la llamada y tiene `bytes` bytes; con subgrupo y ajuste nulos
    // se nombra el plan.
    check(
        unsafe {
            PowerWriteFriendlyName(
                null_mut(),
                guid,
                null(),
                null(),
                wide.as_ptr().cast(),
                bytes,
            )
        },
        "nombrar el plan de energía",
    )?;
    let idle = cpu_idle_off.then_some(&CPU_IDLE_OFF);
    for (subgroup, setting, value) in CUSTOM_AC_VALUES.iter().chain(idle) {
        // SAFETY: los tres GUID viven durante la llamada.
        check(
            unsafe { PowerWriteACValueIndex(null_mut(), guid, subgroup, setting, *value) },
            "escribir un valor de CA del plan de energía",
        )?;
    }
    Ok(())
}

/// Microsoft no documenta qué pasa al borrar el plan activo, así que se rechaza antes.
pub(super) fn delete_power_plan(id: &str) -> Result<()> {
    let guid = existing_guid(id)?;
    if active_power_plan()?.id == format_guid(&guid) {
        return Err(PlatformError::Os(format!(
            "el plan de energía `{id}` está activo y no se borra"
        )));
    }
    // SAFETY: `guid` es un local vivo durante la llamada.
    check(
        unsafe { PowerDeleteScheme(null_mut(), &guid) },
        "borrar el plan de energía",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    /// Los tests que leen o cambian los planes del equipo van de uno en uno: duplicar el plan
    /// activo a la vez que se reactiva hace fallar a uno de los dos.
    fn system_plans() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

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
        let _lock = system_plans();
        let plans = power_plans().unwrap();
        assert!(!plans.is_empty());
        assert!(plans.iter().all(|p| !p.name.is_empty()));
        let active = active_power_plan().unwrap();
        assert!(plans.contains(&active), "{active:?} no está en {plans:?}");
        set_active_power_plan(&active.id).unwrap();
        assert_eq!(active_power_plan().unwrap(), active);
    }

    /// Crea un plan propio a partir del activo, comprueba que existe y lo borra. No lo activa,
    /// así que no cambia el plan del equipo.
    #[test]
    fn custom_plan_is_created_and_deleted() {
        let _lock = system_plans();
        let active = active_power_plan().unwrap();
        let custom = create_custom_power_plan(&active.id, "FrameRush test", true).unwrap();
        assert_eq!(custom.name, "FrameRush test");
        let (subgroup, setting, value) = CPU_IDLE_OFF;
        let guid = parse_guid(&custom.id).unwrap();
        let mut read = u32::MAX;
        // SAFETY: los GUID y `read` viven durante la llamada.
        let code = unsafe {
            windows_sys::Win32::System::Power::PowerReadACValueIndex(
                null_mut(),
                &guid,
                &subgroup,
                &setting,
                &mut read,
            )
        };
        assert_eq!(code, ERROR_SUCCESS);
        assert_eq!(read, value, "C-states desactivados en CA");
        assert!(power_plans().unwrap().contains(&custom));
        assert!(delete_power_plan(&active.id).is_err(), "no borra el activo");
        delete_power_plan(&custom.id).unwrap();
        assert!(!power_plans().unwrap().contains(&custom));
        assert_eq!(active_power_plan().unwrap(), active);
    }

    #[test]
    fn refuses_a_plan_that_does_not_exist() {
        assert!(set_active_power_plan("00000000-0000-0000-0000-000000000001").is_err());
        assert!(set_active_power_plan("alto").is_err());
    }
}
