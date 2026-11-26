//! Bus del sistema de D-Bus (systemd y power-profiles-daemon) con la API bloqueante de `zbus`.

use crate::{PlatformError, Result};
use zbus::DBusError;
use zbus::blocking::{Connection, Proxy, proxy::Builder};
use zbus::proxy::CacheProperties;

/// Errores de D-Bus que significan «no tienes permiso»: la política del bus o polkit lo negó, o
/// polkit pide autenticarse (org.freedesktop.systemd1(5): sin privilegios, cambiar el estado de
/// una unidad pide `org.freedesktop.systemd1.manage-units`).
const DENIED: [&str; 2] = [
    "org.freedesktop.DBus.Error.AccessDenied",
    "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
];

/// El servicio de D-Bus no está en el bus (no instalado o no arrancado).
const UNKNOWN_SERVICE: &str = "org.freedesktop.DBus.Error.ServiceUnknown";

/// Nombre del error de D-Bus, si `e` lo trae.
fn error_name(e: &zbus::Error) -> Option<String> {
    match e {
        zbus::Error::MethodError(name, ..) => Some(name.as_str().to_owned()),
        zbus::Error::FDO(f) => Some(f.name().as_str().to_owned()),
        _ => None,
    }
}

pub(super) fn error(what: &str, e: zbus::Error) -> PlatformError {
    match error_name(&e) {
        Some(name) if DENIED.contains(&name.as_str()) => PlatformError::AccessDenied(what.into()),
        _ => PlatformError::Os(format!("{what}: {e}")),
    }
}

pub(super) fn fdo_error(what: &str, e: zbus::fdo::Error) -> PlatformError {
    error(what, zbus::Error::FDO(Box::new(e)))
}

/// Si `e` es que el servicio no está en el bus.
pub(super) fn is_unknown_service(e: &zbus::Error) -> bool {
    error_name(e).as_deref() == Some(UNKNOWN_SERVICE)
}

pub(super) fn system(what: &str) -> Result<Connection> {
    Connection::system().map_err(|e| error(what, e))
}

/// Proxy sin caché de propiedades: cada lectura pregunta al servicio.
pub(super) fn proxy<'a>(
    conn: &Connection,
    destination: &'a str,
    path: &'a str,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    Builder::<Proxy>::new(conn)
        .destination(destination)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}
