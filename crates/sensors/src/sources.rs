//! Qué fuentes de sensores tiene el equipo. El orden es el de preferencia: NVML (y hwmon en
//! Linux), después el EC de los portátiles gaming de Acer en Windows y, como último recurso y
//! solo si nada da ventiladores, el tacómetro de la GPU por NVAPI.

use crate::{Kind, NvidiaGpus, SensorProvider};
#[cfg(windows)]
use crate::{Reading, Sensor};

/// Nombre de la fuente del EC de Acer.
#[cfg(windows)]
const ACER_EC: &str = "EC Acer";

/// Proveedores del equipo y nombres de todas las fuentes consultadas, encontraran algo o no
/// (para explicar en la UI por qué falta un sensor).
pub struct Sources {
    pub providers: Vec<Box<dyn SensorProvider>>,
    pub checked: Vec<&'static str>,
    /// Fuentes que el equipo tiene pero que solo se leen con admin (con el helper elevado).
    pub needs_admin: Vec<&'static str>,
}

impl std::fmt::Debug for Sources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sources")
            .field("providers", &self.providers.len())
            .field("checked", &self.checked)
            .field("needs_admin", &self.needs_admin)
            .finish()
    }
}

/// Abre las fuentes del equipo. Las que no están (sin GPU NVIDIA, por ejemplo) se saltan.
pub fn detect() -> Sources {
    #[cfg(windows)]
    return detect_with(framerush_platform::WindowsPlatform::new());
    #[cfg(not(windows))]
    detect_with()
}

/// Como [`detect`], pero las fuentes que piden admin se leen a través de `helper`.
#[cfg(windows)]
pub fn detect_elevated(helper: framerush_platform::Helper) -> Sources {
    detect_with(framerush_platform::WindowsPlatform::with_helper(helper))
}

fn detect_with(#[cfg(windows)] platform: framerush_platform::WindowsPlatform) -> Sources {
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
    // Las fuentes que solo se leen con admin; de momento solo las hay en Windows.
    #[cfg(not(windows))]
    let needs_admin = vec![];
    #[cfg(windows)]
    let needs_admin = {
        checked.push(ACER_EC);
        let has_gpu_temperature = has(&mut providers, Kind::GpuTemperature);
        match AcerEc::open(platform, !has_gpu_temperature) {
            Ok(Some(ec)) => {
                providers.push(Box::new(ec));
                vec![]
            }
            Err(framerush_platform::PlatformError::AccessDenied(_)) => vec![ACER_EC],
            // Sin la interfaz (no es un Acer gaming) o sin esos sensores: nada que leer.
            Ok(None) | Err(_) => vec![],
        }
    };
    let has_fans = has(&mut providers, Kind::Fan);
    #[cfg(windows)]
    if !has_fans {
        checked.push("NVAPI");
        if let Some(tach) = GpuTach::open() {
            providers.push(Box::new(tach));
        }
    }
    #[cfg(not(windows))]
    let _ = has_fans;
    Sources {
        providers,
        checked,
        needs_admin,
    }
}

/// Algún proveedor da un sensor de `kind`, con lectura o sin ella.
fn has(providers: &mut [Box<dyn SensorProvider>], kind: Kind) -> bool {
    providers
        .iter_mut()
        .flat_map(|p| p.read())
        .any(|s| s.kind == kind)
}

/// EC de los portátiles gaming de Acer: temperatura de la CPU y RPM medidas de los
/// ventiladores de CPU y GPU. La temperatura de la GPU solo entra si no la da otra fuente.
#[cfg(windows)]
struct AcerEc {
    platform: framerush_platform::WindowsPlatform,
    sensors: Vec<framerush_platform::AcerSensor>,
}

#[cfg(windows)]
impl AcerEc {
    /// `Ok(None)` si el EC no tiene ninguno de esos sensores.
    fn open(
        platform: framerush_platform::WindowsPlatform,
        with_gpu_temperature: bool,
    ) -> framerush_platform::Result<Option<Self>> {
        use framerush_platform::AcerSensor;
        let sensors: Vec<AcerSensor> = platform
            .acer_sensors()?
            .into_iter()
            .filter(|s| with_gpu_temperature || *s != AcerSensor::GpuTemperature)
            .collect();
        Ok((!sensors.is_empty()).then_some(Self { platform, sensors }))
    }
}

#[cfg(windows)]
impl SensorProvider for AcerEc {
    fn read(&mut self) -> Vec<Sensor> {
        use framerush_platform::AcerSensor;
        self.sensors
            .iter()
            .map(|&s| {
                let (id, label, kind) = match s {
                    AcerSensor::CpuTemperature => ("acer:cpu", "CPU", Kind::CpuTemperature),
                    AcerSensor::CpuFan => ("acer:cpu:fan", "CPU · ventilador", Kind::Fan),
                    AcerSensor::GpuFan => ("acer:gpu:fan", "GPU · ventilador", Kind::Fan),
                    AcerSensor::GpuTemperature => ("acer:gpu", "GPU", Kind::GpuTemperature),
                };
                let value = self.platform.acer_read(s).ok().flatten().map(u32::from);
                Sensor {
                    id: id.into(),
                    label: label.into(),
                    kind,
                    reading: value.map(|v| match kind {
                        Kind::Fan => Reading::Rpm {
                            value: v,
                            target: false,
                        },
                        _ => Reading::Celsius(v as f32),
                    }),
                }
            })
            .collect()
    }
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

    /// En Windows se consulta siempre el EC de Acer; si pide admin, se dice.
    #[test]
    fn lists_sources_that_need_admin() {
        let sources = detect();
        assert_eq!(sources.checked.contains(&"EC Acer"), cfg!(windows));
        assert!(
            sources
                .needs_admin
                .iter()
                .all(|s| sources.checked.contains(s))
        );
    }
}
