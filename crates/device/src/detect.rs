//! Detección del `DeviceProfile` a partir de la plataforma (R8).
//!
//! Lo que falla se queda en `None` o vacío, nunca en un valor inventado: el ajuste que depende
//! de ello no se muestra o su valor por defecto sale como estimado (P15).

use crate::DeviceProfile;
use framerush_platform::{DisplayControl, Inventory, Os, PowerControl};

/// Perfil del equipo con lo que la plataforma sabe leer. Todavía no rellena build del SO,
/// fabricante, apps, VBS/HVCI, HAGS, antivirus ni virtualización.
pub fn detect<P: PowerControl + DisplayControl + Inventory>(platform: &P, os: Os) -> DeviceProfile {
    let mut device = DeviceProfile::undetected(os);
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
    }
}
