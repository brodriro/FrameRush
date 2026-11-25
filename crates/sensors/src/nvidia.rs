//! GPU NVIDIA con NVML (`nvml-wrapper`, que carga la biblioteca del driver en tiempo de
//! ejecución). Según la documentación de NVML:
//! - `nvmlDeviceGetTemperature` (sensor de la GPU) da °C.
//! - `nvmlDeviceGetTemperatureThreshold(SLOWDOWN)` es la temperatura a la que la GPU empieza a
//!   bajar frecuencias; puede ser `NotSupported` (V13).
//! - `nvmlDeviceGetFanSpeedRPM` da las RPM **objetivo** ("intended"), no las medidas; si no se
//!   admite se usa `nvmlDeviceGetFanSpeed_v2` (% de la máxima). Los portátiles suelen no
//!   tener ventilador propio de la GPU (`NotSupported`).

use crate::{Kind, Label, Reading, Sensor, SensorError, SensorProvider};
use framerush_platform::{Gpu, PciId};
use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{TemperatureSensor, TemperatureThreshold};

/// Todas las GPU NVIDIA que ve NVML.
#[derive(Debug)]
pub struct NvidiaGpus {
    nvml: Nvml,
    count: u32,
}

impl NvidiaGpus {
    /// Falla si no hay driver de NVIDIA (no hay GPU NVIDIA o NVML no está instalado).
    pub fn open() -> Result<Self, SensorError> {
        let unavailable = |e: nvml_wrapper::error::NvmlError| SensorError::Unavailable {
            source_name: "NVML",
            reason: e.to_string(),
        };
        let nvml = Nvml::init().map_err(unavailable)?;
        let count = nvml.device_count().map_err(unavailable)?;
        Ok(Self { nvml, count })
    }

    /// Cada GPU con su id PCI y su umbral de *slowdown*.
    pub fn gpus(&self) -> Vec<NvidiaGpu> {
        (0..self.count)
            .filter_map(|i| self.nvml.device_by_index(i).ok())
            .map(|d| NvidiaGpu {
                name: d.name().unwrap_or_else(|_| "GPU NVIDIA".into()),
                pci: d.pci_info().ok().and_then(|p| {
                    Some(PciId {
                        // "The combined 16-bit device ID and 16-bit vendor ID": el fabricante
                        // va en los 16 bits bajos (comprobado: 0x10de en una GTX 1650).
                        vendor: (p.pci_device_id & 0xffff) as u16,
                        device: (p.pci_device_id >> 16) as u16,
                        subsystem: p.pci_sub_system_id?,
                    })
                }),
                slowdown_c: d
                    .temperature_threshold(TemperatureThreshold::Slowdown)
                    .ok()
                    .map(|c| c as f32),
            })
            .collect()
    }

    /// Pone el umbral de *slowdown* de NVML en las GPU del perfil (`Gpu::slowdown_c`).
    pub fn fill_slowdown(&self, gpus: &mut [Gpu]) {
        fill_slowdown(gpus, &self.gpus());
    }
}

/// Una GPU según NVML.
#[derive(Debug, Clone, PartialEq)]
pub struct NvidiaGpu {
    pub name: String,
    pub pci: Option<PciId>,
    /// `None` si el driver no lo da (V13).
    pub slowdown_c: Option<f32>,
}

/// Empareja por id PCI. Si el id se repite en algún lado (dos tarjetas iguales) no se sabe cuál
/// es cuál, así que esa GPU se queda sin umbral (fallback "estimado") en lugar de adivinar.
fn fill_slowdown(gpus: &mut [Gpu], nvidia: &[NvidiaGpu]) {
    let count = |pci: PciId| {
        (
            gpus.iter().filter(|g| g.pci == Some(pci)).count(),
            nvidia.iter().filter(|n| n.pci == Some(pci)).count(),
        )
    };
    let unique: Vec<(PciId, Option<f32>)> = nvidia
        .iter()
        .filter_map(|n| Some((n.pci?, n.slowdown_c)))
        .filter(|(pci, _)| count(*pci) == (1, 1))
        .collect();
    for (pci, slowdown) in unique {
        if let Some(gpu) = gpus.iter_mut().find(|g| g.pci == Some(pci)) {
            gpu.slowdown_c = slowdown;
        }
    }
}

impl SensorProvider for NvidiaGpus {
    fn read(&mut self) -> Vec<Sensor> {
        let mut sensors = vec![];
        for i in 0..self.count {
            let device = self.nvml.device_by_index(i).ok();
            let label = device
                .as_ref()
                .and_then(|d| d.name().ok())
                .unwrap_or_else(|| format!("GPU NVIDIA {i}"));
            sensors.push(Sensor {
                id: format!("nvml:{i}:temp"),
                label: Label::Named(label.clone()),
                kind: Kind::GpuTemperature,
                reading: device
                    .as_ref()
                    .and_then(|d| d.temperature(TemperatureSensor::Gpu).ok())
                    .map(|c| Reading::Celsius(c as f32)),
            });
            // Sin número de ventiladores no se inventa ninguno.
            let fans = device.as_ref().and_then(|d| d.num_fans().ok()).unwrap_or(0);
            for fan in 0..fans {
                let reading = device.as_ref().and_then(|d| {
                    d.fan_speed_rpm(fan)
                        .map(|value| Reading::Rpm {
                            value,
                            target: true,
                        })
                        .or_else(|_| d.fan_speed(fan).map(Reading::Percent))
                        .ok()
                });
                sensors.push(Sensor {
                    id: format!("nvml:{i}:fan:{fan}"),
                    label: Label::Fan {
                        device: label.clone(),
                        number: Some(fan + 1),
                    },
                    kind: Kind::Fan,
                    reading,
                });
            }
        }
        sensors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_platform::GpuVendor;

    /// Sin GPU NVIDIA (la CI) `open` falla y no hay nada que leer; con una, cada GPU tiene su
    /// sensor de temperatura.
    #[test]
    fn reads_every_nvidia_gpu_if_there_is_one() {
        let Ok(mut gpus) = NvidiaGpus::open() else {
            return;
        };
        let sensors = gpus.read();
        let temps = sensors
            .iter()
            .filter(|s| s.kind == Kind::GpuTemperature)
            .count();
        assert_eq!(temps, gpus.count as usize);
        let nvidia = gpus.gpus();
        assert_eq!(nvidia.len(), gpus.count as usize);
        for g in nvidia {
            assert_eq!(g.pci.map(|p| p.vendor), Some(0x10de), "{g:?}");
        }
    }

    fn gpu(id: &str, pci: Option<PciId>) -> Gpu {
        Gpu {
            id: id.into(),
            name: id.into(),
            vendor: GpuVendor::Nvidia,
            dedicated_vram_bytes: None,
            slowdown_c: None,
            pci,
        }
    }

    fn nvidia(pci: PciId, slowdown_c: Option<f32>) -> NvidiaGpu {
        NvidiaGpu {
            name: "nv".into(),
            pci: Some(pci),
            slowdown_c,
        }
    }

    const A: PciId = PciId {
        vendor: 0x10de,
        device: 0x1f91,
        subsystem: 0x1,
    };
    const B: PciId = PciId {
        vendor: 0x10de,
        device: 0x2504,
        subsystem: 0x2,
    };

    #[test]
    fn pairs_by_pci_id_and_leaves_the_rest() {
        let mut gpus = [gpu("igpu", None), gpu("a", Some(A)), gpu("b", Some(B))];
        fill_slowdown(&mut gpus, &[nvidia(A, Some(97.0)), nvidia(B, None)]);
        let slowdowns: Vec<_> = gpus.iter().map(|g| g.slowdown_c).collect();
        assert_eq!(slowdowns, [None, Some(97.0), None]);
    }

    #[test]
    fn identical_cards_are_not_guessed() {
        let mut gpus = [gpu("a1", Some(A)), gpu("a2", Some(A))];
        fill_slowdown(&mut gpus, &[nvidia(A, Some(97.0)), nvidia(A, Some(90.0))]);
        assert!(gpus.iter().all(|g| g.slowdown_c.is_none()));
    }
}
