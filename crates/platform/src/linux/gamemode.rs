//! `linux-gamemode`: Feral GameMode por D-Bus (PLAN.md §6, «GameMode (M5)»).
//!
//! Fuentes (repositorio FeralInteractive/gamemode, BSD-3-Clause):
//! - `daemon/gamemode-dbus.c`: bus de sesión, nombre, objeto e interfaz
//!   `com.feralinteractive.GameMode`; `RegisterGameByPID(ii) → i` y `UnregisterGameByPID(ii) → i`,
//!   con el PID del que lo pide y el del juego, en ese orden.
//! - `daemon/gamemode-context.c`: 0 es éxito, -1 un error (al quitar, también que no tenía ese
//!   proceso) y -2 un rechazo por la lista blanca o negra de supervisores. Cualquier supervisor
//!   puede quitar un juego (no tiene que ser el que lo registró), y GameMode quita por su cuenta
//!   los juegos cuyo proceso terminó.
//!
//! Disponibilidad: el nombre tiene dueño en el bus o se puede activar
//! (`NameHasOwner` y `ListActivatableNames`, especificación de D-Bus).

use super::dbus;
use crate::scan::target_of;
use crate::{PlatformError, Result, Target};
use zbus::blocking::fdo::DBusProxy;
use zbus::names::BusName;

const NAME: &str = "com.feralinteractive.GameMode";
const PATH: &str = "/com/feralinteractive/GameMode";
const WHAT: &str = "GameMode";
/// Rechazo por la configuración de supervisores de GameMode.
const REJECTED: i32 = -2;

pub(super) fn available() -> bool {
    let Ok(conn) = dbus::session(WHAT) else {
        return false;
    };
    let Ok(bus) = DBusProxy::new(&conn) else {
        return false;
    };
    let Ok(name) = BusName::try_from(NAME) else {
        return false;
    };
    bus.name_has_owner(name).unwrap_or(false)
        || bus
            .list_activatable_names()
            .is_ok_and(|names| names.iter().any(|n| n.as_str() == NAME))
}

/// Llama a `method` con (este proceso, el juego) y devuelve el código de GameMode.
fn call(method: &str, target: &Target) -> Result<i32> {
    let me = i32::try_from(std::process::id())
        .map_err(|_| PlatformError::Os(format!("{WHAT}: PID propio fuera de rango")))?;
    let game = i32::try_from(target.pid)
        .map_err(|_| PlatformError::NotFound(format!("PID {}", target.pid)))?;
    let conn = dbus::session(WHAT)?;
    let proxy = dbus::proxy(&conn, NAME, PATH, NAME).map_err(|e| dbus::error(WHAT, e))?;
    proxy.call(method, &(me, game)).map_err(|e| match e {
        e if dbus::is_unknown_service(&e) => PlatformError::Unsupported(WHAT),
        e => dbus::error(WHAT, e),
    })
}

pub(super) fn register(target: &Target) -> Result<()> {
    if target_of(target.pid).as_ref() != Some(target) {
        return Err(PlatformError::TargetChanged { pid: target.pid });
    }
    match call("RegisterGameByPID", target)? {
        0 => Ok(()),
        REJECTED => Err(PlatformError::AccessDenied(format!(
            "{WHAT} rechazó a FrameRush como supervisor (lista de supervisores de gamemode.ini)"
        ))),
        code => Err(PlatformError::Os(format!(
            "{WHAT}: RegisterGameByPID devolvió {code} para el PID {}",
            target.pid
        ))),
    }
}

/// -1 es también «no lo tenía» (p. ej. ya lo quitó GameMode al cerrarse el juego): no es un
/// error.
pub(super) fn unregister(target: &Target) -> Result<()> {
    match call("UnregisterGameByPID", target)? {
        REJECTED => Err(PlatformError::AccessDenied(format!(
            "{WHAT} rechazó a FrameRush como supervisor (lista de supervisores de gamemode.ini)"
        ))),
        _ => Ok(()),
    }
}
