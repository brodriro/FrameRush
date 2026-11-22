//! Sensores de temperatura y ventiladores (R4).
//!
//! Cada [`SensorProvider`] lista los sensores que conoce; una lectura que falta es `None`
//! ("No disponible") y nunca se convierte en 0. [`Poller`] los lee en un hilo aparte, por
//! defecto a 1 Hz ([`POLL_INTERVAL`]).

mod hwmon;
mod nvidia;
mod poll;
mod reading;
mod sources;

pub use hwmon::Hwmon;
pub use nvidia::{NvidiaGpu, NvidiaGpus};
pub use poll::{POLL_INTERVAL, Poller};
pub use reading::{Kind, Reading, Sensor};
pub use sources::{Sources, detect};

/// Una fuente de sensores (NVML, hwmon, LibreHardwareMonitor…).
pub trait SensorProvider: Send {
    /// Sensores con su lectura actual. Un sensor que existe pero no se puede leer va con
    /// `reading: None`.
    fn read(&mut self) -> Vec<Sensor>;
}

/// Error al abrir una fuente de sensores.
#[derive(Debug, thiserror::Error)]
pub enum SensorError {
    /// La biblioteca o el driver de la fuente no está en el equipo.
    #[error("{source_name} no está disponible: {reason}")]
    Unavailable {
        source_name: &'static str,
        reason: String,
    },
}
