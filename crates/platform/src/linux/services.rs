//! Servicios de las reglas con systemd por D-Bus (org.freedesktop.systemd1(5)).
//!
//! Un nombre de regla es el de la unidad (`cups.service`); sin sufijo se añade `.service`. Solo
//! se arrancan y detienen: nunca se cambia si se habilitan al inicio. Sin privilegios, systemd
//! pide `org.freedesktop.systemd1.manage-units` a polkit, así que fuera del helper suele dar
//! acceso denegado.

use super::dbus;
use crate::{PlatformError, Result, ServiceState};
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedObjectPath;

const DEST: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
/// Lo mismo que espera el SCM en Windows.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

fn unit_name(name: &str) -> String {
    if name.contains('.') {
        name.to_owned()
    } else {
        format!("{name}.service")
    }
}

/// `LoadState` (org.freedesktop.systemd1(5)): «loaded» y «masked» son unidades que existen; el
/// resto («error» u otros valores que pueda dar systemd) no se pueden gestionar y cuentan como
/// que no está.
fn exists(load_state: &str) -> bool {
    matches!(load_state, "loaded" | "masked")
}

/// `ActiveState` (org.freedesktop.systemd1(5), tabla «Unit ACTIVE states»).
fn state(active_state: &str) -> ServiceState {
    match active_state {
        "active" | "reloading" | "refreshing" => ServiceState::Running,
        "inactive" | "failed" | "maintenance" => ServiceState::Stopped,
        // activating, deactivating y los que añada systemd.
        _ => ServiceState::Pending,
    }
}

/// Propiedades `LoadState` y `ActiveState` de la unidad, cargándola si hace falta (`LoadUnit`).
fn read(name: &str) -> Result<(String, String)> {
    let unit = unit_name(name);
    let what = format!("systemd: {unit}");
    let conn = dbus::system(&what)?;
    let manager = dbus::proxy(&conn, DEST, PATH, MANAGER).map_err(|e| dbus::error(&what, e))?;
    let path: OwnedObjectPath = manager
        .call("LoadUnit", &(unit.as_str(),))
        .map_err(|e| dbus::error(&what, e))?;
    let unit_proxy =
        dbus::proxy(&conn, DEST, path.as_str(), UNIT).map_err(|e| dbus::error(&what, e))?;
    let load: String = unit_proxy
        .get_property("LoadState")
        .map_err(|e| dbus::error(&what, e))?;
    let active: String = unit_proxy
        .get_property("ActiveState")
        .map_err(|e| dbus::error(&what, e))?;
    Ok((load, active))
}

pub(super) fn service_state(name: &str) -> Result<Option<ServiceState>> {
    let (load, active) = read(name)?;
    Ok(exists(&load).then(|| state(&active)))
}

/// `mode` «replace»: arranca o para la unidad y sus dependencias, sustituyendo los trabajos en
/// cola que choquen (org.freedesktop.systemd1(5)).
fn job(method: &str, name: &str) -> Result<()> {
    let unit = unit_name(name);
    let what = format!("systemd: {unit}");
    let conn = dbus::system(&what)?;
    let manager = dbus::proxy(&conn, DEST, PATH, MANAGER).map_err(|e| dbus::error(&what, e))?;
    let _job: OwnedObjectPath = manager
        .call(method, &(unit.as_str(), "replace"))
        .map_err(|e| dbus::error(&what, e))?;
    Ok(())
}

/// Encola la parada y espera a que la unidad quede parada, como el SCM en Windows.
pub(super) fn stop_service(name: &str) -> Result<()> {
    job("StopUnit", name)?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        if !matches!(
            service_state(name)?,
            Some(ServiceState::Running | ServiceState::Pending)
        ) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(PlatformError::Os(format!(
                "systemd: {} no se detuvo en {} s",
                unit_name(name),
                STOP_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub(super) fn start_service(name: &str) -> Result<()> {
    job("StartUnit", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_name_without_suffix_is_a_service_unit() {
        assert_eq!(unit_name("cups"), "cups.service");
        assert_eq!(unit_name("cups.service"), "cups.service");
        assert_eq!(unit_name("cups.socket"), "cups.socket");
    }

    #[test]
    fn maps_the_documented_states() {
        assert!(exists("loaded") && exists("masked"));
        assert!(!exists("error") && !exists("not-found"));
        for s in ["active", "reloading", "refreshing"] {
            assert_eq!(state(s), ServiceState::Running, "{s}");
        }
        for s in ["inactive", "failed", "maintenance"] {
            assert_eq!(state(s), ServiceState::Stopped, "{s}");
        }
        for s in ["activating", "deactivating", "otro"] {
            assert_eq!(state(s), ServiceState::Pending, "{s}");
        }
    }
}
