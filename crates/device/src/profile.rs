use fpsboost_platform::{Os, PowerPlan};
use serde::{Deserialize, Serialize};

/// De dónde sale un valor que se muestra o se usa como valor por defecto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Leído o calculado a partir del equipo.
    Detected,
    /// Fallback fijo porque la detección falló; la UI lo marca como "estimado".
    Estimated,
}

/// Un valor junto a su origen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Sourced<T> {
    pub fn detected(value: T) -> Self {
        Self {
            value,
            source: Source::Detected,
        }
    }

    pub fn estimated(value: T) -> Self {
        Self {
            value,
            source: Source::Estimated,
        }
    }

    pub fn is_estimated(&self) -> bool {
        self.source == Source::Estimated
    }
}

/// Inventario del equipo (R8, Iteración 6). Lo detecta `device` al arrancar y lo consultan
/// `core`, `rules`, `games` y la UI; ningún otro módulo detecta por su cuenta.
///
/// Todo lo que puede faltar es `Option` o una lista vacía: `None` significa "no se pudo
/// detectar" o "no existe", y en ambos casos el ajuste que depende de ello no se muestra (P15).
/// El estado de los servicios no está aquí porque cambia en caliente: va en el snapshot del scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub os: Os,
    /// Versión del SO tal como la da el sistema (p. ej. el número de build en Windows).
    pub os_build: Option<String>,
    /// Solo en Windows: si es Windows 11 (decide `timer-resolution`).
    pub windows_11: Option<bool>,
    /// Fabricante del equipo (WMI/DMI), para las recomendaciones manuales.
    pub manufacturer: Option<String>,
    pub cpu: Option<Cpu>,
    pub gpus: Vec<Gpu>,
    pub displays: Vec<Display>,
    /// `None` si no hay batería o no se pudo leer.
    pub battery: Option<Battery>,
    /// Planes o perfiles de energía que existen en el equipo.
    pub power_plans: Vec<PowerPlan>,
    /// `id` del plan activo.
    pub active_power_plan: Option<String>,
    /// Apps instaladas o en ejecución, con el nombre que da el SO.
    pub installed_apps: Vec<String>,
    /// Estado de VBS/HVCI (Device Guard).
    pub vbs_hvci_enabled: Option<bool>,
    /// Si la GPU y el driver admiten HAGS.
    pub hags_supported: Option<bool>,
    /// Antivirus activo según el SO (Security Center en Windows).
    pub active_antivirus: Option<String>,
    /// Software de virtualización instalado que se rompe con el hipervisor desactivado (V6).
    pub virtualization: Vec<String>,
}

/// CPU y su topología.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    pub model: String,
    /// Núcleos físicos en orden. Vacío si no se pudo leer la topología.
    pub cores: Vec<Core>,
    /// CPU lógicas que ve el SO.
    pub logical_cpus: usize,
    /// Temperatura máxima de unión (MSR/LHM/hwmon `temp*_crit`).
    pub tjmax_c: Option<f32>,
}

impl Cpu {
    /// Núcleos físicos, si se conoce la topología.
    pub fn physical_cores(&self) -> Option<usize> {
        (!self.cores.is_empty()).then_some(self.cores.len())
    }
}

/// Un núcleo físico y las CPU lógicas (hilos SMT) que lo componen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Core {
    pub kind: CoreKind,
    pub logical: Vec<usize>,
}

/// Tipo de núcleo en CPUs híbridas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreKind {
    Performance,
    Efficiency,
}

/// Una GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gpu {
    /// Identificador estable del SO (LUID/ruta de dispositivo), para `gpu-preference`.
    pub id: String,
    pub name: String,
    pub vendor: GpuVendor,
    /// VRAM dedicada; `None` en GPUs integradas o si no se pudo leer.
    pub dedicated_vram_bytes: Option<u64>,
    /// Umbral de *slowdown* (NVML); puede no estar disponible (V13).
    pub slowdown_c: Option<f32>,
}

/// Fabricante de la GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

/// Un monitor y sus modos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub id: String,
    pub current: Mode,
    /// Modos que admite, tal como los lista el SO.
    pub modes: Vec<Mode>,
}

impl Display {
    /// Máxima frecuencia admitida con la resolución actual.
    pub fn max_hz_at_current_resolution(&self) -> Option<u32> {
        self.modes
            .iter()
            .filter(|m| m.width == self.current.width && m.height == self.current.height)
            .map(|m| m.hz)
            .max()
    }
}

/// Resolución y frecuencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

/// Estado de la batería.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub discharging: bool,
}
