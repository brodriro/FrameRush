//! `MockPlatform`: plataforma en memoria para los tests de `core` (feature `mock`).
//!
//! Imita las reglas que debe cumplir una plataforma real: revalida el [`Target`] antes de
//! cada operación, un proceso cerrado desaparece y un servicio que no existe es `None`.
//! Los valores de los tests los pone cada test; aquí no hay datos de ningún equipo.

use crate::{
    Battery, Capabilities, CapabilitySet, Cpu, Display, DisplayControl, GameTuning, Gpu, Inventory,
    MemoryControl, PlatformError, PowerControl, PowerPlan, Priority, ProcessControl, ProcessInfo,
    Result, SavedData, SavedValue, ServiceControl, ServiceState, StartTime, SystemTweaks, Target,
    Tweak,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone)]
struct MockProcess {
    info: ProcessInfo,
    priority: Priority,
    suspended: bool,
    affinity: Vec<usize>,
    /// Veces que se le ha recortado el working set.
    trims: u32,
}

#[derive(Debug, Default)]
struct State {
    processes: Vec<MockProcess>,
    services: BTreeMap<String, ServiceState>,
    plans: Vec<PowerPlan>,
    active_plan: Option<String>,
    /// Planes creados con `create_custom_power_plan` (para darles ids distintos).
    created_plans: u32,
    displays: Vec<Display>,
    cpu: Option<Cpu>,
    gpus: Vec<Gpu>,
    battery: Option<Battery>,
    os_build: Option<String>,
    manufacturer: Option<String>,
    laptop: Option<bool>,
    total_memory: Option<u64>,
    vbs_running: Option<bool>,
    installed_apps: Vec<String>,
    hags_supported: Option<bool>,
    hypervisor_features: Vec<String>,
    antivirus: Vec<String>,
    logical_cpus: usize,
    caps: CapabilitySet,
    next_start: u64,
    /// Veces que se ha purgado la lista standby.
    standby_purges: u32,
    foreground: Option<Target>,
    /// Valores de los ajustes persistentes, por ubicación.
    values: BTreeMap<String, u32>,
    /// Flags de compatibilidad por exe.
    compat: BTreeMap<PathBuf, String>,
    /// Preferencia de GPU por exe.
    gpu: BTreeMap<PathBuf, String>,
}

/// Plataforma en memoria.
#[derive(Debug)]
pub struct MockPlatform {
    state: Mutex<State>,
}

impl MockPlatform {
    /// Plataforma vacía que admite todo, con `logical_cpus` CPU lógicas.
    pub fn new(logical_cpus: usize) -> Self {
        let caps = CapabilitySet {
            set_priority: true,
            suspend: true,
            kill: true,
            services: true,
            power_plans: true,
            custom_power_plan: true,
            refresh_rate: true,
            affinity: true,
            memory: true,
        };
        Self {
            state: Mutex::new(State {
                logical_cpus,
                caps,
                next_start: 1,
                ..State::default()
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Un test que hace panic con el lock tomado ya ha fallado; se sigue con el estado.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Añade un proceso con prioridad Normal y todas las CPU, y devuelve su [`Target`].
    pub fn spawn(&self, pid: u32, name: &str, exe_path: &str, memory_bytes: u64) -> Target {
        let mut s = self.lock();
        let target = Target {
            pid,
            start_time: StartTime(s.next_start),
            exe_path: PathBuf::from(exe_path),
        };
        s.next_start += 1;
        let affinity = (0..s.logical_cpus).collect();
        s.processes.retain(|p| p.info.target.pid != pid);
        s.processes.push(MockProcess {
            info: ProcessInfo {
                target: target.clone(),
                name: name.to_owned(),
                memory_bytes,
                cpu_percent: None,
                own: Some(true),
                framerush: false,
            },
            priority: Priority::Normal,
            suspended: false,
            affinity,
            trims: 0,
        });
        target
    }

    /// Añade un servicio con su estado.
    pub fn add_service(&self, name: &str, state: ServiceState) {
        self.lock().services.insert(name.to_owned(), state);
    }

    /// Define los planes de energía y el activo.
    pub fn set_power_plans(&self, plans: Vec<PowerPlan>, active_id: &str) {
        let mut s = self.lock();
        s.plans = plans;
        s.active_plan = Some(active_id.to_owned());
    }

    /// Define el hardware que devuelve [`Inventory`].
    pub fn set_inventory(&self, cpu: Option<Cpu>, gpus: Vec<Gpu>, battery: Option<Battery>) {
        let mut s = self.lock();
        s.cpu = cpu;
        s.gpus = gpus;
        s.battery = battery;
    }

    /// Define lo que devuelve [`Inventory`] del sistema: build, fabricante, VBS y apps.
    pub fn set_system(
        &self,
        os_build: Option<&str>,
        manufacturer: Option<&str>,
        vbs_running: Option<bool>,
        installed_apps: &[&str],
    ) {
        let mut s = self.lock();
        s.os_build = os_build.map(str::to_owned);
        s.manufacturer = manufacturer.map(str::to_owned);
        s.vbs_running = vbs_running;
        s.installed_apps = installed_apps.iter().map(|a| (*a).to_owned()).collect();
    }

    /// Define la memoria física total ([`Inventory::total_memory`]).
    pub fn set_total_memory(&self, bytes: Option<u64>) {
        self.lock().total_memory = bytes;
    }

    /// Define si el equipo es un portátil ([`Inventory::laptop`]).
    pub fn set_laptop(&self, laptop: Option<bool>) {
        self.lock().laptop = laptop;
    }

    /// Define el soporte de HAGS y los componentes del hipervisor de [`Inventory`].
    pub fn set_gpu_and_hypervisor(&self, hags_supported: Option<bool>, features: &[&str]) {
        let mut s = self.lock();
        s.hags_supported = hags_supported;
        s.hypervisor_features = features.iter().map(|f| (*f).to_owned()).collect();
    }

    /// Define los antivirus registrados de [`Inventory`].
    pub fn set_antivirus(&self, names: &[&str]) {
        self.lock().antivirus = names.iter().map(|n| (*n).to_owned()).collect();
    }

    /// Define los monitores.
    pub fn set_displays(&self, displays: Vec<Display>) {
        self.lock().displays = displays;
    }

    /// Cambia lo que admite la plataforma.
    pub fn set_capabilities(&self, caps: CapabilitySet) {
        self.lock().caps = caps;
    }

    /// `true` si el proceso está congelado.
    /// Valor de un ajuste en `location` (`None`: no existe).
    pub fn value(&self, location: &str) -> Option<u32> {
        self.lock().values.get(location).copied()
    }

    /// Pone flags de compatibilidad previos a un exe, como si el usuario los tuviera ya.
    pub fn set_compat_flags(&self, exe: &str, flags: &str) {
        self.lock().compat.insert(PathBuf::from(exe), flags.into());
    }

    /// Pone una preferencia de GPU previa a un exe, como si el usuario la tuviera ya.
    pub fn set_gpu_preference(&self, exe: &str, value: &str) {
        self.lock().gpu.insert(PathBuf::from(exe), value.into());
    }

    /// Pone un valor previo, como si el usuario lo tuviera ya.
    pub fn set_value(&self, location: &str, value: u32) {
        self.lock().values.insert(location.to_owned(), value);
    }

    /// Pone `target` en primer plano (`None`: ninguna ventana).
    pub fn set_foreground(&self, target: Option<Target>) {
        self.lock().foreground = target;
    }

    pub fn is_suspended(&self, target: &Target) -> Result<bool> {
        self.with_process(target, |p| Ok(p.suspended))
    }

    fn with_process<R>(
        &self,
        target: &Target,
        f: impl FnOnce(&mut MockProcess) -> Result<R>,
    ) -> Result<R> {
        let mut s = self.lock();
        let p = s
            .processes
            .iter_mut()
            .find(|p| p.info.target.pid == target.pid)
            .ok_or_else(|| PlatformError::NotFound(format!("pid {}", target.pid)))?;
        if p.info.target != *target {
            return Err(PlatformError::TargetChanged { pid: target.pid });
        }
        f(p)
    }

    fn require(&self, cap: fn(&CapabilitySet) -> bool, what: &'static str) -> Result<()> {
        if cap(&self.lock().caps) {
            Ok(())
        } else {
            Err(PlatformError::Unsupported(what))
        }
    }
}

impl ProcessControl for MockPlatform {
    fn processes(&self) -> Result<Vec<ProcessInfo>> {
        Ok(self
            .lock()
            .processes
            .iter()
            .map(|p| p.info.clone())
            .collect())
    }

    fn is_same_process(&self, target: &Target) -> Result<bool> {
        match self.with_process(target, |_| Ok(())) {
            Ok(()) => Ok(true),
            Err(PlatformError::NotFound(_) | PlatformError::TargetChanged { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn priority(&self, target: &Target) -> Result<Priority> {
        self.with_process(target, |p| Ok(p.priority))
    }

    fn set_priority(&self, target: &Target, priority: Priority) -> Result<()> {
        self.require(|c| c.set_priority, "prioridad")?;
        self.with_process(target, |p| {
            p.priority = priority;
            Ok(())
        })
    }

    fn suspend(&self, target: &Target) -> Result<()> {
        self.require(|c| c.suspend, "suspender")?;
        self.with_process(target, |p| {
            p.suspended = true;
            Ok(())
        })
    }

    fn resume(&self, target: &Target) -> Result<()> {
        self.require(|c| c.suspend, "reanudar")?;
        self.with_process(target, |p| {
            p.suspended = false;
            Ok(())
        })
    }

    fn kill(&self, target: &Target) -> Result<()> {
        self.require(|c| c.kill, "cerrar")?;
        self.with_process(target, |_| Ok(()))?;
        self.lock().processes.retain(|p| p.info.target != *target);
        Ok(())
    }

    /// Un proceso cerrado deja de estar en primer plano.
    fn foreground(&self) -> Result<Option<Target>> {
        let s = self.lock();
        Ok(s.foreground
            .clone()
            .filter(|t| s.processes.iter().any(|p| p.info.target == *t)))
    }
}

impl ServiceControl for MockPlatform {
    fn service_state(&self, name: &str) -> Result<Option<ServiceState>> {
        Ok(self.lock().services.get(name).copied())
    }

    fn stop_service(&self, name: &str) -> Result<()> {
        self.set_service(name, ServiceState::Stopped)
    }

    fn start_service(&self, name: &str) -> Result<()> {
        self.set_service(name, ServiceState::Running)
    }
}

impl MockPlatform {
    fn set_service(&self, name: &str, state: ServiceState) -> Result<()> {
        self.require(|c| c.services, "servicios")?;
        let mut s = self.lock();
        let slot = s
            .services
            .get_mut(name)
            .ok_or_else(|| PlatformError::NotFound(format!("servicio {name}")))?;
        *slot = state;
        Ok(())
    }
}

impl PowerControl for MockPlatform {
    fn power_plans(&self) -> Result<Vec<PowerPlan>> {
        Ok(self.lock().plans.clone())
    }

    fn active_power_plan(&self) -> Result<PowerPlan> {
        let s = self.lock();
        let id = s
            .active_plan
            .as_deref()
            .ok_or_else(|| PlatformError::NotFound("plan activo".into()))?;
        s.plans
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| PlatformError::NotFound(format!("plan {id}")))
    }

    fn set_active_power_plan(&self, id: &str) -> Result<()> {
        self.require(|c| c.power_plans, "planes de energía")?;
        let mut s = self.lock();
        if !s.plans.iter().any(|p| p.id == id) {
            return Err(PlatformError::NotFound(format!("plan {id}")));
        }
        s.active_plan = Some(id.to_owned());
        Ok(())
    }

    fn create_custom_power_plan(&self, base_id: &str, name: &str) -> Result<PowerPlan> {
        self.require(|c| c.custom_power_plan, "plan de energía propio")?;
        let mut s = self.lock();
        if !s.plans.iter().any(|p| p.id == base_id) {
            return Err(PlatformError::NotFound(format!("plan {base_id}")));
        }
        s.created_plans += 1;
        let plan = PowerPlan {
            id: format!("custom-{}", s.created_plans),
            name: name.to_owned(),
        };
        s.plans.push(plan.clone());
        Ok(plan)
    }

    fn delete_power_plan(&self, id: &str) -> Result<()> {
        let mut s = self.lock();
        if s.active_plan.as_deref() == Some(id) {
            return Err(PlatformError::Os(format!("el plan {id} está activo")));
        }
        let before = s.plans.len();
        s.plans.retain(|p| p.id != id);
        if s.plans.len() == before {
            return Err(PlatformError::NotFound(format!("plan {id}")));
        }
        Ok(())
    }
}

impl Inventory for MockPlatform {
    fn cpu(&self) -> Result<Option<Cpu>> {
        Ok(self.lock().cpu.clone())
    }

    fn gpus(&self) -> Result<Vec<Gpu>> {
        Ok(self.lock().gpus.clone())
    }

    fn battery(&self) -> Result<Option<Battery>> {
        Ok(self.lock().battery)
    }

    fn os_build(&self) -> Result<Option<String>> {
        Ok(self.lock().os_build.clone())
    }

    fn manufacturer(&self) -> Result<Option<String>> {
        Ok(self.lock().manufacturer.clone())
    }

    fn laptop(&self) -> Result<Option<bool>> {
        Ok(self.lock().laptop)
    }

    fn total_memory(&self) -> Result<Option<u64>> {
        Ok(self.lock().total_memory)
    }

    fn vbs_running(&self) -> Result<Option<bool>> {
        Ok(self.lock().vbs_running)
    }

    fn installed_apps(&self) -> Result<Vec<String>> {
        Ok(self.lock().installed_apps.clone())
    }

    fn hags_supported(&self) -> Result<Option<bool>> {
        Ok(self.lock().hags_supported)
    }

    fn hypervisor_features(&self) -> Result<Vec<String>> {
        Ok(self.lock().hypervisor_features.clone())
    }

    fn antivirus(&self) -> Result<Vec<String>> {
        Ok(self.lock().antivirus.clone())
    }
}

impl DisplayControl for MockPlatform {
    fn displays(&self) -> Result<Vec<Display>> {
        Ok(self.lock().displays.clone())
    }

    fn set_refresh_rate(&self, id: &str, hz: u32) -> Result<()> {
        self.require(|c| c.refresh_rate, "frecuencia de refresco")?;
        let mut s = self.lock();
        let d = s
            .displays
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| PlatformError::NotFound(format!("pantalla {id}")))?;
        let (w, h) = (d.current.width, d.current.height);
        if !d
            .modes
            .iter()
            .any(|m| m.width == w && m.height == h && m.hz == hz)
        {
            return Err(PlatformError::Os(format!(
                "{id} no admite {w}x{h} a {hz} Hz"
            )));
        }
        d.current.hz = hz;
        Ok(())
    }
}

impl GameTuning for MockPlatform {
    fn affinity(&self, target: &Target) -> Result<Vec<usize>> {
        self.with_process(target, |p| Ok(p.affinity.clone()))
    }

    fn set_affinity(&self, target: &Target, cpus: &[usize]) -> Result<()> {
        self.require(|c| c.affinity, "afinidad")?;
        let n = self.lock().logical_cpus;
        if cpus.is_empty() || cpus.iter().any(|&c| c >= n) {
            return Err(PlatformError::Os(format!(
                "afinidad inválida {cpus:?} con {n} CPU lógicas"
            )));
        }
        self.with_process(target, |p| {
            p.affinity = cpus.to_vec();
            Ok(())
        })
    }

    fn compat_flags(&self, exe: &Path) -> Result<Option<String>> {
        Ok(self.lock().compat.get(exe).cloned())
    }

    fn disable_fullscreen_optimizations(&self, exe: &Path) -> Result<()> {
        self.lock()
            .compat
            .insert(exe.to_path_buf(), MOCK_FSO_FLAG.into());
        Ok(())
    }

    fn clear_compat_flags(&self, exe: &Path) -> Result<()> {
        self.lock().compat.remove(exe);
        Ok(())
    }

    fn gpu_preference(&self, exe: &Path) -> Result<Option<String>> {
        Ok(self.lock().gpu.get(exe).cloned())
    }

    fn prefer_high_performance_gpu(&self, exe: &Path) -> Result<()> {
        self.lock()
            .gpu
            .insert(exe.to_path_buf(), MOCK_HIGH_PERFORMANCE_GPU.into());
        Ok(())
    }

    fn clear_gpu_preference(&self, exe: &Path) -> Result<()> {
        self.lock().gpu.remove(exe);
        Ok(())
    }

    fn read_persistent_priority(&self, exe: &Path) -> Result<Vec<SavedValue>> {
        let location = mock_priority_location(exe);
        let value = self.lock().values.get(&location).copied();
        Ok(vec![SavedValue {
            location,
            value: value.map(SavedData::Number),
        }])
    }

    fn set_persistent_priority(&self, exe: &Path) -> Result<()> {
        self.lock()
            .values
            .insert(mock_priority_location(exe), MOCK_HIGH_PRIORITY);
        Ok(())
    }

    fn restore_persistent_priority(&self, _exe: &Path, saved: &[SavedValue]) -> Result<()> {
        self.restore_values(saved)
    }
}

/// Valor que pone el mock como prioridad persistente.
pub const MOCK_HIGH_PRIORITY: u32 = 3;

/// Dónde guarda el mock la prioridad persistente de un exe: `mock\ifeo\<ruta>`.
pub fn mock_priority_location(exe: &Path) -> String {
    format!(r"mock\ifeo\{}", exe.display())
}

/// Preferencia que pone el mock al elegir la GPU de alto rendimiento.
pub const MOCK_HIGH_PERFORMANCE_GPU: &str = "gpu-alto";

/// Flag que pone el mock al desactivar las FSO.
pub const MOCK_FSO_FLAG: &str = "fso-off";

/// Valor que pone cada ajuste en el mock: uno por ajuste, en `mock\<id>`, a 1.
fn mock_location(tweak: Tweak) -> String {
    format!(r"mock\{}", tweak.id())
}

impl SystemTweaks for MockPlatform {
    fn supported_tweaks(&self) -> Vec<Tweak> {
        Tweak::ALL.to_vec()
    }

    /// Como en Windows: los de todo el equipo.
    fn tweak_requires_admin(&self, tweak: Tweak) -> bool {
        matches!(
            tweak,
            Tweak::GameDvrPolicyOff
                | Tweak::SchedulerForegroundBoost
                | Tweak::PowerThrottlingOff
                | Tweak::MmcssGames
                | Tweak::UwpBackgroundOff
        )
    }

    fn read_tweak(&self, tweak: Tweak) -> Result<Vec<SavedValue>> {
        let location = mock_location(tweak);
        let value = self.lock().values.get(&location).copied();
        Ok(vec![SavedValue {
            location,
            value: value.map(SavedData::Number),
        }])
    }

    fn apply_tweak(&self, tweak: Tweak) -> Result<()> {
        self.lock().values.insert(mock_location(tweak), 1);
        Ok(())
    }

    fn restore_tweak(&self, _tweak: Tweak, saved: &[SavedValue]) -> Result<()> {
        self.restore_values(saved)
    }
}

impl MockPlatform {
    fn restore_values(&self, saved: &[SavedValue]) -> Result<()> {
        let mut s = self.lock();
        for v in saved {
            match &v.value {
                Some(SavedData::Number(x)) => s.values.insert(v.location.clone(), *x),
                None => s.values.remove(&v.location),
                Some(SavedData::Text(_)) => {
                    return Err(PlatformError::Os("el mock solo guarda números".into()));
                }
            };
        }
        Ok(())
    }
}

impl MockPlatform {
    /// Veces que se ha recortado el working set de `target`.
    pub fn trims(&self, target: &Target) -> Result<u32> {
        self.with_process(target, |p| Ok(p.trims))
    }

    /// Veces que se ha purgado la lista standby.
    pub fn standby_purges(&self) -> u32 {
        self.lock().standby_purges
    }
}

impl MemoryControl for MockPlatform {
    fn trim_working_set(&self, target: &Target) -> Result<()> {
        self.require(|c| c.memory, "recortar el working set")?;
        self.with_process(target, |p| {
            p.trims += 1;
            Ok(())
        })
    }

    fn purge_standby_list(&self) -> Result<()> {
        self.require(|c| c.memory, "purgar la lista standby")?;
        self.lock().standby_purges += 1;
        Ok(())
    }
}

impl Capabilities for MockPlatform {
    fn capabilities(&self) -> CapabilitySet {
        self.lock().caps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;

    fn platform() -> MockPlatform {
        MockPlatform::new(8)
    }

    #[test]
    fn is_a_full_platform() {
        fn takes(_: &dyn Platform) {}
        takes(&platform());
    }

    #[test]
    fn suspend_and_resume_round_trip() {
        let m = platform();
        let t = m.spawn(10, "OneDrive.exe", r"C:\OneDrive\OneDrive.exe", 100);
        m.suspend(&t).unwrap();
        assert!(m.is_suspended(&t).unwrap());
        m.resume(&t).unwrap();
        assert!(!m.is_suspended(&t).unwrap());
    }

    #[test]
    fn reused_pid_is_rejected() {
        let m = platform();
        let old = m.spawn(42, "a.exe", "/a", 1);
        let new = m.spawn(42, "b.exe", "/b", 1);
        assert_ne!(old, new);
        assert!(!m.is_same_process(&old).unwrap());
        assert_eq!(
            m.set_priority(&old, Priority::Idle),
            Err(PlatformError::TargetChanged { pid: 42 })
        );
        assert_eq!(m.priority(&new).unwrap(), Priority::Normal);
    }

    #[test]
    fn foreground_ends_with_the_process() {
        let m = platform();
        let t = m.spawn(7, "game.exe", "/game.exe", 1);
        assert_eq!(m.foreground().unwrap(), None);
        m.set_foreground(Some(t.clone()));
        assert_eq!(m.foreground().unwrap(), Some(t.clone()));
        m.kill(&t).unwrap();
        assert_eq!(m.foreground().unwrap(), None);
    }

    #[test]
    fn tweaks_restore_the_saved_values() {
        let m = platform();
        let loc = r"mock\game-dvr-off";
        let before = m.read_tweak(Tweak::GameDvrOff).unwrap();
        m.apply_tweak(Tweak::GameDvrOff).unwrap();
        assert_eq!(m.value(loc), Some(1));
        m.restore_tweak(Tweak::GameDvrOff, &before).unwrap();
        assert_eq!(m.value(loc), None);

        m.set_value(loc, 5);
        let before = m.read_tweak(Tweak::GameDvrOff).unwrap();
        m.apply_tweak(Tweak::GameDvrOff).unwrap();
        m.restore_tweak(Tweak::GameDvrOff, &before).unwrap();
        assert_eq!(m.value(loc), Some(5));
    }

    #[test]
    fn fullscreen_optimizations_round_trip() {
        let m = platform();
        let exe = Path::new("/games/game.exe");
        assert_eq!(m.compat_flags(exe).unwrap(), None);
        m.disable_fullscreen_optimizations(exe).unwrap();
        assert_eq!(m.compat_flags(exe).unwrap().as_deref(), Some(MOCK_FSO_FLAG));
        m.clear_compat_flags(exe).unwrap();
        assert_eq!(m.compat_flags(exe).unwrap(), None);
    }

    #[test]
    fn gpu_preference_round_trip() {
        let m = platform();
        let exe = Path::new("/games/game.exe");
        assert_eq!(m.gpu_preference(exe).unwrap(), None);
        m.prefer_high_performance_gpu(exe).unwrap();
        assert_eq!(
            m.gpu_preference(exe).unwrap().as_deref(),
            Some(MOCK_HIGH_PERFORMANCE_GPU)
        );
        m.clear_gpu_preference(exe).unwrap();
        assert_eq!(m.gpu_preference(exe).unwrap(), None);
    }

    #[test]
    fn kill_removes_the_process() {
        let m = platform();
        let t = m.spawn(7, "x.exe", "/x", 1);
        m.kill(&t).unwrap();
        assert!(!m.is_same_process(&t).unwrap());
        assert!(matches!(m.resume(&t), Err(PlatformError::NotFound(_))));
    }

    #[test]
    fn missing_service_is_none() {
        let m = platform();
        m.add_service("WSearch", ServiceState::Running);
        assert_eq!(m.service_state("SysMain").unwrap(), None);
        m.stop_service("WSearch").unwrap();
        assert_eq!(
            m.service_state("WSearch").unwrap(),
            Some(ServiceState::Stopped)
        );
        assert!(matches!(
            m.stop_service("SysMain"),
            Err(PlatformError::NotFound(_))
        ));
    }

    #[test]
    fn power_plan_must_exist() {
        let m = platform();
        let plan = |id: &str| PowerPlan {
            id: id.into(),
            name: format!("Plan {id}"),
        };
        m.set_power_plans(vec![plan("a"), plan("b")], "a");
        m.set_active_power_plan("b").unwrap();
        assert_eq!(m.active_power_plan().unwrap().id, "b");
        assert!(m.set_active_power_plan("z").is_err());
    }

    #[test]
    fn refresh_rate_must_be_a_mode_of_the_current_resolution() {
        let m = platform();
        let mode = |width, height, hz| crate::Mode { width, height, hz };
        m.set_displays(vec![Display {
            id: "d1".into(),
            current: mode(1920, 1080, 60),
            modes: vec![
                mode(1920, 1080, 60),
                mode(1920, 1080, 144),
                mode(1280, 720, 240),
            ],
        }]);
        m.set_refresh_rate("d1", 144).unwrap();
        assert_eq!(m.displays().unwrap()[0].current.hz, 144);
        assert!(m.set_refresh_rate("d1", 240).is_err());
        assert!(m.set_refresh_rate("d2", 60).is_err());
    }

    #[test]
    fn custom_plan_is_created_and_deleted_but_not_while_active() {
        let m = platform();
        let base = PowerPlan {
            id: "a".into(),
            name: "Plan a".into(),
        };
        m.set_power_plans(vec![base], "a");
        assert!(m.create_custom_power_plan("z", "Max").is_err());
        let custom = m.create_custom_power_plan("a", "Max").unwrap();
        assert_eq!(custom.name, "Max");
        m.set_active_power_plan(&custom.id).unwrap();
        assert!(m.delete_power_plan(&custom.id).is_err());
        m.set_active_power_plan("a").unwrap();
        m.delete_power_plan(&custom.id).unwrap();
        assert_eq!(m.power_plans().unwrap().len(), 1);
    }

    #[test]
    fn affinity_is_validated_against_the_cpu_count() {
        let m = MockPlatform::new(4);
        let t = m.spawn(1, "game", "/game", 1);
        assert_eq!(m.affinity(&t).unwrap(), vec![0, 1, 2, 3]);
        m.set_affinity(&t, &[2, 3]).unwrap();
        assert_eq!(m.affinity(&t).unwrap(), vec![2, 3]);
        assert!(m.set_affinity(&t, &[4]).is_err());
        assert!(m.set_affinity(&t, &[]).is_err());
    }

    #[test]
    fn unsupported_capabilities_fail() {
        let m = platform();
        let t = m.spawn(1, "a", "/a", 1);
        m.set_capabilities(CapabilitySet::default());
        assert_eq!(m.suspend(&t), Err(PlatformError::Unsupported("suspender")));
        assert!(!m.capabilities().kill);
    }
}
