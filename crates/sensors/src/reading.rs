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

/// Nombre para mostrar de un sensor. Los nombres del hardware van tal cual; lo que hay que
/// traducir (la palabra «ventilador») lo pone quien lo muestra, en su idioma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    /// Nombre que da la fuente (p. ej. el modelo de la GPU o un canal de hwmon).
    Named(String),
    /// Ventilador sin nombre propio de `device`; `number` (desde 1) si tiene varios.
    Fan { device: String, number: Option<u32> },
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Label::Named(name) => f.write_str(name),
            Label::Fan {
                device,
                number: Some(n),
            } => write!(f, "{device} · ventilador {n}"),
            Label::Fan { device, .. } => write!(f, "{device} · ventilador"),
        }
    }
}

/// Un sensor con su lectura actual.
#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    /// Estable entre lecturas (p. ej. `nvml:0:fan:1`).
    pub id: String,
    pub label: Label,
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
            label: Label::Fan {
                device: "GPU".into(),
                number: Some(2),
            },
            kind: Kind::Fan,
            reading: None,
        };
        assert_eq!(s.to_string(), "GPU · ventilador 2: No disponible");
        let rpm = Reading::Rpm {
            value: 1800,
            target: true,
        };
        assert_eq!(rpm.to_string(), "1800 RPM (objetivo)");
        assert_eq!(Reading::Celsius(61.4).to_string(), "61 °C");
    }
}
