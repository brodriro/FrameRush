use framerush_platform::{Battery, Cpu, Display, Gpu, Os, PowerPlan};
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
    /// Si es un portátil (`PCSystemType` en Windows). Sin sensores de ventilador, la UI lo
    /// usa para explicar por qué no hay RPM.
    pub laptop: Option<bool>,
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
    /// Antivirus registrados en el SO (Security Center en Windows). Solo el nombre: si hay más
    /// de uno no se sabe cuál está activo.
    pub antivirus: Vec<String>,
    /// Software de virtualización instalado que se rompe con el hipervisor desactivado (V6).
    pub virtualization: Vec<String>,
}

impl DeviceProfile {
    /// Perfil sin detectar: solo el SO. Todo lo demás falta, así que los valores por defecto
    /// salen como estimados y los ajustes que dependen del equipo no se muestran (P15).
    pub fn undetected(os: Os) -> Self {
        Self {
            os,
            os_build: None,
            windows_11: None,
            manufacturer: None,
            laptop: None,
            cpu: None,
            gpus: vec![],
            displays: vec![],
            battery: None,
            power_plans: vec![],
            active_power_plan: None,
            installed_apps: vec![],
            vbs_hvci_enabled: None,
            hags_supported: None,
            antivirus: vec![],
            virtualization: vec![],
        }
    }
}
