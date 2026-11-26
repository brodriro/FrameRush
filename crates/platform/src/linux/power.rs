//! Perfiles de energía con power-profiles-daemon por D-Bus (V16).
//!
//! Desde la v0.20 el servicio es `org.freedesktop.UPower.PowerProfiles` en
//! `/org/freedesktop/UPower/PowerProfiles` (referencia gdbus del proyecto); antes era
//! `net.hadess.PowerProfiles` en `/net/hadess/PowerProfiles` (plantilla de python-dbusmock), que
//! se mantiene por compatibilidad. Se prueba primero el nuevo. `tuned-ppd` expone la misma API.
//!
//! El id y el nombre de cada plan son el valor de la clave `Profile` («power-saver», «balanced»
//! o «performance»): el servicio no da un nombre para mostrar. No hay planes propios.

use super::dbus;
use crate::{PlatformError, PowerPlan, Result};
use std::collections::HashMap;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

const NAMES: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];
const WHAT: &str = "power-profiles-daemon";

/// Proxy al primer nombre que responde. Sin ninguno, no está instalado o no está en marcha.
fn connect<T>(read: impl Fn(&Proxy<'_>) -> zbus::Result<T>) -> Result<T> {
    let conn: Connection = dbus::system(WHAT)?;
    for (name, path) in NAMES {
        let proxy = dbus::proxy(&conn, name, path, name).map_err(|e| dbus::error(WHAT, e))?;
        match read(&proxy) {
            Ok(v) => return Ok(v),
            Err(e) if dbus::is_unknown_service(&e) => continue,
            Err(e) => return Err(dbus::error(WHAT, e)),
        }
    }
    Err(PlatformError::Unsupported(WHAT))
}

/// Perfiles de la propiedad `Profiles` (`aa{sv}`), en el orden que da el servicio.
fn plans(profiles: &[HashMap<String, OwnedValue>]) -> Vec<PowerPlan> {
    profiles
        .iter()
        .filter_map(|p| String::try_from(p.get("Profile")?.try_clone().ok()?).ok())
        .map(|id| PowerPlan {
            name: id.clone(),
            id,
        })
        .collect()
}

fn plan(id: String) -> PowerPlan {
    PowerPlan {
        name: id.clone(),
        id,
    }
}

pub(super) fn power_plans() -> Result<Vec<PowerPlan>> {
    let profiles: Vec<HashMap<String, OwnedValue>> = connect(|p| p.get_property("Profiles"))?;
    Ok(plans(&profiles))
}

pub(super) fn active_power_plan() -> Result<PowerPlan> {
    connect(|p| p.get_property::<String>("ActiveProfile")).map(plan)
}

/// `ActiveProfile` es de lectura y escritura: es como cambian el perfil las interfaces del
/// escritorio (referencia gdbus).
pub(super) fn set_active_power_plan(id: &str) -> Result<()> {
    if !power_plans()?.iter().any(|p| p.id == id) {
        return Err(PlatformError::NotFound(format!("perfil de energía {id}")));
    }
    let conn = dbus::system(WHAT)?;
    for (name, path) in NAMES {
        let proxy = dbus::proxy(&conn, name, path, name).map_err(|e| dbus::error(WHAT, e))?;
        match proxy.set_property("ActiveProfile", id) {
            Ok(()) => return Ok(()),
            Err(zbus::fdo::Error::ServiceUnknown(_)) => continue,
            Err(e) => return Err(dbus::fdo_error(WHAT, e)),
        }
    }
    Err(PlatformError::Unsupported(WHAT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn profile(name: &str) -> HashMap<String, OwnedValue> {
        let mut p = HashMap::new();
        p.insert(
            "Profile".to_owned(),
            Value::from(name).try_to_owned().unwrap(),
        );
        p.insert(
            "Driver".to_owned(),
            Value::from("platform_profile").try_to_owned().unwrap(),
        );
        p
    }

    #[test]
    fn each_profile_is_a_plan_named_after_it_in_the_daemon_order() {
        let list = [
            profile("power-saver"),
            profile("balanced"),
            profile("performance"),
        ];
        let ids: Vec<_> = plans(&list).into_iter().map(|p| (p.id, p.name)).collect();
        assert_eq!(
            ids,
            [
                ("power-saver".into(), "power-saver".into()),
                ("balanced".into(), "balanced".into()),
                ("performance".into(), "performance".into()),
            ]
        );
        // Sin la clave `Profile`, se salta.
        assert!(plans(&[HashMap::new()]).is_empty());
    }
}
