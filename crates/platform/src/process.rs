use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Instante de arranque de un proceso tal como lo da el SO.
///
/// Es opaco: solo se compara con otro valor del mismo equipo y del mismo arranque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StartTime(pub u64);

/// Identifica un proceso de forma estable (It2.4).
///
/// Los PID se reutilizan, así que cada acción guarda también el arranque y la ruta del
/// ejecutable, y se revalidan antes de tocar o revertir el proceso.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target {
    pub pid: u32,
    pub start_time: StartTime,
    pub exe_path: PathBuf,
}

/// Un proceso en ejecución, tal como lo ve el `scan`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessInfo {
    pub target: Target,
    /// Nombre del proceso según el SO.
    pub name: String,
    pub memory_bytes: u64,
    /// Uso de CPU en %, si el SO lo da ya calculado.
    pub cpu_percent: Option<f32>,
    /// Es del mismo usuario que FrameRush. `None`: el SO no deja leer su dueño (procesos de
    /// SYSTEM o de otro usuario sin admin). Todo lo que no es `Some(true)` pide admin (P12).
    pub same_user: Option<bool>,
}

/// Prioridad de CPU, de menor a mayor.
///
/// Cada plataforma la traduce a lo suyo (clases de prioridad en Windows, `nice` en Unix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    /// Solo con confirmación explícita y nunca persistida.
    Realtime,
}

/// Control de procesos.
///
/// Toda operación que recibe un [`Target`] comprueba antes que sigue siendo el mismo proceso
/// y, si no, falla con [`crate::PlatformError::TargetChanged`].
pub trait ProcessControl {
    /// Procesos en ejecución.
    fn processes(&self) -> Result<Vec<ProcessInfo>>;
    /// `true` si `target` sigue vivo y es el mismo proceso (mismo PID, arranque y ruta).
    fn is_same_process(&self, target: &Target) -> Result<bool>;
    fn priority(&self, target: &Target) -> Result<Priority>;
    fn set_priority(&self, target: &Target, priority: Priority) -> Result<()>;
    fn suspend(&self, target: &Target) -> Result<()>;
    fn resume(&self, target: &Target) -> Result<()>;
    /// Cierra el proceso. No es reversible.
    fn kill(&self, target: &Target) -> Result<()>;
    /// Proceso dueño de la ventana en primer plano, o `None` si no hay ninguna.
    fn foreground(&self) -> Result<Option<Target>>;
}
