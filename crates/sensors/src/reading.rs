//! Lecturas de sensores.

use std::fmt;

/// Qué mide un sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    CpuTemperature,
    GpuTemperature,
    Fan,
}

/// Valor leído. Nunca hay un 0 de relleno: lo que no se puede leer es `None` en [`Sensor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    Celsius(f32),
    /// RPM. `target` = son las RPM objetivo que pide el driver, no las medidas (V13).
    Rpm {
        value: u32,
        target: bool,
    },
    /// Velocidad del ventilador en % de la máxima, cuando la fuente no da RPM.
    Percent(u32),
}

impl fmt::Display for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reading::Celsius(c) => write!(f, "{c:.0} °C"),
            Reading::Rpm {
                value,
                target: true,
            } => write!(f, "{value} RPM (objetivo)"),
            Reading::Rpm { value, .. } => write!(f, "{value} RPM"),
            Reading::Percent(p) => write!(f, "{p} %"),
        }
    }
}

/// Un sensor con su lectura actual.
#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    /// Estable entre lecturas (p. ej. `nvml:0:fan:1`).
    pub id: String,
    /// Nombre para mostrar (p. ej. el modelo de la GPU).
    pub label: String,
    pub kind: Kind,
    /// `None` = "No disponible".
    pub reading: Option<Reading>,
}

impl fmt::Display for Sensor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reading {
            Some(r) => write!(f, "{}: {r}", self.label),
            None => write!(f, "{}: No disponible", self.label),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_readings_say_no_disponible_not_zero() {
        let s = Sensor {
            id: "x".into(),
            label: "Ventilador".into(),
            kind: Kind::Fan,
            reading: None,
        };
        assert_eq!(s.to_string(), "Ventilador: No disponible");
        let rpm = Reading::Rpm {
            value: 1800,
            target: true,
        };
        assert_eq!(rpm.to_string(), "1800 RPM (objetivo)");
        assert_eq!(Reading::Celsius(61.4).to_string(), "61 °C");
    }
}
