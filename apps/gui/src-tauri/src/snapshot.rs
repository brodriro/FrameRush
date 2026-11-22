//! Snapshot del equipo para la UI (comando `snapshot`, design/README.md "Shared backend
//! contract"). Todo sale de este equipo: `device`, `scan`, `plan` y el journal (R8). Lo que no
//! se puede leer va como `null` o lista vacía y la UI lo oculta o muestra "No disponible"; aquí
//! no hay ningún valor de muestra.

use anyhow::{Context, Result, bail};
use framerush_core::{
    Category, EntryStatus, FileStore, JournalStore, Level, MemoryOp, Plan, PlanOptions,
    ProtectedReason, Snapshot, Subject, defaults, plan, protected_reason,
};
use framerush_device::{CoreKind, DeviceProfile, Sourced};
use framerush_platform::{Os, ProcessInfo, scan_processes};
use framerush_sensors::{Kind, Reading, Sensor};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// `win`, `lin` o `mac`: la clave que usa la UI para sus textos por SO.
    os: &'static str,
    caps: Caps,
    /// Ids de design/SETTINGS.md que este equipo y esta plataforma pueden aplicar (P15). La UI
    /// oculta los demás ajustes del sistema.
    available: Vec<&'static str>,
    device: Device,
    processes: Vec<Row>,
    /// Anticheats en ejecución (`plan.anticheats`): cuentan para cualquier juego (It1.3).
    anticheats: Vec<String>,
    /// Procesos protegidos que están en ejecución ahora mismo.
    protected: Vec<Protected>,
    /// Fuentes de sensores consultadas, encontraran algo o no (para explicar lo que falta).
    sensor_sources: Vec<&'static str>,
    /// Fuentes del equipo que solo se leen con el helper elevado (p. ej. el EC de Acer).
    sensor_needs_admin: Vec<&'static str>,
    /// Journal sin cerrar (restore-after-crash).
    pending_session: Option<Pending>,
    /// Hay un helper elevado abierto. La GUI sigue sin privilegios; de momento el helper solo
    /// lee sensores (aplicar aún no está conectado).
    elevated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Caps {
    windows: bool,
    affinity: bool,
    /// `timer-resolution` todavía no está implementado.
    timer: bool,
    /// `write` si se puede fijar la GPU del exe; `null` si no hay nada que elegir.
    gpu: Option<&'static str>,
    /// Fuente de FPS; `null` mientras no haya medición (PresentMon/MangoHud llegan en M4).
    fps: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    cpu: Option<CpuView>,
    gpus: Vec<GpuView>,
    manufacturer: Option<String>,
    /// `null` si el SO no dice si es un portátil.
    laptop: Option<bool>,
    displays: Vec<DisplayView>,
    /// `null` sin batería o si no se pudo leer.
    battery: Option<BatteryView>,
    battery_warning: bool,
    power: Option<Power>,
    vbs_active: Option<bool>,
    hags_supported: Option<bool>,
    windows11: Option<bool>,
    antivirus: Vec<String>,
    virtualization: Vec<String>,
    keep_alive: Estimate<Vec<String>>,
    thermal_alert: Estimate<f32>,
    thermal_warn: Estimate<f32>,
    affinity: Estimate<Vec<usize>>,
    affinity_warning: Option<bool>,
}

/// Valor con su origen: `estimated` = fallback porque la detección falló (R8).
#[derive(Debug, Serialize)]
struct Estimate<T> {
    value: T,
    estimated: bool,
}

impl<T> From<Sourced<T>> for Estimate<T> {
    fn from(s: Sourced<T>) -> Self {
        let estimated = s.is_estimated();
        Self {
            value: s.value,
            estimated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CpuView {
    model: String,
    threads: usize,
    tjmax: Option<f32>,
    /// Núcleos físicos con sus CPU lógicas; vacío si no se conoce la topología.
    cores: Vec<CoreView>,
}

#[derive(Debug, Serialize)]
struct CoreView {
    efficiency: bool,
    logical: Vec<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GpuView {
    name: String,
    vram_bytes: Option<u64>,
    slowdown: Option<f32>,
    /// La que elige `gpu-preference = alto` (más VRAM dedicada, con más de una GPU).
    preferred: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayView {
    id: String,
    width: u32,
    height: u32,
    hz: u32,
    /// Máximo con la resolución actual.
    max_hz: Option<u32>,
}

#[derive(Debug, Serialize)]
struct BatteryView {
    discharging: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Power {
    /// Nombre de cada plan; los repetidos llevan el principio de su id ([`plan_labels`]).
    plans: Vec<String>,
    /// Índice del plan activo en `plans`.
    current: Option<usize>,
    /// Plan que deja cada nivel (Normal, Agresivo, Extremo): el del nivel o el actual.
    for_level: [Option<usize>; 3],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    id: String,
    name: String,
    desc: String,
    category: &'static str,
    /// Acción en Extremo; la UI la degrada a suspender en los demás niveles (SETTINGS §4.2).
    action: framerush_core::Action,
    risk: framerush_core::Risk,
    min_level: Level,
    ram_bytes: u64,
    /// `null` si el SO no da el uso de CPU.
    cpu: Option<f32>,
    locked: bool,
    /// Marcada por defecto en Normal, Agresivo y Extremo.
    checked: [bool; 3],
}

#[derive(Debug, Serialize)]
struct Protected {
    name: String,
    reason: &'static str,
}

#[derive(Debug, Serialize)]
struct Pending {
    changes: usize,
}

/// Estado de las fuentes de sensores para el snapshot.
#[derive(Debug, Clone, Default)]
pub struct SensorInfo {
    pub checked: Vec<&'static str>,
    pub needs_admin: Vec<&'static str>,
    pub elevated: bool,
}

/// Lo que hace falta para planificar en este equipo, leído en el momento.
pub(crate) struct Inputs {
    pub os: Os,
    pub device: DeviceProfile,
    pub rules: Vec<framerush_core::Rule>,
    pub processes: Vec<ProcessInfo>,
    pub snapshot: Snapshot,
    pub tweaks: Tweaks,
    pub caps: framerush_platform::CapabilitySet,
}

/// Equipo, reglas, procesos y ajustes de ahora mismo. Tarda: fuera del hilo de la UI.
pub(crate) fn inputs() -> Result<Inputs> {
    let Some(os) = Os::current() else {
        bail!("sistema operativo no admitido");
    };
    let device = native_device(os);
    let rules = framerush_rules::embedded().context("las reglas embebidas no son válidas")?;
    let processes = scan_processes().context("no se pudo listar los procesos")?;
    let (tweaks, admin_tweaks, caps) = native_caps();
    let snapshot = Snapshot {
        processes: processes.clone(),
        // Los servicios se consultan con el SCM / systemd en M5.
        services: vec![],
        game: None,
        tweaks: tweaks.clone(),
        admin_tweaks,
        memory: if caps.memory {
            MemoryOp::ALL.to_vec()
        } else {
            vec![]
        },
    };
    Ok(Inputs {
        os,
        device,
        rules,
        processes,
        snapshot,
        tweaks,
        caps,
    })
}

/// Lee el equipo y arma el snapshot. Tarda (WMI, NVML, escaneo de procesos): se llama fuera
/// del hilo de la UI.
pub fn build(sensors: SensorInfo) -> Result<View> {
    let Inputs {
        os,
        device,
        rules,
        processes,
        snapshot,
        tweaks,
        caps,
    } = inputs()?;
    let options = PlanOptions {
        elevated: false,
        keep_alive: defaults::keep_alive(&device).value,
    };
    let plans: Vec<Plan> = Level::ALL
        .iter()
        .map(|l| plan(*l, &snapshot, &rules, &device, &options))
        .collect();

    let gpu_pref = defaults::gpu_preference(&device).map(|g| g.id.clone());
    let windows = os == Os::Windows;
    let mut available: Vec<&'static str> = tweaks.iter().map(|t| t.id()).collect();
    if caps.memory {
        available.extend(MemoryOp::ALL.iter().map(|m| m.id()));
    }
    if caps.power_plans && !device.power_plans.is_empty() {
        available.push("power-plan");
    }
    if caps.custom_power_plan {
        available.push("custom-power-plan");
    }
    if caps.refresh_rate && !defaults::refresh_rate_max(Level::Extremo, &device).is_empty() {
        available.push("refresh-rate-max");
    }
    if caps.affinity {
        available.push("cpu-affinity");
    }
    if windows {
        available.extend(["game-priority-persistent", "fullscreen-optimizations-off"]);
    }
    if windows && gpu_pref.is_some() {
        available.push("gpu-preference");
    }

    Ok(View {
        os: match os {
            Os::Windows => "win",
            Os::Linux => "lin",
            Os::Macos => "mac",
        },
        caps: Caps {
            windows,
            affinity: caps.affinity,
            timer: false,
            gpu: (windows && gpu_pref.is_some()).then_some("write"),
            fps: None,
        },
        available,
        processes: rows(&plans),
        anticheats: plans[0].anticheats.clone(),
        protected: protected(&processes),
        sensor_sources: sensors.checked,
        sensor_needs_admin: sensors.needs_admin,
        pending_session: pending_session(),
        elevated: sensors.elevated,
        device: device_view(&device, gpu_pref.as_deref()),
    })
}

/// Nombres para el selector. Si varios planes se llaman igual (copias de «Alto rendimiento»,
/// por ejemplo), cada uno lleva los 8 primeros caracteres de su id (el GUID en Windows) o,
/// si también coinciden, el id entero.
fn plan_labels(plans: &[framerush_platform::PowerPlan]) -> Vec<String> {
    const SHORT: usize = 8;
    let short = |p: &framerush_platform::PowerPlan| p.id.chars().take(SHORT).collect::<String>();
    plans
        .iter()
        .map(|p| {
            let same_name: Vec<_> = plans.iter().filter(|q| q.name == p.name).collect();
            if same_name.len() < 2 {
                return p.name.clone();
            }
            let suffix = if same_name.iter().filter(|q| short(q) == short(p)).count() > 1 {
                p.id.clone()
            } else {
                short(p)
            };
            format!("{} · {suffix}", p.name)
        })
        .collect()
}

fn device_view(d: &DeviceProfile, gpu_pref: Option<&str>) -> Device {
    let power = (!d.power_plans.is_empty()).then(|| {
        let index = |id: &str| d.power_plans.iter().position(|p| p.id == id);
        let current = d.active_power_plan.as_deref().and_then(index);
        let for_level = Level::ALL.map(|l| {
            defaults::power_plan_for(l, d)
                .and_then(|p| index(&p.id))
                .or(current)
        });
        Power {
            plans: plan_labels(&d.power_plans),
            current,
            for_level,
        }
    });
    Device {
        cpu: d.cpu.as_ref().map(|c| CpuView {
            model: c.model.clone(),
            threads: c.logical_cpus,
            tjmax: c.tjmax_c,
            cores: c
                .cores
                .iter()
                .map(|k| CoreView {
                    efficiency: k.kind == CoreKind::Efficiency,
                    logical: k.logical.clone(),
                })
                .collect(),
        }),
        gpus: d
            .gpus
            .iter()
            .map(|g| GpuView {
                name: g.name.clone(),
                vram_bytes: g.dedicated_vram_bytes,
                slowdown: g.slowdown_c,
                preferred: gpu_pref == Some(g.id.as_str()),
            })
            .collect(),
        manufacturer: d.manufacturer.clone(),
        laptop: d.laptop,
        displays: d
            .displays
            .iter()
            .map(|x| DisplayView {
                id: x.id.clone(),
                width: x.current.width,
                height: x.current.height,
                hz: x.current.hz,
                max_hz: x.max_hz_at_current_resolution(),
            })
            .collect(),
        battery: d.battery.map(|b| BatteryView {
            discharging: b.discharging,
        }),
        battery_warning: defaults::battery_warning(d),
        power,
        vbs_active: d.vbs_hvci_enabled,
        hags_supported: d.hags_supported,
        windows11: d.windows_11,
        antivirus: d.antivirus.clone(),
        virtualization: d.virtualization.clone(),
        keep_alive: defaults::keep_alive(d).into(),
        thermal_alert: defaults::thermal_alert_c(d).into(),
        thermal_warn: defaults::thermal_warn_c(d).into(),
        affinity: defaults::affinity_cpus(d).into(),
        affinity_warning: defaults::affinity_warning(d),
    }
}

/// Filas del checklist: las de Extremo (el plan solo decide qué viene marcado en cada nivel).
fn rows(plans: &[Plan]) -> Vec<Row> {
    let Some(extremo) = plans.last() else {
        return vec![];
    };
    extremo
        .items
        .iter()
        .map(|item| {
            let names: Vec<String> = match &item.subject {
                Subject::Processes(ps) => ps.iter().map(|p| p.name.clone()).collect(),
                Subject::Services(s) => s.clone(),
            };
            let mut unique = names.clone();
            unique.sort_unstable();
            unique.dedup();
            let name = match (unique.as_slice(), names.len()) {
                ([one], n) if n > 1 => format!("{one} ×{n}"),
                _ => unique.join(" · "),
            };
            let cpu = match &item.subject {
                Subject::Processes(ps) => {
                    ps.iter().filter_map(|p| p.cpu_percent).reduce(|a, b| a + b)
                }
                Subject::Services(_) => None,
            };
            let checked = [0, 1, 2].map(|i| {
                plans[i]
                    .items
                    .iter()
                    .any(|x| x.rule_id == item.rule_id && x.default_checked)
            });
            Row {
                id: item.rule_id.clone(),
                name,
                desc: item.description.clone(),
                category: category(item.category, item.min_level),
                action: item.action,
                risk: item.risk,
                min_level: item.min_level,
                ram_bytes: item.memory_bytes(),
                cpu,
                locked: item.locked,
                checked,
            }
        })
        .collect()
}

/// Grupo del checklist. Los servicios se parten en A (Agresivo) y B (Extremo) por `min_level`.
fn category(c: Category, min_level: Level) -> &'static str {
    match c {
        Category::Telemetry => "telemetry",
        Category::Launcher => "launcher",
        Category::Updater => "updater",
        Category::Overlay => "overlay",
        Category::Sync => "sync",
        Category::Peripheral => "peripheral",
        Category::Browser => "browser",
        Category::Service if min_level == Level::Extremo => "service_b",
        Category::Service => "service_a",
    }
}

fn protected(processes: &[ProcessInfo]) -> Vec<Protected> {
    let mut out: Vec<Protected> = processes
        .iter()
        .filter_map(|p| {
            let reason = match protected_reason(&p.name)? {
                ProtectedReason::OsCritical => "os_critical",
                ProtectedReason::Compositor => "compositor",
                ProtectedReason::Audio => "audio",
                ProtectedReason::GpuDriver => "gpu_driver",
                ProtectedReason::AntiCheat => "anti_cheat",
                ProtectedReason::SelfApp => "self_app",
            };
            Some(Protected {
                name: p.name.clone(),
                reason,
            })
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

/// Carpeta de datos, la misma que la CLI: `FRAMERUSH_DATA_DIR` o `%LOCALAPPDATA%/framerush`.
pub(crate) fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FRAMERUSH_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("framerush"))
}

fn pending_session() -> Option<Pending> {
    let journal = FileStore::new(data_dir()?.join("session.json"))
        .load()
        .ok()?;
    let changes = journal
        .entries
        .iter()
        .filter(|e| e.status != EntryStatus::Failed)
        .count();
    (changes > 0).then_some(Pending { changes })
}

/// Lectura de un sensor para el evento `sensors` (1 Hz). `value: null` = "No disponible".
#[derive(Debug, Clone, Serialize)]
pub struct SensorView {
    id: String,
    label: String,
    /// `NVML`, `hwmon`…: de dónde sale la lectura.
    source: &'static str,
    kind: &'static str,
    value: Option<f32>,
    /// `c`, `rpm` o `pct`.
    unit: Option<&'static str>,
    /// RPM objetivo del driver, no medidas (V13).
    target: bool,
}

pub fn sensor_views(sensors: &[Sensor]) -> Vec<SensorView> {
    sensors
        .iter()
        .map(|s| {
            let (value, unit, target) = match s.reading {
                Some(Reading::Celsius(c)) => (Some(c), Some("c"), false),
                Some(Reading::Rpm { value, target }) => (Some(value as f32), Some("rpm"), target),
                Some(Reading::Percent(p)) => (Some(p as f32), Some("pct"), false),
                None => (None, None, false),
            };
            SensorView {
                id: s.id.clone(),
                label: s.label.clone(),
                source: match s.id.split(':').next() {
                    Some("nvml") => "NVML",
                    Some("hwmon") => "hwmon",
                    Some("nvapi") => "NVAPI",
                    _ => "desconocida",
                },
                kind: match s.kind {
                    Kind::CpuTemperature => "cpu",
                    Kind::GpuTemperature => "gpu",
                    Kind::Fan => "fan",
                },
                value,
                unit,
                target,
            }
        })
        .collect()
}

/// Perfil del equipo con la plataforma nativa (R8), como en la CLI.
#[cfg(windows)]
fn native_device(os: Os) -> DeviceProfile {
    let mut device = framerush_device::detect(&framerush_platform::WindowsPlatform::new(), os);
    // El umbral de slowdown viene de NVML (sensores), no de la plataforma.
    if let Ok(nvidia) = framerush_sensors::NvidiaGpus::open() {
        nvidia.fill_slowdown(&mut device.gpus);
    }
    device
}

/// Fuera de Windows aún no hay plataforma nativa: solo el SO.
#[cfg(not(windows))]
fn native_device(os: Os) -> DeviceProfile {
    DeviceProfile::undetected(os)
}

type Tweaks = Vec<framerush_platform::Tweak>;

/// Ajustes que admite la plataforma, los que piden admin y sus capacidades.
#[cfg(windows)]
fn native_caps() -> (Tweaks, Tweaks, framerush_platform::CapabilitySet) {
    use framerush_platform::{Capabilities, SystemTweaks};
    let w = framerush_platform::WindowsPlatform::new();
    let all = w.supported_tweaks();
    let admin = all
        .iter()
        .copied()
        .filter(|t| w.tweak_requires_admin(*t))
        .collect();
    (all, admin, w.capabilities())
}

#[cfg(not(windows))]
fn native_caps() -> (Tweaks, Tweaks, framerush_platform::CapabilitySet) {
    (vec![], vec![], framerush_platform::CapabilitySet::default())
}

/// Solo en Windows: es la única plataforma con detección nativa por ahora.
#[cfg(test)]
mod label_tests {
    use super::plan_labels;
    use framerush_platform::PowerPlan;

    fn plan(id: &str, name: &str) -> PowerPlan {
        PowerPlan {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn repeated_names_carry_their_id() {
        let plans = [
            plan("381b4222-f694-41f0-9685-ff5bb260df2e", "Equilibrado"),
            plan("13a0df50-2343-4328-8946-57d564e2ad32", "Alto rendimiento"),
            plan("3d629b31-6d59-4f68-8990-494973f173a8", "Alto rendimiento"),
            plan("3d629b31-ffff-4f68-8990-494973f173a8", "Ahorro"),
            plan("aaaa0000-0000-0000-0000-000000000001", "Copia"),
            plan("aaaa0000-0000-0000-0000-000000000002", "Copia"),
        ];
        assert_eq!(
            plan_labels(&plans),
            [
                "Equilibrado",
                "Alto rendimiento · 13a0df50",
                "Alto rendimiento · 3d629b31",
                "Ahorro",
                "Copia · aaaa0000-0000-0000-0000-000000000001",
                "Copia · aaaa0000-0000-0000-0000-000000000002",
            ]
        );
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// El snapshot sale entero de este equipo: mismo SO, mismas GPU que la detección y ningún
    /// juego ni dato de muestra.
    #[test]
    fn this_machine_builds_a_snapshot() {
        let sources = framerush_sensors::detect();
        let view = build(SensorInfo {
            checked: sources.checked,
            needs_admin: sources.needs_admin,
            elevated: false,
        })
        .unwrap();
        assert_eq!(view.os, "win");
        let json = serde_json::to_value(&view).unwrap();
        assert!(json.get("games").is_none());
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    }
}
