//! GPU NVIDIA con NVML (`nvml-wrapper`, que carga la biblioteca del driver en tiempo de
//! ejecución). Según la documentación de NVML:
//! - `nvmlDeviceGetTemperature` (sensor de la GPU) da °C.
//! - `nvmlDeviceGetTemperatureThreshold(SLOWDOWN)` es la temperatura a la que la GPU empieza a
//!   bajar frecuencias; puede ser `NotSupported` (V13).
//! - `nvmlDeviceGetFanSpeedRPM` da las RPM **objetivo** ("intended"), no las medidas; si no se
//!   admite se usa `nvmlDeviceGetFanSpeed_v2` (% de la máxima). Los portátiles suelen no
//!   tener ventilador propio de la GPU (`NotSupported`).

use crate::{Kind, Reading, Sensor, SensorError, SensorProvider};
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

    /// Umbral de *slowdown* de cada GPU por nombre, en °C; `None` si el driver no lo da.
    pub fn slowdown_c(&self) -> Vec<(String, Option<f32>)> {
        (0..self.count)
            .filter_map(|i| self.nvml.device_by_index(i).ok())
            .map(|d| {
                let name = d.name().unwrap_or_else(|_| "GPU NVIDIA".into());
                let slowdown = d
                    .temperature_threshold(TemperatureThreshold::Slowdown)
                    .ok()
                    .map(|c| c as f32);
                (name, slowdown)
            })
            .collect()
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
                label: label.clone(),
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
                    label: format!("{label} · ventilador {}", fan + 1),
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
        assert_eq!(gpus.slowdown_c().len(), gpus.count as usize);
    }
}
