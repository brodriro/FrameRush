//! Valores por defecto calculados a partir del equipo (PLAN.md, Iteración 6, R8).
//!
//! Cada función aplica la regla de cálculo de la tabla de It6. Cuando falta el dato, devuelve
//! el fallback de esa tabla marcado como estimado ([`Sourced::estimated`]), o `None` si el
//! ajuste no debe mostrarse (P15).

use crate::Level;
use framerush_device::{CoreKind, DeviceProfile, Gpu, Sourced};
use framerush_platform::{Display, Os, PowerPlan};

/// Fallback del umbral de alerta térmica (It6).
pub const FALLBACK_ALERT_C: f32 = 85.0;
/// Distancia entre el aviso (ámbar) y la alerta (It6).
pub const WARN_BELOW_ALERT_C: f32 = 15.0;

/// Apps de voz que se mantienen vivas por defecto (It6, `keep-alive-list`). Se comparan sin
/// distinguir mayúsculas como subcadena del nombre detectado.
const VOICE_APPS: &[&str] = &["discord", "teamspeak", "mumble", "teams"];

/// GUID de los planes de Windows que usa la referencia CS2-FPS-Boost.
const WIN_HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
const WIN_ULTIMATE_PERFORMANCE: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";
/// Perfil de power-profiles-daemon de mayor rendimiento.
const PPD_PERFORMANCE: &str = "performance";

/// `thermal-alert-threshold`: el menor de (TjMax − 10 °C, slowdown de la GPU − 5 °C).
pub fn thermal_alert_c(device: &DeviceProfile) -> Sourced<f32> {
    let cpu = device
        .cpu
        .as_ref()
        .and_then(|c| c.tjmax_c)
        .map(|t| t - 10.0);
    let gpu = device
        .gpus
        .iter()
        .filter_map(|g| g.slowdown_c)
        .map(|s| s - 5.0)
        .reduce(f32::min);
    match (cpu, gpu) {
        (Some(c), Some(g)) => Sourced::detected(c.min(g)),
        (Some(v), None) | (None, Some(v)) => Sourced::detected(v),
        (None, None) => Sourced::estimated(FALLBACK_ALERT_C),
    }
}

/// `thermal-warn-threshold`: la alerta − 15 °C, con el mismo origen.
pub fn thermal_warn_c(device: &DeviceProfile) -> Sourced<f32> {
    let alert = thermal_alert_c(device);
    Sourced {
        value: alert.value - WARN_BELOW_ALERT_C,
        source: alert.source,
    }
}

/// `keep-alive-list`: los tokens de [`VOICE_APPS`] de las apps de voz detectadas. El plan los
/// busca como subcadena del nombre del proceso ("Microsoft Teams" instalado protege
/// `ms-teams.exe`).
///
/// Sin apps detectadas (perfil sin detectar) el fallback, marcado como estimado, son todos los
/// tokens: es más seguro no congelar una app de voz que congelarla (P7, P15).
pub fn keep_alive(device: &DeviceProfile) -> Sourced<Vec<String>> {
    if device.installed_apps.is_empty() {
        return Sourced::estimated(VOICE_APPS.iter().map(|v| (*v).to_owned()).collect());
    }
    Sourced::detected(
        VOICE_APPS
            .iter()
            .filter(|v| {
                device
                    .installed_apps
                    .iter()
                    .any(|app| app.to_lowercase().contains(*v))
            })
            .map(|v| (*v).to_owned())
            .collect(),
    )
}

/// Plan de energía del nivel (It6: "Máximo > Alto > actual"), o `None` si no hay que cambiarlo.
///
/// Normal no cambia el plan; Agresivo usa "Alto rendimiento" y Extremo "Máximo rendimiento"
/// (o "Alto" si no existe). Solo se eligen planes que existen en el equipo.
pub fn power_plan_for(level: Level, device: &DeviceProfile) -> Option<&PowerPlan> {
    let plan = level_plan(level, device)?;
    (device.active_power_plan.as_deref() != Some(plan.id.as_str())).then_some(plan)
}

/// El plan del nivel si existe en el equipo, esté activo o no.
fn level_plan(level: Level, device: &DeviceProfile) -> Option<&PowerPlan> {
    let wanted: &[&str] = match (device.os, level) {
        (_, Level::Normal) | (Os::Macos, _) => &[],
        (Os::Windows, Level::Agresivo) => &[WIN_HIGH_PERFORMANCE],
        (Os::Windows, Level::Extremo) => &[WIN_ULTIMATE_PERFORMANCE, WIN_HIGH_PERFORMANCE],
        (Os::Linux, _) => &[PPD_PERFORMANCE],
    };
    wanted.iter().find_map(|id| {
        device
            .power_plans
            .iter()
            .find(|p| p.id.eq_ignore_ascii_case(id))
    })
}

/// `custom-power-plan` (P3; Windows, desde Agresivo): el plan que se duplica como
/// [`crate::CUSTOM_POWER_PLAN_NAME`]. Es el del nivel aunque ya esté activo y, si el equipo no
/// tiene ninguno de los del nivel, el activo (como la referencia). Nunca un plan propio que
/// haya quedado de antes.
pub fn custom_power_plan_base(level: Level, device: &DeviceProfile) -> Option<&PowerPlan> {
    if device.os != Os::Windows || level == Level::Normal {
        return None;
    }
    let active = || {
        let id = device.active_power_plan.as_deref()?;
        device.power_plans.iter().find(|p| p.id == id)
    };
    level_plan(level, device)
        .or_else(active)
        .filter(|p| p.name != crate::CUSTOM_POWER_PLAN_NAME)
}

/// `refresh-rate-max` (Windows, desde Agresivo): cada monitor que no está a su máxima
/// frecuencia con la resolución actual, con esa frecuencia. Si ya están al máximo, la lista
/// sale vacía y el ajuste se oculta (It6).
pub fn refresh_rate_max(level: Level, device: &DeviceProfile) -> Vec<(&Display, u32)> {
    if device.os != Os::Windows || level == Level::Normal {
        return vec![];
    }
    device
        .displays
        .iter()
        .filter_map(|d| {
            let max = d.max_hz_at_current_resolution()?;
            (max > d.current.hz).then_some((d, max))
        })
        .collect()
}

/// `gpu-preference`: la GPU con más VRAM dedicada, solo si hay más de una.
pub fn gpu_preference(device: &DeviceProfile) -> Option<&Gpu> {
    if device.gpus.len() < 2 {
        return None;
    }
    device
        .gpus
        .iter()
        .filter(|g| g.dedicated_vram_bytes.is_some())
        .max_by_key(|g| g.dedicated_vram_bytes)
}

/// `cpu-affinity-cores`: todas las CPU lógicas menos las del primer núcleo físico y las de los
/// E-cores. Sin topología, el fallback es "todas", que se devuelve como lista vacía (sin
/// restricción) y marcado como estimado.
pub fn affinity_cpus(device: &DeviceProfile) -> Sourced<Vec<usize>> {
    let Some(cpu) = device.cpu.as_ref().filter(|c| !c.cores.is_empty()) else {
        return Sourced::estimated(vec![]);
    };
    let mut cpus: Vec<usize> = cpu
        .cores
        .iter()
        .skip(1)
        .filter(|c| c.kind == CoreKind::Performance)
        .flat_map(|c| c.logical.iter().copied())
        .collect();
    if cpus.is_empty() {
        cpus = cpu
            .cores
            .iter()
            .flat_map(|c| c.logical.iter().copied())
            .collect();
    }
    cpus.sort_unstable();
    Sourced::detected(cpus)
}

/// Aviso de afinidad: la CPU tiene 4 núcleos físicos o menos. `None` si no se conoce.
pub fn affinity_warning(device: &DeviceProfile) -> Option<bool> {
    device
        .cpu
        .as_ref()
        .and_then(|c| c.physical_cores())
        .map(|n| n <= 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_device::samples;

    #[test]
    fn thermal_thresholds_come_from_the_device() {
        // TjMax 100 − 10 = 90; slowdown 98 − 5 = 93 → 90.
        let laptop = samples::hybrid_laptop();
        assert_eq!(thermal_alert_c(&laptop), Sourced::detected(90.0));
        assert_eq!(thermal_warn_c(&laptop), Sourced::detected(75.0));
        // Solo TjMax 95 → 85, detectado.
        assert_eq!(
            thermal_alert_c(&samples::amd_desktop_linux()),
            Sourced::detected(85.0)
        );
    }

    #[test]
    fn thermal_thresholds_fall_back_when_nothing_is_readable() {
        for d in [samples::without_sensors(), samples::apple_silicon_mac()] {
            assert_eq!(thermal_alert_c(&d), Sourced::estimated(85.0));
            assert_eq!(thermal_warn_c(&d), Sourced::estimated(70.0));
        }
    }

    #[test]
    fn keep_alive_lists_detected_voice_apps() {
        assert_eq!(
            keep_alive(&samples::hybrid_laptop()),
            Sourced::detected(vec!["discord".into()])
        );
        assert_eq!(
            keep_alive(&samples::amd_desktop_linux()).value,
            ["discord", "mumble"]
        );
        let teams = DeviceProfile {
            installed_apps: vec!["Microsoft Teams".into(), "Steam".into()],
            ..DeviceProfile::undetected(Os::Windows)
        };
        assert_eq!(keep_alive(&teams).value, ["teams"]);
    }

    #[test]
    fn keep_alive_falls_back_to_every_voice_app() {
        let k = keep_alive(&DeviceProfile::undetected(Os::Windows));
        assert!(k.is_estimated());
        assert_eq!(k.value, VOICE_APPS);
    }

    #[test]
    fn power_plan_only_uses_existing_plans() {
        let laptop = samples::hybrid_laptop();
        assert_eq!(power_plan_for(Level::Normal, &laptop), None);
        let high = power_plan_for(Level::Agresivo, &laptop).unwrap();
        assert_eq!(high.id, WIN_HIGH_PERFORMANCE);
        // No hay "Máximo": Extremo usa "Alto".
        assert_eq!(power_plan_for(Level::Extremo, &laptop), Some(high));
        // Sin "Alto rendimiento" no se cambia nada.
        assert_eq!(
            power_plan_for(Level::Extremo, &samples::desktop_without_battery()),
            None
        );
        let linux = samples::amd_desktop_linux();
        assert_eq!(
            power_plan_for(Level::Agresivo, &linux).map(|p| p.id.as_str()),
            Some("performance")
        );
        assert_eq!(
            power_plan_for(Level::Extremo, &samples::apple_silicon_mac()),
            None
        );
    }

    #[test]
    fn power_plan_is_none_when_already_active() {
        let mut d = samples::hybrid_laptop();
        d.active_power_plan = Some(WIN_HIGH_PERFORMANCE.into());
        assert_eq!(power_plan_for(Level::Agresivo, &d), None);
    }

    #[test]
    fn refresh_rate_max_only_where_it_can_go_up() {
        let laptop = samples::hybrid_laptop();
        assert!(refresh_rate_max(Level::Normal, &laptop).is_empty());
        let up = refresh_rate_max(Level::Agresivo, &laptop);
        assert_eq!(up.len(), 1);
        assert_eq!(
            up[0].1,
            laptop.displays[0].max_hz_at_current_resolution().unwrap()
        );
        assert!(up[0].1 > laptop.displays[0].current.hz);
        // Ya al máximo: se oculta.
        let mut at_max = laptop.clone();
        at_max.displays[0].current.hz = up[0].1;
        assert!(refresh_rate_max(Level::Extremo, &at_max).is_empty());
        // Solo Windows.
        let mut linux = laptop;
        linux.os = Os::Linux;
        assert!(refresh_rate_max(Level::Extremo, &linux).is_empty());
    }

    #[test]
    fn custom_plan_base_is_the_level_plan_or_the_active_one() {
        let mut laptop = samples::hybrid_laptop();
        assert_eq!(custom_power_plan_base(Level::Normal, &laptop), None);
        // El plan del nivel, aunque ya sea el activo.
        laptop.active_power_plan = Some(WIN_HIGH_PERFORMANCE.into());
        let base = custom_power_plan_base(Level::Agresivo, &laptop).unwrap();
        assert_eq!(base.id, WIN_HIGH_PERFORMANCE);
        // Sin los planes del nivel, el activo.
        let desktop = samples::desktop_without_battery();
        assert_eq!(
            custom_power_plan_base(Level::Extremo, &desktop).map(|p| p.id.as_str()),
            desktop.active_power_plan.as_deref()
        );
        assert!(desktop.active_power_plan.is_some());
        // Solo Windows.
        let linux = samples::amd_desktop_linux();
        assert_eq!(custom_power_plan_base(Level::Extremo, &linux), None);
        // No se duplica un plan propio que quedó de antes.
        let mut stale = DeviceProfile::undetected(Os::Windows);
        stale.power_plans = vec![PowerPlan {
            id: "x".into(),
            name: crate::CUSTOM_POWER_PLAN_NAME.into(),
        }];
        stale.active_power_plan = Some("x".into());
        assert_eq!(custom_power_plan_base(Level::Agresivo, &stale), None);
    }

    #[test]
    fn gpu_preference_needs_two_gpus() {
        assert_eq!(
            gpu_preference(&samples::hybrid_laptop()).map(|g| g.id.as_str()),
            Some("gpu-dedicada")
        );
        assert_eq!(gpu_preference(&samples::amd_desktop_linux()), None);
    }

    #[test]
    fn affinity_skips_first_core_and_efficiency_cores() {
        // 4 núcleos con SMT: fuera el núcleo 0 (CPU 0 y 1).
        assert_eq!(
            affinity_cpus(&samples::hybrid_laptop()),
            Sourced::detected(vec![2, 3, 4, 5, 6, 7])
        );
        // 6P (SMT) + 4E: fuera el núcleo 0 y los E-cores 12..16.
        assert_eq!(
            affinity_cpus(&samples::amd_desktop_linux()).value,
            (2..12).collect::<Vec<_>>()
        );
        let unknown = affinity_cpus(&samples::without_sensors());
        assert!(unknown.is_estimated() && unknown.value.is_empty());
    }

    #[test]
    fn affinity_warning_on_four_cores_or_fewer() {
        assert_eq!(affinity_warning(&samples::hybrid_laptop()), Some(true));
        assert_eq!(affinity_warning(&samples::amd_desktop_linux()), Some(false));
        assert_eq!(affinity_warning(&samples::without_sensors()), None);
    }
}
