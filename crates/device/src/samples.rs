//! Perfiles de ejemplo para tests (Iteración 6, punto 4), con la feature `samples`.
//!
//! Son equipos ficticios: los nombres y cifras no describen ningún modelo real, solo cubren
//! las situaciones que el código tiene que manejar.

use crate::{Battery, Core, CoreKind, Cpu, DeviceProfile, Display, Gpu, GpuVendor, Mode};
use framerush_platform::{Os, PowerPlan};

const GIB: u64 = 1024 * 1024 * 1024;

fn smt_cores(physical: usize, kind: CoreKind, first_logical: usize) -> Vec<Core> {
    (0..physical)
        .map(|i| Core {
            kind,
            logical: vec![first_logical + 2 * i, first_logical + 2 * i + 1],
        })
        .collect()
}

fn plan(id: &str, name: &str) -> PowerPlan {
    PowerPlan {
        id: id.into(),
        name: name.into(),
    }
}

/// Plan de prueba que hace de plan actual.
const TEST_BALANCED: &str = "plan-equilibrado-de-prueba";
/// GUID de "Alto rendimiento" que usa la referencia CS2-FPS-Boost (`$HighPerfGuid`).
const WIN_HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";

/// Portátil Windows 11 con GPU híbrida (integrada + dedicada), 4 núcleos con SMT,
/// batería descargándose, VBS activo y WSL2 instalado.
pub fn hybrid_laptop() -> DeviceProfile {
    DeviceProfile {
        os: Os::Windows,
        os_build: Some("10.0.26100".into()),
        windows_11: Some(true),
        manufacturer: Some("Fabricante de prueba".into()),
        cpu: Some(Cpu {
            model: "CPU de prueba 4C/8T".into(),
            cores: smt_cores(4, CoreKind::Performance, 0),
            logical_cpus: 8,
            tjmax_c: Some(100.0),
        }),
        gpus: vec![
            Gpu {
                id: "gpu-integrada".into(),
                name: "GPU integrada de prueba".into(),
                vendor: GpuVendor::Intel,
                dedicated_vram_bytes: None,
                slowdown_c: None,
            },
            Gpu {
                id: "gpu-dedicada".into(),
                name: "GPU dedicada de prueba".into(),
                vendor: GpuVendor::Nvidia,
                dedicated_vram_bytes: Some(4 * GIB),
                slowdown_c: Some(98.0),
            },
        ],
        displays: vec![Display {
            id: "pantalla-1".into(),
            current: Mode {
                width: 1920,
                height: 1080,
                hz: 60,
            },
            modes: vec![
                Mode {
                    width: 1920,
                    height: 1080,
                    hz: 60,
                },
                Mode {
                    width: 1920,
                    height: 1080,
                    hz: 75,
                },
                Mode {
                    width: 1280,
                    height: 960,
                    hz: 75,
                },
            ],
        }],
        battery: Some(Battery { discharging: true }),
        power_plans: vec![
            plan(TEST_BALANCED, "Equilibrado"),
            plan(WIN_HIGH_PERFORMANCE, "Alto rendimiento"),
        ],
        active_power_plan: Some(TEST_BALANCED.into()),
        installed_apps: vec!["Discord".into(), "Steam".into()],
        vbs_hvci_enabled: Some(true),
        hags_supported: Some(true),
        antivirus: vec!["Microsoft Defender Antivirus".into()],
        virtualization: vec!["WSL2".into()],
    }
}

/// Sobremesa Linux con CPU híbrida (P + E cores) y una sola GPU AMD, sin batería.
pub fn amd_desktop_linux() -> DeviceProfile {
    let mut cores = smt_cores(6, CoreKind::Performance, 0);
    cores.extend((0..4).map(|i| Core {
        kind: CoreKind::Efficiency,
        logical: vec![12 + i],
    }));
    DeviceProfile {
        os: Os::Linux,
        os_build: Some("6.12".into()),
        windows_11: None,
        manufacturer: None,
        cpu: Some(Cpu {
            model: "CPU híbrida de prueba 6P+4E".into(),
            cores,
            logical_cpus: 16,
            tjmax_c: Some(95.0),
        }),
        gpus: vec![Gpu {
            id: "card0".into(),
            name: "GPU AMD de prueba".into(),
            vendor: GpuVendor::Amd,
            dedicated_vram_bytes: Some(8 * GIB),
            slowdown_c: None,
        }],
        displays: vec![Display {
            id: "DP-1".into(),
            current: Mode {
                width: 2560,
                height: 1440,
                hz: 144,
            },
            modes: vec![Mode {
                width: 2560,
                height: 1440,
                hz: 144,
            }],
        }],
        battery: None,
        power_plans: vec![
            plan("power-saver", "power-saver"),
            plan("balanced", "balanced"),
            plan("performance", "performance"),
        ],
        active_power_plan: Some("balanced".into()),
        installed_apps: vec!["mumble".into(), "Discord".into()],
        vbs_hvci_enabled: None,
        hags_supported: None,
        antivirus: vec![],
        virtualization: vec![],
    }
}

/// Mac con Apple Silicon: sin TjMax ni umbral de GPU legibles y sin apps de voz.
pub fn apple_silicon_mac() -> DeviceProfile {
    let mut cores: Vec<Core> = (0..8)
        .map(|i| Core {
            kind: CoreKind::Performance,
            logical: vec![i],
        })
        .collect();
    cores.extend((8..12).map(|i| Core {
        kind: CoreKind::Efficiency,
        logical: vec![i],
    }));
    DeviceProfile {
        os: Os::Macos,
        os_build: Some("15.0".into()),
        windows_11: None,
        manufacturer: Some("Apple".into()),
        cpu: Some(Cpu {
            model: "SoC de prueba 8P+4E".into(),
            cores,
            logical_cpus: 12,
            tjmax_c: None,
        }),
        gpus: vec![Gpu {
            id: "gpu0".into(),
            name: "GPU integrada de prueba".into(),
            vendor: GpuVendor::Apple,
            dedicated_vram_bytes: None,
            slowdown_c: None,
        }],
        displays: vec![],
        battery: Some(Battery { discharging: false }),
        power_plans: vec![],
        active_power_plan: None,
        installed_apps: vec![],
        vbs_hvci_enabled: None,
        hags_supported: None,
        antivirus: vec![],
        virtualization: vec![],
    }
}

/// Sobremesa Windows sin batería ni "Alto rendimiento" disponible.
pub fn desktop_without_battery() -> DeviceProfile {
    DeviceProfile {
        battery: None,
        power_plans: vec![plan(TEST_BALANCED, "Balanced")],
        gpus: vec![hybrid_laptop().gpus.remove(1)],
        ..hybrid_laptop()
    }
}

/// Equipo en el que no se pudo leer ningún sensor ni la topología.
pub fn without_sensors() -> DeviceProfile {
    DeviceProfile {
        cpu: Some(Cpu {
            model: "CPU desconocida".into(),
            cores: vec![],
            logical_cpus: 4,
            tjmax_c: None,
        }),
        gpus: vec![Gpu {
            slowdown_c: None,
            ..hybrid_laptop().gpus.remove(1)
        }],
        ..desktop_without_battery()
    }
}

/// Todos los perfiles de ejemplo, para tests que recorren varios equipos.
pub fn all() -> Vec<(&'static str, DeviceProfile)> {
    vec![
        ("hybrid_laptop", hybrid_laptop()),
        ("amd_desktop_linux", amd_desktop_linux()),
        ("apple_silicon_mac", apple_silicon_mac()),
        ("desktop_without_battery", desktop_without_battery()),
        ("without_sensors", without_sensors()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_internally_consistent() {
        for (name, d) in all() {
            if let Some(cpu) = &d.cpu {
                let logical: usize = cpu.cores.iter().map(|c| c.logical.len()).sum();
                assert!(
                    cpu.cores.is_empty() || logical == cpu.logical_cpus,
                    "{name}: la topología no suma {}",
                    cpu.logical_cpus
                );
            }
            if let Some(active) = &d.active_power_plan {
                assert!(d.power_plans.iter().any(|p| &p.id == active), "{name}");
            }
        }
    }

    #[test]
    fn max_hz_uses_the_current_resolution() {
        let d = hybrid_laptop();
        assert_eq!(d.displays[0].max_hz_at_current_resolution(), Some(75));
    }
}
