//! Sensores del controlador embebido (EC) de los portátiles gaming de Acer por WMI: clase
//! `AcerGamingFunction` de `root\WMI`, método `GetGamingSysInfo` (gmInput `uint32` → gmOutput
//! `uint64`). Llamarlo pide admin.
//!
//! El protocolo es el del driver `acer-wmi` de Linux (`drivers/platform/x86/acer-wmi.c`,
//! interfaz «predator v4»): con la orden `0x0000` los bits 24–39 de la salida son el mapa de
//! sensores admitidos (bit `id - 1` por sensor) y con `0x0001 | id << 8` los bits 0–7 son el
//! estado (0 = bien) y los bits 8–23 la lectura. Comprobado en un Acer Nitro AN515-54 el
//! 2026-11-22. Solo se envían órdenes de lectura: [`Query`] no puede expresar otra cosa.

use crate::{PlatformError, Result};
use serde::{Deserialize, Serialize};

const NAMESPACE: &str = r"root\WMI";
const CLASS: &str = "AcerGamingFunction";
const METHOD: &str = "GetGamingSysInfo";

/// Sensores del EC que FrameRush lee (ids de `acer-wmi.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AcerSensor {
    /// Temperatura de la CPU en °C.
    CpuTemperature,
    /// RPM medidas del ventilador de la CPU.
    CpuFan,
    /// RPM medidas del ventilador de la GPU.
    GpuFan,
    /// Temperatura de la GPU en °C.
    GpuTemperature,
}

impl AcerSensor {
    pub const ALL: [AcerSensor; 4] = [
        AcerSensor::CpuTemperature,
        AcerSensor::CpuFan,
        AcerSensor::GpuFan,
        AcerSensor::GpuTemperature,
    ];

    fn id(self) -> u32 {
        match self {
            AcerSensor::CpuTemperature => 0x01,
            AcerSensor::CpuFan => 0x02,
            AcerSensor::GpuFan => 0x06,
            AcerSensor::GpuTemperature => 0x0A,
        }
    }
}

/// Órdenes de solo lectura de `GetGamingSysInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Query {
    Supported,
    Read(AcerSensor),
}

impl Query {
    fn input(self) -> u32 {
        match self {
            Query::Supported => 0x0000,
            Query::Read(sensor) => 0x0001 | sensor.id() << 8,
        }
    }
}

/// Salida cruda de `query`. `AccessDenied` sin elevación; `Os` si el equipo no tiene la clase.
pub(super) fn query(query: Query) -> Result<u64> {
    let out = super::wmi::call_u32(
        NAMESPACE,
        CLASS,
        METHOD,
        ("gmInput", query.input()),
        "gmOutput",
    )?
    .ok_or_else(|| PlatformError::Os(format!("{METHOD} no devolvió gmOutput")))?;
    out.trim()
        .parse()
        .map_err(|_| PlatformError::Os(format!("{METHOD}: salida no numérica {out:?}")))
}

/// Sensores que el EC dice tener, según la salida de [`Query::Supported`].
pub(super) fn supported(output: u64) -> Vec<AcerSensor> {
    let map = (output >> 24) & 0xFFFF;
    AcerSensor::ALL
        .into_iter()
        .filter(|s| map & (1 << (s.id() - 1)) != 0)
        .collect()
}

/// Lectura de un sensor según la salida de [`Query::Read`]; `None` si el estado no es 0.
pub(super) fn reading(output: u64) -> Option<u16> {
    (output & 0xFF == 0).then_some(((output >> 8) & 0xFFFF) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_follow_acer_wmi() {
        assert_eq!(Query::Supported.input(), 0x0000);
        assert_eq!(Query::Read(AcerSensor::CpuFan).input(), 0x0201);
        assert_eq!(Query::Read(AcerSensor::GpuFan).input(), 0x0601);
        assert_eq!(Query::Read(AcerSensor::GpuTemperature).input(), 0x0A01);
    }

    /// Salidas reales del AN515-54: mapa 0x0227 y el ventilador de la GPU a 3797 RPM.
    #[test]
    fn decodes_real_outputs() {
        assert_eq!(supported(0x0227 << 24), AcerSensor::ALL.to_vec());
        assert_eq!(
            supported(0x0003 << 24),
            vec![AcerSensor::CpuTemperature, AcerSensor::CpuFan]
        );
        assert_eq!(reading(0x0ED5 << 8), Some(3797));
        assert_eq!(reading(0x0ED5 << 8 | 3), None);
    }

    /// Necesita un portátil Acer con el EC por WMI y admin; si no, solo comprueba el error.
    #[test]
    fn reads_this_machine_or_explains_why_not() {
        match query(Query::Supported) {
            Ok(out) => {
                for s in supported(out) {
                    let value = query(Query::Read(s)).map(reading);
                    println!("{s:?}: {value:?}");
                }
            }
            Err(e) => println!("sin EC de Acer: {e}"),
        }
    }
}
