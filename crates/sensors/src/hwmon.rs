//! Linux hwmon (`/sys/class/hwmon`), según el ABI del kernel (`sysfs-class-hwmon`):
//! - `hwmonX/name`: nombre del chip, el único atributo obligatorio.
//! - `tempY_input`: temperatura en miligrados Celsius; `tempY_label`, su etiqueta opcional.
//!   Leerla puede fallar (coretemp da `ENODATA` si la CPU la marca como no válida).
//! - `fanY_input`: RPM medidas; `fanY_label`, su etiqueta opcional.
//!
//! Temperaturas de CPU de `coretemp` (Intel) y `k10temp` (AMD), de GPU de `amdgpu`; las de
//! otros chips (NVMe, ACPI…) no son de CPU ni de GPU y no se muestran (R4). Los ventiladores
//! salen de cualquier chip. Las GPU NVIDIA van por NVML.

use crate::{Kind, Reading, Sensor, SensorProvider};
use std::fs;
use std::path::{Path, PathBuf};

/// Chips de temperatura que se muestran y qué miden.
const TEMPERATURE_CHIPS: [(&str, Kind); 3] = [
    ("coretemp", Kind::CpuTemperature),
    ("k10temp", Kind::CpuTemperature),
    ("amdgpu", Kind::GpuTemperature),
];

/// Todos los chips de `/sys/class/hwmon`.
#[derive(Debug)]
pub struct Hwmon {
    root: PathBuf,
}

impl Hwmon {
    pub fn new() -> Self {
        Self::at("/sys/class/hwmon")
    }

    fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Default for Hwmon {
    fn default() -> Self {
        Self::new()
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Índices `Y` de los archivos `{prefix}Y_input` del chip, ordenados.
fn channels(chip: &Path, prefix: &str) -> Vec<u32> {
    let Ok(entries) = fs::read_dir(chip) else {
        return vec![];
    };
    let mut indices: Vec<u32> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|name| {
            name.strip_prefix(prefix)?
                .strip_suffix("_input")?
                .parse()
                .ok()
        })
        .collect();
    indices.sort_unstable();
    indices
}

impl SensorProvider for Hwmon {
    fn read(&mut self) -> Vec<Sensor> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return vec![];
        };
        let mut chips: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
        chips.sort();
        let mut sensors = vec![];
        for chip in chips {
            let Some(name) = read_trimmed(&chip.join("name")) else {
                continue;
            };
            let dir = chip
                .file_name()
                .map_or_else(String::new, |d| d.to_string_lossy().into_owned());
            let label = |kind: &str, y: u32| {
                let channel = read_trimmed(&chip.join(format!("{kind}{y}_label")))
                    .unwrap_or_else(|| format!("{kind}{y}"));
                format!("{name} · {channel}")
            };
            if let Some((_, kind)) = TEMPERATURE_CHIPS.iter().find(|(n, _)| *n == name) {
                for y in channels(&chip, "temp") {
                    sensors.push(Sensor {
                        id: format!("hwmon:{dir}:temp{y}"),
                        label: label("temp", y),
                        kind: *kind,
                        reading: read_trimmed(&chip.join(format!("temp{y}_input")))
                            .and_then(|v| v.parse::<i64>().ok())
                            .map(|milli| Reading::Celsius(milli as f32 / 1000.0)),
                    });
                }
            }
            for y in channels(&chip, "fan") {
                sensors.push(Sensor {
                    id: format!("hwmon:{dir}:fan{y}"),
                    label: label("fan", y),
                    kind: Kind::Fan,
                    // 0 RPM es un ventilador parado, no "No disponible" (R4).
                    reading: read_trimmed(&chip.join(format!("fan{y}_input")))
                        .and_then(|v| v.parse::<u32>().ok())
                        .map(|value| Reading::Rpm {
                            value,
                            target: false,
                        }),
                });
            }
        }
        sensors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, value: &str) {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{value}\n")).unwrap();
    }

    #[test]
    fn reads_cpu_gpu_and_fans_from_a_sysfs_tree() {
        let root = std::env::temp_dir().join(format!("framerush-hwmon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write(&root, "hwmon0/name", "coretemp");
        write(&root, "hwmon0/temp1_input", "54000");
        write(&root, "hwmon0/temp1_label", "Package id 0");
        // Existe pero no se puede leer (ENODATA): "No disponible", no 0.
        write(&root, "hwmon0/temp2_input", "");
        write(&root, "hwmon1/name", "amdgpu");
        write(&root, "hwmon1/temp1_input", "61500");
        write(&root, "hwmon1/temp1_label", "edge");
        write(&root, "hwmon1/fan1_input", "0");
        // NVMe: ni CPU ni GPU.
        write(&root, "hwmon2/name", "nvme");
        write(&root, "hwmon2/temp1_input", "40000");
        // Sin `name` no es un chip válido.
        write(&root, "hwmon3/temp1_input", "1000");

        let sensors = Hwmon::at(&root).read();
        let got: Vec<(String, Kind, Option<Reading>)> = sensors
            .into_iter()
            .map(|s| (s.label, s.kind, s.reading))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "coretemp · Package id 0".into(),
                    Kind::CpuTemperature,
                    Some(Reading::Celsius(54.0))
                ),
                ("coretemp · temp2".into(), Kind::CpuTemperature, None),
                (
                    "amdgpu · edge".into(),
                    Kind::GpuTemperature,
                    Some(Reading::Celsius(61.5))
                ),
                (
                    "amdgpu · fan1".into(),
                    Kind::Fan,
                    Some(Reading::Rpm {
                        value: 0,
                        target: false
                    })
                ),
            ]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_root_has_no_sensors() {
        assert!(Hwmon::at("/no/existe/hwmon").read().is_empty());
    }
}
