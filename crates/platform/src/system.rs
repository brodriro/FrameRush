use crate::{ProcessControl, Result, Target};
use serde::{Deserialize, Serialize};

/// Estado de un servicio del sistema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopped,
    Paused,
    /// Arrancando, parándose u otro estado transitorio.
    Pending,
}

/// Control de servicios (SCM en Windows, systemd en Linux, launchd en macOS).
///
/// Los servicios se detienen solo durante la sesión: nunca se cambia su tipo de inicio.
pub trait ServiceControl {
    /// Estado del servicio, o `None` si no existe en este equipo (R8: las reglas se filtran
    /// contra lo que hay).
    fn service_state(&self, name: &str) -> Result<Option<ServiceState>>;
    fn stop_service(&self, name: &str) -> Result<()>;
    fn start_service(&self, name: &str) -> Result<()>;
}

/// Un plan o perfil de energía del equipo.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PowerPlan {
    /// Identificador del SO (GUID del plan en Windows, nombre del perfil en Linux).
    pub id: String,
    /// Nombre que muestra el SO, en el idioma del usuario. No se traduce ni se fija en código.
    pub name: String,
}

/// Lectura y cambio del modo de energía (R5).
pub trait PowerControl {
    /// Planes que existen en este equipo.
    fn power_plans(&self) -> Result<Vec<PowerPlan>>;
    fn active_power_plan(&self) -> Result<PowerPlan>;
    fn set_active_power_plan(&self, id: &str) -> Result<()>;
}

/// Ajustes sobre el proceso del juego.
pub trait GameTuning {
    /// CPU lógicas en las que puede correr el proceso (índices desde 0).
    fn affinity(&self, target: &Target) -> Result<Vec<usize>>;
    /// Fija la afinidad. Es opt-in en todos los niveles y se desactiva con anticheat (P2).
    fn set_affinity(&self, target: &Target, cpus: &[usize]) -> Result<()>;
}

/// Qué admite esta plataforma en este equipo. La UI oculta lo que no está (PLAN.md P15).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapabilitySet {
    pub set_priority: bool,
    pub suspend: bool,
    pub kill: bool,
    pub services: bool,
    pub power_plans: bool,
    pub affinity: bool,
}

/// Informa de lo que admite la plataforma.
pub trait Capabilities {
    fn capabilities(&self) -> CapabilitySet;
}

/// Todo lo que `core` necesita del SO.
pub trait Platform:
    ProcessControl + ServiceControl + PowerControl + GameTuning + Capabilities
{
}

impl<T> Platform for T where
    T: ProcessControl + ServiceControl + PowerControl + GameTuning + Capabilities
{
}
