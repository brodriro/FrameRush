use crate::{ProcessControl, Result, Target};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
    /// Flags de compatibilidad que el SO guarda para ese exe (`None`: no tiene).
    fn compat_flags(&self, exe: &Path) -> Result<Option<String>>;
    /// `fullscreen-optimizations-off`: desactiva las optimizaciones de pantalla completa solo
    /// para ese exe (PLAN.md V10). Es opt-in.
    fn disable_fullscreen_optimizations(&self, exe: &Path) -> Result<()>;
    /// Borra los flags de compatibilidad del exe (revert).
    fn clear_compat_flags(&self, exe: &Path) -> Result<()>;
}

/// Ajuste de sistema (PLAN.md It5.4). Qué valores toca cada uno lo decide la plataforma; su
/// alcance y su nivel, `core`. El id es el de design/SETTINGS.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tweak {
    /// `game-dvr-off`: sin grabación en segundo plano.
    GameDvrOff,
    /// `windows-game-mode`: Modo juego activado, sin el panel de inicio de la Game Bar.
    WindowsGameMode,
    /// `notifications-off`: sin notificaciones emergentes.
    NotificationsOff,
    /// `visual-effects-performance`: efectos visuales "mejor rendimiento" y sin transparencias.
    VisualEffectsPerformance,
    /// `uwp-background-off`: las apps UWP no se ejecutan en segundo plano.
    UwpBackgroundOff,
}

impl Tweak {
    pub const ALL: [Tweak; 5] = [
        Tweak::GameDvrOff,
        Tweak::WindowsGameMode,
        Tweak::NotificationsOff,
        Tweak::VisualEffectsPerformance,
        Tweak::UwpBackgroundOff,
    ];

    /// Id de design/SETTINGS.md.
    pub fn id(self) -> &'static str {
        match self {
            Tweak::GameDvrOff => "game-dvr-off",
            Tweak::WindowsGameMode => "windows-game-mode",
            Tweak::NotificationsOff => "notifications-off",
            Tweak::VisualEffectsPerformance => "visual-effects-performance",
            Tweak::UwpBackgroundOff => "uwp-background-off",
        }
    }
}

/// Valor que había antes de aplicar un ajuste. `value: None` es "no existía" y restaurar lo
/// borra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedValue {
    /// Dónde vive el valor, legible por una persona (`HKCU\...\Nombre` en Windows).
    pub location: String,
    pub value: Option<u32>,
}

/// Ajustes persistentes: leer el estado previo, aplicarlo y restaurarlo.
pub trait SystemTweaks {
    /// Ajustes que existen en este SO.
    fn supported_tweaks(&self) -> Vec<Tweak>;
    /// Estado actual de todos los valores que toca `tweak` (para el journal).
    fn read_tweak(&self, tweak: Tweak) -> Result<Vec<SavedValue>>;
    fn apply_tweak(&self, tweak: Tweak) -> Result<()>;
    /// Deja los valores como en `saved` (lo que devolvió `read_tweak` antes de aplicar).
    fn restore_tweak(&self, tweak: Tweak, saved: &[SavedValue]) -> Result<()>;
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
    ProcessControl + ServiceControl + PowerControl + GameTuning + SystemTweaks + Capabilities
{
}

impl<T> Platform for T where
    T: ProcessControl + ServiceControl + PowerControl + GameTuning + SystemTweaks + Capabilities
{
}
