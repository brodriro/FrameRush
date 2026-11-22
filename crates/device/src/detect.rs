//! Detección del `DeviceProfile` a partir de la plataforma (R8).
//!
//! Lo que falla se queda en `None` o vacío, nunca en un valor inventado: el ajuste que depende
//! de ello no se muestra o su valor por defecto sale como estimado (P15).

use crate::DeviceProfile;
use framerush_platform::{DisplayControl, Inventory, Os, PowerControl};

/// Primer build de Windows 11: 21H2 es "OS build 22000" en "Windows 11 release information"
/// (Microsoft Learn); el último de Windows 10 es 19045.
const WINDOWS_11_FIRST_BUILD: u32 = 22000;

/// Si `build` ("26100" o "10.0.26100") es de Windows 11; `None` si no es un número.
fn is_windows_11(build: &str) -> Option<bool> {
    let number = build.rsplit('.').next()?.trim().parse::<u32>().ok()?;
    Some(number >= WINDOWS_11_FIRST_BUILD)
}

/// Perfil del equipo con lo que la plataforma sabe leer. Todavía no rellena HAGS, antivirus ni
/// virtualización.
pub fn detect<P: PowerControl + DisplayControl + Inventory>(platform: &P, os: Os) -> DeviceProfile {
    let mut device = DeviceProfile::undetected(os);
    device.os_build = platform.os_build().ok().flatten();
    if os == Os::Windows {
        device.windows_11 = device.os_build.as_deref().and_then(is_windows_11);
    }
    device.manufacturer = platform.manufacturer().ok().flatten();
    device.vbs_hvci_enabled = platform.vbs_running().ok().flatten();
    device.installed_apps = platform.installed_apps().unwrap_or_default();
    device.cpu = platform.cpu().ok().flatten();
    device.gpus = platform.gpus().unwrap_or_default();
    device.battery = platform.battery().ok().flatten();
    device.displays = platform.displays().unwrap_or_default();
    device.power_plans = platform.power_plans().unwrap_or_default();
    device.active_power_plan = platform.active_power_plan().ok().map(|p| p.id);
    device
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_platform::mock::MockPlatform;
    use framerush_platform::{Battery, Core, CoreKind, Cpu, PowerPlan};

    #[test]
    fn copies_what_the_platform_reads_and_leaves_the_rest_empty() {
        let m = MockPlatform::new(4);
        let empty = detect(&m, Os::Windows);
        assert_eq!(empty, DeviceProfile::undetected(Os::Windows));

        let cpu = Cpu {
            model: "cpu".into(),
            cores: vec![Core {
                kind: CoreKind::Performance,
                logical: vec![0, 1],
            }],
            logical_cpus: 2,
            tjmax_c: None,
        };
        m.set_inventory(
            Some(cpu.clone()),
            vec![],
            Some(Battery { discharging: true }),
        );
        let plan = PowerPlan {
            id: "a".into(),
            name: "A".into(),
        };
        m.set_power_plans(vec![plan.clone()], "a");
        let d = detect(&m, Os::Windows);
        assert_eq!(d.cpu, Some(cpu));
        assert_eq!(d.battery, Some(Battery { discharging: true }));
        assert_eq!(d.power_plans, [plan]);
        assert_eq!(d.active_power_plan.as_deref(), Some("a"));
        assert_eq!(d.manufacturer, None);
        assert_eq!(d.windows_11, None);
    }

    #[test]
    fn reads_the_system_and_derives_windows_11_from_the_build() {
        let m = MockPlatform::new(4);
        m.set_system(
            Some("22631"),
            Some("Acme"),
            Some(true),
            &["Discord", "Steam"],
        );
        let d = detect(&m, Os::Windows);
        assert_eq!(d.os_build.as_deref(), Some("22631"));
        assert_eq!(d.windows_11, Some(true));
        assert_eq!(d.manufacturer.as_deref(), Some("Acme"));
        assert_eq!(d.vbs_hvci_enabled, Some(true));
        assert_eq!(d.installed_apps, ["Discord", "Steam"]);

        // Windows 11 solo tiene sentido en Windows.
        assert_eq!(detect(&m, Os::Linux).windows_11, None);
    }

    #[test]
    fn windows_11_starts_at_build_22000() {
        assert_eq!(is_windows_11("19045"), Some(false));
        assert_eq!(is_windows_11("22000"), Some(true));
        assert_eq!(is_windows_11("10.0.26100"), Some(true));
        assert_eq!(is_windows_11("raro"), None);
    }
}
