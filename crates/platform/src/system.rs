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
    /// `custom-power-plan`: duplica `base_id` como un plan nuevo llamado `name` con los valores
    /// de CA del plan propio (procesador al 100 %, core parking off, turbo agresivo, PCIe ASPM y
    /// suspensión selectiva de USB off). Los de CC no se tocan. No lo activa.
    fn create_custom_power_plan(&self, base_id: &str, name: &str) -> Result<PowerPlan>;
    /// Borra un plan que no es el activo.
    fn delete_power_plan(&self, id: &str) -> Result<()>;
}

/// Un monitor y sus modos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    /// Identificador del SO (en Windows, el `DeviceName` del adaptador: `\\.\DISPLAY1`).
    pub id: String,
    pub current: Mode,
    /// Modos que admite, tal como los lista el SO.
    pub modes: Vec<Mode>,
}

impl Display {
    /// Máxima frecuencia admitida con la resolución actual.
    pub fn max_hz_at_current_resolution(&self) -> Option<u32> {
        self.modes
            .iter()
            .filter(|m| m.width == self.current.width && m.height == self.current.height)
            .map(|m| m.hz)
            .max()
    }
}

/// Resolución y frecuencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

/// CPU y su topología.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    pub model: String,
    /// Núcleos físicos en orden. Vacío si no se pudo leer la topología.
    pub cores: Vec<Core>,
    /// CPU lógicas que ve el SO.
    pub logical_cpus: usize,
    /// Temperatura máxima de unión (MSR/LHM/hwmon `temp*_crit`).
    pub tjmax_c: Option<f32>,
}

impl Cpu {
    /// Núcleos físicos, si se conoce la topología.
    pub fn physical_cores(&self) -> Option<usize> {
        (!self.cores.is_empty()).then_some(self.cores.len())
    }
}

/// Un núcleo físico y las CPU lógicas (hilos SMT) que lo componen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Core {
    pub kind: CoreKind,
    pub logical: Vec<usize>,
}

/// Tipo de núcleo en CPUs híbridas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreKind {
    Performance,
    Efficiency,
}

/// Una GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gpu {
    /// Identificador estable del SO (LUID/ruta de dispositivo), para `gpu-preference`.
    pub id: String,
    pub name: String,
    pub vendor: GpuVendor,
    /// VRAM dedicada tal como la da el SO (`None` si no se pudo leer). Una integrada suele
    /// dar un valor pequeño que el driver reserva.
    pub dedicated_vram_bytes: Option<u64>,
    /// Umbral de *slowdown* (NVML); puede no estar disponible (V13).
    pub slowdown_c: Option<f32>,
    /// Id PCI, para emparejar la GPU con otras fuentes (NVML). `None` si el SO no lo da.
    pub pci: Option<PciId>,
}

/// Id PCI de una GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PciId {
    pub vendor: u16,
    pub device: u16,
    pub subsystem: u32,
}

/// Fabricante de la GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

/// Estado de la batería.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub discharging: bool,
}

/// Inventario del hardware para el `DeviceProfile` (R8). `None` o lista vacía: no se pudo leer.
pub trait Inventory {
    fn cpu(&self) -> Result<Option<Cpu>>;
    fn gpus(&self) -> Result<Vec<Gpu>>;
    /// `None` si el equipo no tiene batería.
    fn battery(&self) -> Result<Option<Battery>>;
    /// Build del SO tal como lo da el sistema (p. ej. "26100" en Windows).
    fn os_build(&self) -> Result<Option<String>>;
    /// Fabricante del equipo.
    fn manufacturer(&self) -> Result<Option<String>>;
    /// Si el equipo es un portátil; `None` si el SO no lo dice.
    fn laptop(&self) -> Result<Option<bool>>;
    /// Memoria física total en bytes; `None` si el SO no la da.
    fn total_memory(&self) -> Result<Option<u64>>;
    /// Si VBS (Device Guard) está en ejecución; `None` si el SO no lo tiene o no lo expone.
    fn vbs_running(&self) -> Result<Option<bool>>;
    /// Nombres de las apps instaladas, ordenados y sin repetir.
    fn installed_apps(&self) -> Result<Vec<String>>;
    /// Si alguna GPU admite la programación de GPU acelerada por hardware (HAGS); `None` si
    /// ningún driver lo dice.
    fn hags_supported(&self) -> Result<Option<bool>>;
    /// Componentes del SO activados que dependen del hipervisor (V6), con su nombre del SO
    /// (p. ej. `VirtualMachinePlatform`).
    fn hypervisor_features(&self) -> Result<Vec<String>>;
    /// Nombres de los antivirus registrados en el SO (Security Center en Windows), ordenados.
    /// Solo el nombre: no dice cuál está activo.
    fn antivirus(&self) -> Result<Vec<String>>;
}

/// Pantallas: modos y frecuencia de refresco (`refresh-rate-max`).
pub trait DisplayControl {
    /// Monitores del escritorio con su modo actual y los modos que admiten.
    fn displays(&self) -> Result<Vec<Display>>;
    /// Cambia solo la frecuencia de `id`, manteniendo su resolución. `hz` tiene que ser uno de
    /// sus modos con la resolución actual.
    fn set_refresh_rate(&self, id: &str, hz: u32) -> Result<()>;
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
    /// Preferencia de GPU que el SO guarda para ese exe (`None`: no tiene).
    fn gpu_preference(&self, exe: &Path) -> Result<Option<String>>;
    /// `gpu-preference = alto`: el exe usa la GPU de alto rendimiento (PLAN.md V22). Solo
    /// `alto` está verificado; `ahorro` y `sistema` esperan fuente.
    fn prefer_high_performance_gpu(&self, exe: &Path) -> Result<()>;
    /// Borra la preferencia de GPU del exe (revert).
    fn clear_gpu_preference(&self, exe: &Path) -> Result<()>;
    /// Estado actual de la prioridad persistente del exe (para el journal).
    fn read_persistent_priority(&self, exe: &Path) -> Result<Vec<SavedValue>>;
    /// `game-priority-persistent`: el SO arranca siempre ese exe en Alta (P1: solo Alta). Pide
    /// admin.
    fn set_persistent_priority(&self, exe: &Path) -> Result<()>;
    /// Deja la prioridad persistente como en `saved`.
    fn restore_persistent_priority(&self, exe: &Path, saved: &[SavedValue]) -> Result<()>;
}

/// Ajuste de sistema (PLAN.md It5.4). Qué valores toca cada uno lo decide la plataforma; su
/// alcance y su nivel, `core`. El id es el de design/SETTINGS.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tweak {
    /// `game-dvr-off`: sin grabación en segundo plano.
    GameDvrOff,
    /// `game-dvr-off-policy`: la parte de directiva de `game-dvr-off` (desde Agresivo). Es de
    /// todo el equipo y pide admin.
    GameDvrPolicyOff,
    /// `windows-game-mode`: Modo juego activado, sin el panel de inicio de la Game Bar.
    WindowsGameMode,
    /// `notifications-off`: sin notificaciones emergentes.
    NotificationsOff,
    /// `visual-effects-performance`: efectos visuales "mejor rendimiento" y sin transparencias.
    VisualEffectsPerformance,
    /// `uwp-background-off`: las apps UWP no se ejecutan en segundo plano.
    UwpBackgroundOff,
    /// `scheduler-foreground-boost`: quantum corto y más tiempo de CPU para el primer plano.
    SchedulerForegroundBoost,
    /// `power-throttling-off`: Windows no limita la frecuencia de los procesos en segundo plano.
    PowerThrottlingOff,
    /// `mmcss-games`: reparto de MMCSS para la tarea `Games`. Es opt-in: su efecto es casi nulo.
    MmcssGames,
}

impl Tweak {
    pub const ALL: [Tweak; 9] = [
        Tweak::GameDvrOff,
        Tweak::GameDvrPolicyOff,
        Tweak::WindowsGameMode,
        Tweak::NotificationsOff,
        Tweak::VisualEffectsPerformance,
        Tweak::UwpBackgroundOff,
        Tweak::SchedulerForegroundBoost,
        Tweak::PowerThrottlingOff,
        Tweak::MmcssGames,
    ];

    /// Id de design/SETTINGS.md.
    pub fn id(self) -> &'static str {
        match self {
            Tweak::GameDvrOff => "game-dvr-off",
            Tweak::GameDvrPolicyOff => "game-dvr-off-policy",
            Tweak::WindowsGameMode => "windows-game-mode",
            Tweak::NotificationsOff => "notifications-off",
            Tweak::VisualEffectsPerformance => "visual-effects-performance",
            Tweak::UwpBackgroundOff => "uwp-background-off",
            Tweak::SchedulerForegroundBoost => "scheduler-foreground-boost",
            Tweak::PowerThrottlingOff => "power-throttling-off",
            Tweak::MmcssGames => "mmcss-games",
        }
    }
}

/// Dato de un valor guardado: número (DWORD) o texto (REG_SZ). En el journal es un número o
/// una cadena JSON, así que los journals con solo números siguen leyéndose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SavedData {
    Number(u32),
    Text(String),
}

/// Valor que había antes de aplicar un ajuste. `value: None` es "no existía" y restaurar lo
/// borra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedValue {
    /// Dónde vive el valor, legible por una persona (`HKCU\...\Nombre` en Windows).
    pub location: String,
    pub value: Option<SavedData>,
}

/// Ajustes persistentes: leer el estado previo, aplicarlo y restaurarlo.
pub trait SystemTweaks {
    /// Ajustes que existen en este SO.
    fn supported_tweaks(&self) -> Vec<Tweak>;
    /// Aplicarlo o restaurarlo pide elevación (lo hace el helper). Leerlo, no.
    fn tweak_requires_admin(&self, tweak: Tweak) -> bool;
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
    /// [`PowerControl::create_custom_power_plan`] (solo Windows).
    pub custom_power_plan: bool,
    /// [`DisplayControl::set_refresh_rate`].
    pub refresh_rate: bool,
    pub affinity: bool,
    /// `memory-trim` y `standby-purge` ([`MemoryControl`]).
    pub memory: bool,
}

/// Informa de lo que admite la plataforma.
pub trait Capabilities {
    fn capabilities(&self) -> CapabilitySet;
}

/// Liberar memoria antes de jugar (PLAN.md It5.2). No deja estado que revertir: el SO vuelve
/// a cargar las páginas cuando hacen falta. Las dos operaciones piden admin (P9).
pub trait MemoryControl {
    /// `memory-trim`: saca del working set del proceso todas las páginas que pueda.
    fn trim_working_set(&self, target: &Target) -> Result<()>;
    /// `standby-purge`: vacía la lista standby (caché de archivos en RAM).
    fn purge_standby_list(&self) -> Result<()>;
}

/// Todo lo que `core` necesita del SO.
pub trait Platform:
    ProcessControl
    + ServiceControl
    + PowerControl
    + DisplayControl
    + Inventory
    + GameTuning
    + SystemTweaks
    + MemoryControl
    + Capabilities
{
}

impl<T> Platform for T where
    T: ProcessControl
        + ServiceControl
        + PowerControl
        + DisplayControl
        + Inventory
        + GameTuning
        + SystemTweaks
        + MemoryControl
        + Capabilities
{
}
