//! Qué fuentes de sensores tiene el equipo. El orden es el de preferencia; el tacómetro de la
//! GPU por NVAPI es el último recurso y solo se consulta si ninguna otra fuente da ventiladores.

use crate::{Kind, NvidiaGpus, SensorProvider};
#[cfg(windows)]
use crate::{Reading, Sensor};

/// Proveedores del equipo y nombres de todas las fuentes consultadas, encontraran algo o no
/// (para explicar en la UI por qué falta un sensor).
pub struct Sources {
    pub providers: Vec<Box<dyn SensorProvider>>,
    pub checked: Vec<&'static str>,
}

impl std::fmt::Debug for Sources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sources")
            .field("providers", &self.providers.len())
            .field("checked", &self.checked)
            .finish()
    }
}

/// Abre las fuentes del equipo. Las que no están (sin GPU NVIDIA, por ejemplo) se saltan.
pub fn detect() -> Sources {
    let mut providers: Vec<Box<dyn SensorProvider>> = vec![];
    // En macOS no hay más fuentes que añadir después de NVML.
    #[cfg_attr(target_os = "macos", allow(unused_mut))]
    let mut checked = vec!["NVML"];
    if let Ok(gpus) = NvidiaGpus::open() {
        providers.push(Box::new(gpus));
    }
    #[cfg(target_os = "linux")]
    {
        checked.push("hwmon");
        providers.push(Box::new(crate::Hwmon::new()));
    }
    let has_fans = providers
        .iter_mut()
        .flat_map(|p| p.read())
        .any(|s| s.kind == Kind::Fan);
    #[cfg(windows)]
    if !has_fans {
        checked.push("NVAPI");
        if let Some(tach) = GpuTach::open() {
            providers.push(Box::new(tach));
        }
    }
    #[cfg(not(windows))]
    let _ = has_fans;
    Sources { providers, checked }
}

/// Tacómetro de las GPU NVIDIA por NVAPI: RPM medidas, no objetivo. Solo entran las GPU que
/// lo tienen; en un portátil NVAPI suele responder que no está admitido.
#[cfg(windows)]
struct GpuTach {
    nvapi: framerush_platform::Nvapi,
    /// Índice NVAPI y nombre de las GPU con tacómetro.
    gpus: Vec<(usize, String)>,
}

#[cfg(windows)]
impl GpuTach {
    fn open() -> Option<Self> {
        let nvapi = framerush_platform::Nvapi::open().ok()?;
        let gpus: Vec<(usize, String)> = nvapi
            .gpu_names()
            .iter()
            .enumerate()
            .filter(|(i, _)| matches!(nvapi.tach_rpm(*i), Ok(Some(_))))
            .map(|(i, n)| (i, (*n).to_owned()))
            .collect();
        (!gpus.is_empty()).then_some(Self { nvapi, gpus })
    }
}

#[cfg(windows)]
impl SensorProvider for GpuTach {
    fn read(&mut self) -> Vec<Sensor> {
        self.gpus
            .iter()
            .map(|(i, name)| Sensor {
                id: format!("nvapi:{i}:fan"),
                label: format!("{name} · ventilador"),
                kind: Kind::Fan,
                reading: self
                    .nvapi
                    .tach_rpm(*i)
                    .ok()
                    .flatten()
                    .map(|value| Reading::Rpm {
                        value,
                        target: false,
                    }),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NVML siempre se consulta; NVAPI solo en Windows y cuando nada más da ventiladores.
    #[test]
    fn checks_nvml_first_and_nvapi_only_as_a_last_resort() {
        let mut sources = detect();
        assert_eq!(sources.checked[0], "NVML");
        let fans_before_nvapi = sources
            .providers
            .iter_mut()
            .flat_map(|p| p.read())
            .filter(|s| s.kind == Kind::Fan && !s.id.starts_with("nvapi:"))
            .count();
        assert_eq!(
            sources.checked.contains(&"NVAPI"),
            cfg!(windows) && fans_before_nvapi == 0
        );
    }
}
