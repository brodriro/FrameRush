//! Implementación de Windows (M1): prioridad, suspender/reanudar, cerrar, afinidad y proceso en
//! primer plano.
//!
//! Ajustes de sistema en el registro (`SystemTweaks`, módulo `registry`). Los que piden admin
//! pasan por el helper elevado si hay uno (módulo `helper`).
//!
//! Planes de energía con `powrprof` (módulo `power`). Servicios con el SCM (módulo `service`):
//! detener y arrancar piden admin y, sin él, pasan al helper si hay uno. Exclusiones de
//! Defender con WMI (módulo `defender`), también por el helper.
//!
//! Antes de cada operación se revalida el `Target` (PID, arranque y ruta) con `sysinfo`, la
//! misma fuente del scan, para no tocar un PID reutilizado. Queda una ventana mínima entre la
//! comprobación y la llamada al SO.
#![allow(unsafe_code)]

mod acer;
mod bcd;
mod cpus;
mod defender;
mod display;
mod effects;
mod helper;
mod integrity;
mod inventory;
mod nvapi;
mod power;
mod registry;
mod service;
mod wmi;

pub use acer::AcerSensor;
pub(crate) use acer::Query as AcerQuery;
pub use helper::{Helper, serve_helper};
pub(crate) use integrity::integrity_level;
pub use nvapi::Nvapi;

use registry::Hive;

use crate::{
    Battery, Capabilities, CapabilitySet, Cpu, DefenderExclusion, Display, DisplayControl,
    GameFiles, GameTuning, Gpu, Inventory, MemoryControl, PlatformError, PowerControl, PowerPlan,
    Priority, ProcessControl, ProcessInfo, Result, SavedData, SavedValue, SecurityTweaks,
    ServiceControl, ServiceState, StartTime, SystemTweaks, Target, Tweak, scan_processes,
};
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, LUID, STATUS_PRIVILEGE_NOT_HELD,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_PROF_SINGLE_PROCESS_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetPriorityClass,
    GetProcessAffinityMask, GetProcessDefaultCpuSets, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS,
    NORMAL_PRIORITY_CLASS, OpenProcess, OpenProcessToken, OpenThread, PROCESS_ACCESS_RIGHTS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SET_LIMITED_INFORMATION,
    PROCESS_SET_QUOTA, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE, REALTIME_PRIORITY_CLASS,
    ResumeThread, SetPriorityClass, SetProcessAffinityMask, SetProcessDefaultCpuSets,
    SuspendThread, THREAD_SUSPEND_RESUME, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

// API no documentada de ntdll (PLAN.md §6, V4 y V17). `raw-dylib` evita necesitar `ntdll.lib`.
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
    fn NtSetSystemInformation(class: i32, info: *mut c_void, len: u32) -> i32;
}

/// `SystemMemoryListInformation` (0x50) y su orden `MemoryPurgeStandbyList` (4), PLAN.md V4.
const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 0x50;
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

/// Plataforma de Windows.
#[derive(Debug, Default)]
pub struct WindowsPlatform {
    /// Helper elevado para los ajustes que piden admin. Sin él se intentan en este proceso,
    /// que falla con `AccessDenied` si no está elevado.
    helper: Option<Helper>,
    /// PID del helper: como la propia app, nunca se congela ni se cierra (V17).
    helper_pid: Option<u32>,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plataforma que delega en `helper` los ajustes que piden admin.
    pub fn with_helper(helper: Helper) -> Self {
        Self {
            helper_pid: helper.pid(),
            helper: Some(helper),
        }
    }

    /// Helper que debe aplicar `tweak`, si lo pide admin y hay uno.
    fn helper_for(&self, tweak: Tweak) -> Option<&Helper> {
        self.helper
            .as_ref()
            .filter(|_| self.tweak_requires_admin(tweak))
    }

    /// Comprueba que el PID sigue siendo el proceso del `Target`.
    fn revalidate(&self, target: &Target) -> Result<()> {
        if self.is_same_process(target)? {
            Ok(())
        } else {
            Err(PlatformError::TargetChanged { pid: target.pid })
        }
    }

    /// Revalida y abre el proceso con los permisos pedidos.
    fn open(&self, target: &Target, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle> {
        self.revalidate(target)?;
        // SAFETY: OpenProcess no toma punteros; devuelve un handle nuevo o null.
        let raw = unsafe { OpenProcess(access, 0, target.pid) };
        if raw.is_null() {
            return Err(last_error(target));
        }
        // SAFETY: `raw` es un handle válido que nos pertenece; OwnedHandle lo cierra al soltarlo.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    /// Nunca se congela ni se cierra el proceso que tiene que reanudar al resto, ni el helper
    /// (V17).
    fn refuse_own(&self, target: &Target) -> Result<()> {
        if target.pid == std::process::id() || Some(target.pid) == self.helper_pid {
            Err(PlatformError::AccessDenied(
                "FrameRush no actúa sobre sus propios procesos".into(),
            ))
        } else {
            Ok(())
        }
    }
}

/// Sensores del EC de los portátiles gaming de Acer (ver `acer.rs`). Piden admin: sin elevar
/// pasan por el helper si hay uno y, si no, devuelven `AccessDenied`.
impl WindowsPlatform {
    /// Sensores que el EC dice tener. `Os` si el equipo no tiene esa interfaz.
    pub fn acer_sensors(&self) -> Result<Vec<AcerSensor>> {
        self.acer_query(AcerQuery::Supported).map(acer::supported)
    }

    /// Lectura de `sensor` (°C o RPM medidas); `None` si el EC no la da.
    pub fn acer_read(&self, sensor: AcerSensor) -> Result<Option<u16>> {
        self.acer_query(AcerQuery::Read(sensor)).map(acer::reading)
    }

    fn acer_query(&self, query: AcerQuery) -> Result<u64> {
        match (acer::query(query), &self.helper) {
            (Err(PlatformError::AccessDenied(_)), Some(helper)) => helper.acer(query),
            (result, _) => result,
        }
    }
}

/// Acciones de proceso en este proceso, sin elevar. Las de procesos de otro usuario o de SYSTEM
/// fallan con `AccessDenied` y pasan al helper si hay uno (P12).
impl WindowsPlatform {
    /// Sin el privilegio de aumentar la prioridad base, Windows convierte Tiempo real en Alta.
    fn set_priority_here(&self, target: &Target, priority: Priority) -> Result<()> {
        let h = self.open(target, PROCESS_SET_INFORMATION)?;
        // SAFETY: `h` es un handle abierto con PROCESS_SET_INFORMATION.
        if unsafe { SetPriorityClass(handle(&h), to_class(priority)) } == 0 {
            return Err(last_error(target));
        }
        Ok(())
    }

    /// `NtSuspendProcess` y, si falla, hilo a hilo (V17).
    fn suspend_here(&self, target: &Target) -> Result<()> {
        let h = self.open(target, PROCESS_SUSPEND_RESUME)?;
        // SAFETY: `h` es un handle abierto con PROCESS_SUSPEND_RESUME.
        if nt_status(unsafe { NtSuspendProcess(handle(&h)) }).is_ok() {
            return Ok(());
        }
        self.revalidate(target)?;
        for_each_thread(target.pid, SuspendThread)
    }

    /// `NtResumeProcess` y, si falla, hilo a hilo.
    fn resume_here(&self, target: &Target) -> Result<()> {
        let h = self.open(target, PROCESS_SUSPEND_RESUME)?;
        // SAFETY: `h` es un handle abierto con PROCESS_SUSPEND_RESUME.
        if nt_status(unsafe { NtResumeProcess(handle(&h)) }).is_ok() {
            return Ok(());
        }
        self.revalidate(target)?;
        for_each_thread(target.pid, ResumeThread)
    }

    fn kill_here(&self, target: &Target) -> Result<()> {
        let h = self.open(target, PROCESS_TERMINATE)?;
        // SAFETY: `h` es un handle abierto con PROCESS_TERMINATE.
        if unsafe { TerminateProcess(handle(&h), 1) } == 0 {
            return Err(last_error(target));
        }
        Ok(())
    }

    /// `here` o, si el SO lo deniega y hay helper, la misma acción en el helper.
    fn or_helper(
        &self,
        here: Result<()>,
        elevated: impl FnOnce(&Helper) -> Result<()>,
    ) -> Result<()> {
        match (here, &self.helper) {
            (Err(PlatformError::AccessDenied(_)), Some(helper)) => elevated(helper),
            (result, _) => result,
        }
    }
}

fn handle(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle()
}

/// Error del último fallo de Win32.
fn last_error(target: &Target) -> PlatformError {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error().map(|c| c as u32) {
        Some(ERROR_ACCESS_DENIED) => PlatformError::AccessDenied(format!(
            "{} (pid {})",
            target.exe_path.display(),
            target.pid
        )),
        Some(ERROR_INVALID_PARAMETER) => PlatformError::NotFound(format!("pid {}", target.pid)),
        _ => PlatformError::Os(e.to_string()),
    }
}

/// Aplica `op` (`SuspendThread` o `ResumeThread`) a cada hilo de `pid`. No es atómico: un hilo
/// creado durante el recorrido queda sin tocar. Un hilo que ya terminó se salta.
fn for_each_thread(pid: u32, op: unsafe extern "system" fn(HANDLE) -> u32) -> Result<()> {
    // SAFETY: sin punteros; devuelve un handle nuevo o INVALID_HANDLE_VALUE.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(PlatformError::Os(format!(
            "listar los hilos: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `raw` es un handle válido y propio.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let (mut touched, mut failed) = (0usize, 0usize);
    // SAFETY: `entry` es un THREADENTRY32 con `dwSize` puesto, vivo durante el recorrido.
    let mut more = unsafe { Thread32First(handle(&snapshot), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: OpenThread no toma punteros; devuelve un handle nuevo o null.
            let t = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !t.is_null() {
                // SAFETY: `t` es un handle propio que se cierra al soltarlo.
                let t = unsafe { OwnedHandle::from_raw_handle(t) };
                // SAFETY: `t` se abrió con THREAD_SUSPEND_RESUME.
                if unsafe { op(handle(&t)) } == u32::MAX {
                    failed += 1;
                } else {
                    touched += 1;
                }
            }
        }
        // SAFETY: como en Thread32First.
        more = unsafe { Thread32Next(handle(&snapshot), &mut entry) } != 0;
    }
    match (touched, failed) {
        (0, _) => Err(PlatformError::Os(format!(
            "ningún hilo accesible en el pid {pid}"
        ))),
        (_, 0) => Ok(()),
        (_, n) => Err(PlatformError::Os(format!(
            "{n} hilo(s) del pid {pid} sin cambiar"
        ))),
    }
}

fn nt_status(status: i32) -> Result<()> {
    if status >= 0 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!(
            "NTSTATUS {:#010x}",
            status as u32
        )))
    }
}

fn to_class(p: Priority) -> u32 {
    match p {
        Priority::Idle => IDLE_PRIORITY_CLASS,
        Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
        Priority::Realtime => REALTIME_PRIORITY_CLASS,
    }
}

fn from_class(class: u32) -> Option<Priority> {
    [
        Priority::Idle,
        Priority::BelowNormal,
        Priority::Normal,
        Priority::AboveNormal,
        Priority::High,
        Priority::Realtime,
    ]
    .into_iter()
    .find(|p| to_class(*p) == class)
}

impl ProcessControl for WindowsPlatform {
    fn processes(&self) -> Result<Vec<ProcessInfo>> {
        scan_processes()
    }

    fn is_same_process(&self, target: &Target) -> Result<bool> {
        Ok(target_of(target.pid).is_some_and(|t| t == *target))
    }

    fn priority(&self, target: &Target) -> Result<Priority> {
        let h = self.open(target, PROCESS_QUERY_LIMITED_INFORMATION)?;
        // SAFETY: `h` es un handle abierto con permiso de consulta.
        let class = unsafe { GetPriorityClass(handle(&h)) };
        if class == 0 {
            return Err(last_error(target));
        }
        from_class(class).ok_or_else(|| PlatformError::Os(format!("clase de prioridad {class:#x}")))
    }

    /// Sin el privilegio de aumentar la prioridad base, Windows convierte Tiempo real en Alta.
    fn set_priority(&self, target: &Target, priority: Priority) -> Result<()> {
        self.or_helper(self.set_priority_here(target, priority), |h| {
            h.set_priority(target, priority)
        })
    }

    /// `NtSuspendProcess` y, si falla, hilo a hilo (V17).
    fn suspend(&self, target: &Target) -> Result<()> {
        self.refuse_own(target)?;
        self.or_helper(self.suspend_here(target), |h| h.suspend(target))
    }

    fn resume(&self, target: &Target) -> Result<()> {
        self.or_helper(self.resume_here(target), |h| h.resume(target))
    }

    fn kill(&self, target: &Target) -> Result<()> {
        self.refuse_own(target)?;
        self.or_helper(self.kill_here(target), |h| h.kill(target))
    }

    fn foreground(&self) -> Result<Option<Target>> {
        // SAFETY: GetForegroundWindow no toma argumentos; puede devolver null.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return Ok(None);
        }
        let mut pid = 0u32;
        // SAFETY: `hwnd` viene de GetForegroundWindow y `pid` es una variable local viva.
        // Si la ventana ya no existe, devuelve 0 y `pid` queda en 0.
        if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
            return Ok(None);
        }
        Ok(target_of(pid))
    }
}

/// `Target` de un PID vivo, con la misma fuente que el scan.
fn target_of(pid: u32) -> Option<Target> {
    let spid = sysinfo::Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[spid]),
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    sys.process(spid).map(|p| Target {
        pid,
        start_time: StartTime(p.start_time()),
        exe_path: p.exe().map(PathBuf::from).unwrap_or_default(),
    })
}

/// CPU Sets por defecto de un proceso, como CPU globales. Sin CPU Sets el proceso puede usar
/// todas las CPU del equipo.
fn default_cpu_sets(h: &OwnedHandle, target: &Target, sets: &[cpus::CpuSet]) -> Result<Vec<usize>> {
    let mut ids = vec![0u32; sets.len()];
    let mut required = 0u32;
    // SAFETY: `ids` tiene `sets.len()` huecos y vive durante la llamada.
    let ok = unsafe {
        GetProcessDefaultCpuSets(handle(h), ids.as_mut_ptr(), ids.len() as u32, &mut required)
    };
    if ok == 0 {
        return Err(last_error(target));
    }
    ids.truncate(required as usize);
    if ids.is_empty() {
        return Ok((0..sets.len()).collect());
    }
    Ok(cpus::cpus_for(sets, &ids))
}

impl GameTuning for WindowsPlatform {
    /// Con un grupo, la máscara de afinidad; con varios, los CPU Sets por defecto (V23).
    fn affinity(&self, target: &Target) -> Result<Vec<usize>> {
        let sets = cpus::system_cpu_sets()?;
        if cpus::multi_group(&sets) {
            let h = self.open(target, PROCESS_QUERY_LIMITED_INFORMATION)?;
            return default_cpu_sets(&h, target, &sets);
        }
        let h = self.open(target, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let (mut process_mask, mut system_mask) = (0usize, 0usize);
        // SAFETY: `h` es válido y los dos punteros apuntan a variables locales vivas.
        if unsafe { GetProcessAffinityMask(handle(&h), &mut process_mask, &mut system_mask) } == 0 {
            return Err(last_error(target));
        }
        Ok((0..usize::BITS as usize)
            .filter(|i| process_mask & (1 << i) != 0)
            .collect())
    }

    /// Con varios grupos son CPU Sets: el SO los respeta salvo en los hilos que el juego fije
    /// por su cuenta. Pedir todas las CPU los quita, que es el estado por defecto.
    fn set_affinity(&self, target: &Target, cpus: &[usize]) -> Result<()> {
        if cpus.is_empty() {
            return Err(PlatformError::Os("afinidad vacía".into()));
        }
        let sets = cpus::system_cpu_sets()?;
        if cpus::multi_group(&sets) {
            let mut wanted = cpus.to_vec();
            wanted.sort_unstable();
            wanted.dedup();
            let ids = if wanted.len() == sets.len() {
                vec![]
            } else {
                cpus::ids_for(&sets, &wanted)?
            };
            let h = self.open(target, PROCESS_SET_LIMITED_INFORMATION)?;
            let ptr = if ids.is_empty() {
                std::ptr::null()
            } else {
                ids.as_ptr()
            };
            // SAFETY: `ptr` es nulo (quita los CPU Sets) o apunta a `ids.len()` ids vivos.
            if unsafe { SetProcessDefaultCpuSets(handle(&h), ptr, ids.len() as u32) } == 0 {
                return Err(last_error(target));
            }
            return Ok(());
        }
        let bits = usize::BITS as usize;
        if cpus.is_empty() || cpus.iter().any(|&c| c >= bits) {
            return Err(PlatformError::Os(format!("afinidad inválida {cpus:?}")));
        }
        let mask = cpus.iter().fold(0usize, |m, &c| m | (1 << c));
        let h = self.open(
            target,
            PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
        )?;
        // SAFETY: `h` es un handle abierto con PROCESS_SET_INFORMATION.
        if unsafe { SetProcessAffinityMask(handle(&h), mask) } == 0 {
            return Err(last_error(target));
        }
        Ok(())
    }

    fn compat_flags(&self, exe: &Path) -> Result<Option<String>> {
        registry::read_string(HKCU, COMPAT_LAYERS, &exe_value_name(exe)?)
    }

    fn disable_fullscreen_optimizations(&self, exe: &Path) -> Result<()> {
        registry::write_string(HKCU, COMPAT_LAYERS, &exe_value_name(exe)?, FSO_OFF_FLAG)
    }

    fn clear_compat_flags(&self, exe: &Path) -> Result<()> {
        registry::delete_value(HKCU, COMPAT_LAYERS, &exe_value_name(exe)?)
    }

    fn gpu_preference(&self, exe: &Path) -> Result<Option<String>> {
        registry::read_string(HKCU, GPU_PREFERENCES, &exe_value_name(exe)?)
    }

    fn prefer_high_performance_gpu(&self, exe: &Path) -> Result<()> {
        registry::write_string(
            HKCU,
            GPU_PREFERENCES,
            &exe_value_name(exe)?,
            HIGH_PERFORMANCE_GPU,
        )
    }

    fn clear_gpu_preference(&self, exe: &Path) -> Result<()> {
        registry::delete_value(HKCU, GPU_PREFERENCES, &exe_value_name(exe)?)
    }

    fn read_persistent_priority(&self, exe: &Path) -> Result<Vec<SavedValue>> {
        let key = perf_options_key(exe)?;
        Ok(vec![SavedValue {
            location: priority_location(&key),
            value: registry::read_dword(HKLM, &key, CPU_PRIORITY_CLASS)?.map(SavedData::Number),
        }])
    }

    fn set_persistent_priority(&self, exe: &Path) -> Result<()> {
        if let Some(h) = &self.helper {
            return h.set_persistent_priority(exe);
        }
        let key = perf_options_key(exe)?;
        registry::write_dword(HKLM, &key, CPU_PRIORITY_CLASS, IFEO_HIGH)
    }

    fn restore_persistent_priority(&self, exe: &Path, saved: &[SavedValue]) -> Result<()> {
        if let Some(h) = &self.helper {
            return h.restore_persistent_priority(exe, saved);
        }
        let key = perf_options_key(exe)?;
        let location = priority_location(&key);
        let Some(s) = saved.iter().find(|s| s.location == location) else {
            return Err(PlatformError::Os(format!(
                "el journal no tiene el valor previo de {location}"
            )));
        };
        match &s.value {
            Some(SavedData::Number(x)) => registry::write_dword(HKLM, &key, CPU_PRIORITY_CLASS, *x),
            Some(SavedData::Text(t)) => registry::write_string(HKLM, &key, CPU_PRIORITY_CLASS, t),
            None => registry::delete_value(HKLM, &key, CPU_PRIORITY_CLASS),
        }
    }
}

/// Preferencia de GPU por exe del usuario (lo que escribe Configuración > Pantalla > Gráficos).
/// El nombre del valor es la ruta completa del exe (PLAN.md V22).
const GPU_PREFERENCES: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
/// GPU de alto rendimiento: valor de la referencia CS2-FPS-Boost, probado en un portátil Optimus.
const HIGH_PERFORMANCE_GPU: &str = "GpuPreference=2;";

/// Opciones por imagen (IFEO): la clave es el nombre del exe, no su ruta.
const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";
const CPU_PRIORITY_CLASS: &str = "CpuPriorityClass";
/// `CpuPriorityClass = 3` es Alta (PLAN.md V9). `IoPriority` no se escribe.
const IFEO_HIGH: u32 = 3;

/// `IFEO\<exe>\PerfOptions`. Solo nombres `*.exe`, para no escribir en otra clave.
fn perf_options_key(exe: &Path) -> Result<String> {
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.to_ascii_lowercase().ends_with(".exe") && n.len() > 4)
        .ok_or_else(|| {
            PlatformError::NotFound(format!("exe del juego no válido: `{}`", exe.display()))
        })?;
    Ok(format!(r"{IFEO}\{name}\PerfOptions"))
}

fn priority_location(key: &str) -> String {
    format!(r"{}\{key}\{CPU_PRIORITY_CLASS}", HKLM.prefix())
}

/// Flags de compatibilidad por exe del usuario (PLAN.md V10).
const COMPAT_LAYERS: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";
/// Valor de la referencia CS2-FPS-Boost y de PLAN.md V10.
const FSO_OFF_FLAG: &str = "~ DISABLEDXMAXIMIZEDWINDOWEDMODE";

/// El nombre del valor es la ruta completa del exe.
fn exe_value_name(exe: &Path) -> Result<String> {
    if !exe.is_absolute() {
        return Err(PlatformError::NotFound(format!(
            "ruta del exe del juego no válida: `{}`",
            exe.display()
        )));
    }
    Ok(exe.to_string_lossy().into_owned())
}

/// Error de E/S con la ruta del archivo.
fn file_error(path: &Path, e: std::io::Error) -> PlatformError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            PlatformError::AccessDenied(path.display().to_string())
        }
        _ => PlatformError::Os(format!("{}: {e}", path.display())),
    }
}

/// Quita o pone el atributo de solo lectura (en Windows, `set_readonly` es ese atributo).
fn set_read_only(path: &Path, read_only: bool) -> Result<()> {
    let mut perms = std::fs::metadata(path)
        .map_err(|e| file_error(path, e))?
        .permissions();
    if perms.readonly() != read_only {
        #[allow(clippy::permissions_set_readonly_false)]
        // Windows: es el atributo, no un modo Unix.
        perms.set_readonly(read_only);
        std::fs::set_permissions(path, perms).map_err(|e| file_error(path, e))?;
    }
    Ok(())
}

impl GameFiles for WindowsPlatform {
    fn read_game_file(&self, path: &Path) -> Result<Option<(String, bool)>> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(file_error(path, e)),
        };
        let read_only = std::fs::metadata(path)
            .map_err(|e| file_error(path, e))?
            .permissions()
            .readonly();
        Ok(Some((content, read_only)))
    }

    fn write_game_file(&self, path: &Path, content: &str, read_only: bool) -> Result<()> {
        if path.exists() {
            set_read_only(path, false)?;
        } else if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| file_error(dir, e))?;
        }
        std::fs::write(path, content).map_err(|e| file_error(path, e))?;
        if read_only {
            set_read_only(path, true)?;
        }
        Ok(())
    }

    fn remove_game_file(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        set_read_only(path, false)?;
        std::fs::remove_file(path).map_err(|e| file_error(path, e))
    }
}

impl ServiceControl for WindowsPlatform {
    fn service_state(&self, name: &str) -> Result<Option<ServiceState>> {
        service::service_state(name)
    }

    fn stop_service(&self, name: &str) -> Result<()> {
        self.or_helper(service::stop_service(name), |h| h.stop_service(name))
    }

    fn start_service(&self, name: &str) -> Result<()> {
        self.or_helper(service::start_service(name), |h| h.start_service(name))
    }
}

impl PowerControl for WindowsPlatform {
    fn power_plans(&self) -> Result<Vec<PowerPlan>> {
        power::power_plans()
    }

    fn active_power_plan(&self) -> Result<PowerPlan> {
        power::active_power_plan()
    }

    fn set_active_power_plan(&self, id: &str) -> Result<()> {
        power::set_active_power_plan(id)
    }

    fn create_custom_power_plan(
        &self,
        base_id: &str,
        name: &str,
        cpu_idle_off: bool,
    ) -> Result<PowerPlan> {
        power::create_custom_power_plan(base_id, name, cpu_idle_off)
    }

    fn delete_power_plan(&self, id: &str) -> Result<()> {
        power::delete_power_plan(id)
    }
}

impl Inventory for WindowsPlatform {
    fn cpu(&self) -> Result<Option<Cpu>> {
        inventory::cpu()
    }

    fn gpus(&self) -> Result<Vec<Gpu>> {
        inventory::gpus()
    }

    fn battery(&self) -> Result<Option<Battery>> {
        inventory::battery()
    }

    fn os_build(&self) -> Result<Option<String>> {
        inventory::os_build()
    }

    fn manufacturer(&self) -> Result<Option<String>> {
        inventory::manufacturer()
    }

    fn laptop(&self) -> Result<Option<bool>> {
        inventory::laptop()
    }

    fn total_memory(&self) -> Result<Option<u64>> {
        let mut system = System::new();
        system.refresh_memory();
        // sysinfo da 0 si no pudo leerla.
        Ok(Some(system.total_memory()).filter(|&b| b > 0))
    }

    fn vbs_running(&self) -> Result<Option<bool>> {
        inventory::vbs_running()
    }

    fn installed_apps(&self) -> Result<Vec<String>> {
        inventory::installed_apps()
    }

    fn hags_supported(&self) -> Result<Option<bool>> {
        inventory::hags_supported()
    }

    fn hypervisor_features(&self) -> Result<Vec<String>> {
        inventory::hypervisor_features()
    }

    fn antivirus(&self) -> Result<Vec<String>> {
        inventory::antivirus()
    }

    fn defender_active(&self) -> Result<Option<bool>> {
        defender::active()
    }
}

impl SecurityTweaks for WindowsPlatform {
    fn has_defender_exclusion(&self, exclusion: &DefenderExclusion) -> Result<bool> {
        match (defender::has(exclusion), &self.helper) {
            (Err(PlatformError::AccessDenied(_)), Some(h)) => h.has_defender_exclusion(exclusion),
            (result, _) => result,
        }
    }

    fn add_defender_exclusion(&self, exclusion: &DefenderExclusion) -> Result<()> {
        self.or_helper(defender::add(exclusion), |h| {
            h.add_defender_exclusion(exclusion)
        })
    }

    fn remove_defender_exclusion(&self, exclusion: &DefenderExclusion) -> Result<()> {
        self.or_helper(defender::remove(exclusion), |h| {
            h.remove_defender_exclusion(exclusion)
        })
    }
}

impl DisplayControl for WindowsPlatform {
    fn displays(&self) -> Result<Vec<Display>> {
        display::displays()
    }

    fn set_refresh_rate(&self, id: &str, hz: u32) -> Result<()> {
        display::set_refresh_rate(id, hz)
    }
}

const HKCU: Hive = Hive::CurrentUser;
const HKLM: Hive = Hive::LocalMachine;

/// Clave de Steam del usuario. Valve no la documenta (PLAN §6, V12); se lee en local.
const STEAM: &str = r"Software\Valve\Steam";

/// Carpeta de instalación de Steam (`SteamPath`), o `None` si Steam no está instalado.
pub fn steam_root() -> Result<Option<PathBuf>> {
    Ok(registry::read_string(HKCU, STEAM, "SteamPath")?.map(PathBuf::from))
}

/// Pide a Steam que lance el juego `appid` (`steam://rungameid/<appid>`, como el
/// `Start-Process` de la referencia CS2-FPS-Boost). Vuelve en cuanto el shell entrega la URL:
/// que el juego arranque se ve después en `RunningAppID` o en sus procesos.
pub fn steam_launch(appid: u32) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, url) = (
        helper::wide("open"),
        helper::wide(&format!("steam://rungameid/{appid}")),
    );
    // SAFETY: las cadenas terminan en nulo y viven durante la llamada; los punteros opcionales
    // son nulos.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Documentado: un valor mayor que 32 es éxito; si no, el propio valor es el código de error.
    let code = code as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!(
            "abrir steam://rungameid/{appid}: ShellExecute devolvió {code}"
        )))
    }
}

/// `appid` del juego que Steam tiene en marcha (`RunningAppID`; 0 = ninguno), o `None`.
pub fn steam_running_app() -> Result<Option<u32>> {
    Ok(registry::read_dword(HKCU, STEAM, "RunningAppID")?.filter(|&id| id != 0))
}

/// Carpeta «Descargas» del usuario (`SHGetKnownFolderPath(FOLDERID_Downloads)`), o `None` si
/// Windows no la da. Es la real aunque el usuario la haya movido.
pub fn downloads_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath};
    let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: el GUID vive durante la llamada, el token nulo es el usuario actual y `raw` es un
    // puntero de salida válido.
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, 0, std::ptr::null_mut(), &mut raw) };
    let path = (hr >= 0 && !raw.is_null()).then(|| {
        // SAFETY: si la llamada tiene éxito, `raw` es una cadena UTF-16 terminada en nulo.
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        // SAFETY: `raw` tiene `len` unidades válidas antes del nulo.
        let units = unsafe { std::slice::from_raw_parts(raw, len) };
        PathBuf::from(std::ffi::OsString::from_wide(units))
    });
    // SAFETY: la documentación pide liberar `raw` con CoTaskMemFree haya éxito o no; acepta nulo.
    unsafe { CoTaskMemFree(raw as *const core::ffi::c_void) };
    path
}

/// Dato que pone un ajuste en un valor.
#[derive(Debug)]
enum RegData {
    Dword(u32),
    Sz(&'static str),
}

use RegData::{Dword, Sz};

/// Un valor del registro que pone un ajuste.
#[derive(Debug)]
struct RegValue {
    hive: Hive,
    path: &'static str,
    name: &'static str,
    value: RegData,
}

impl RegValue {
    /// Lo que hay ahora, leído con el tipo que pone el ajuste.
    fn read(&self) -> Result<Option<SavedData>> {
        Ok(match self.value {
            Dword(_) => {
                registry::read_dword(self.hive, self.path, self.name)?.map(SavedData::Number)
            }
            Sz(_) => registry::read_string(self.hive, self.path, self.name)?.map(SavedData::Text),
        })
    }

    fn write(&self, data: &SavedData) -> Result<()> {
        match data {
            SavedData::Number(x) => registry::write_dword(self.hive, self.path, self.name, *x),
            SavedData::Text(s) => registry::write_string(self.hive, self.path, self.name, s),
        }
    }

    fn apply(&self) -> Result<()> {
        match self.value {
            Dword(x) => registry::write_dword(self.hive, self.path, self.name, x),
            Sz(s) => registry::write_string(self.hive, self.path, self.name, s),
        }
    }

    fn location(&self) -> String {
        format!(r"{}\{}\{}", self.hive.prefix(), self.path, self.name)
    }
}

const GAME_CONFIG_STORE: &str = r"System\GameConfigStore";
const GAME_DVR: &str = r"Software\Microsoft\Windows\CurrentVersion\GameDVR";
const GAME_BAR: &str = r"Software\Microsoft\GameBar";
/// Tarea `Games` de MMCSS (PLAN.md V11).
const MMCSS_GAMES: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks\Games";
/// VBS e integridad de memoria (HVCI): Microsoft Learn, "Enable memory integrity".
const DEVICE_GUARD: &str = r"SYSTEM\CurrentControlSet\Control\DeviceGuard";
const HVCI: &str =
    r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity";
/// Directiva de grupo "Turn on Virtualization Based Security": si existe, manda sobre
/// `DEVICE_GUARD` y cambiar este no serviría.
const DEVICE_GUARD_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows\DeviceGuard";
/// Directiva de equipo de Game DVR (referencia CS2-FPS-Boost, `$RegAggressive`).
const GAME_DVR_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows\GameDVR";

/// Valores de cada ajuste, de la referencia CS2-FPS-Boost (`$RegNormal`, probada en hardware
/// real). De su bloque de Modo juego no se toma `UseNexusForGameBarEnabled`: cambia lo que hace
/// el botón Xbox del mando y SETTINGS.md no lo incluye.
fn reg_values(tweak: Tweak) -> &'static [RegValue] {
    match tweak {
        Tweak::GameDvrOff => &[
            RegValue {
                hive: HKCU,
                path: GAME_CONFIG_STORE,
                name: "GameDVR_Enabled",
                value: Dword(0),
            },
            RegValue {
                hive: HKCU,
                path: GAME_DVR,
                name: "AppCaptureEnabled",
                value: Dword(0),
            },
        ],
        Tweak::GameDvrPolicyOff => &[RegValue {
            hive: Hive::LocalMachine,
            path: GAME_DVR_POLICY,
            name: "AllowGameDVR",
            value: Dword(0),
        }],
        Tweak::WindowsGameMode => &[
            RegValue {
                hive: HKCU,
                path: GAME_BAR,
                name: "AutoGameModeEnabled",
                value: Dword(1),
            },
            RegValue {
                hive: HKCU,
                path: GAME_BAR,
                name: "AllowAutoGameMode",
                value: Dword(1),
            },
            RegValue {
                hive: HKCU,
                path: GAME_BAR,
                name: "ShowStartupPanel",
                value: Dword(0),
            },
        ],
        Tweak::NotificationsOff => &[RegValue {
            hive: HKCU,
            path: r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings",
            name: "NOC_GLOBAL_SETTING_TOAST_ENABLED",
            value: Dword(0),
        }],
        // No va por el registro: `SystemParametersInfo` (módulo `effects`, V21).
        Tweak::VisualEffectsPerformance => &[],
        // PLAN.md V8: 0x26, quantum corto y variable con más peso para el primer plano.
        Tweak::SchedulerForegroundBoost => &[RegValue {
            hive: Hive::LocalMachine,
            path: r"SYSTEM\CurrentControlSet\Control\PriorityControl",
            name: "Win32PrioritySeparation",
            value: Dword(38),
        }],
        // Directiva "Turn off Power Throttling" (ADMX_Power Policy CSP).
        Tweak::PowerThrottlingOff => &[RegValue {
            hive: Hive::LocalMachine,
            path: r"SYSTEM\CurrentControlSet\Control\Power\PowerThrottling",
            name: "PowerThrottlingOff",
            value: Dword(1),
        }],
        // Valores de la referencia (`$RegAggressive`); los nombres son los de la documentación
        // de MMCSS.
        Tweak::MmcssGames => &[
            RegValue {
                hive: Hive::LocalMachine,
                path: MMCSS_GAMES,
                name: "GPU Priority",
                value: Dword(8),
            },
            RegValue {
                hive: Hive::LocalMachine,
                path: MMCSS_GAMES,
                name: "Priority",
                value: Dword(6),
            },
            RegValue {
                hive: Hive::LocalMachine,
                path: MMCSS_GAMES,
                name: "Scheduling Category",
                value: Sz("High"),
            },
            RegValue {
                hive: Hive::LocalMachine,
                path: MMCSS_GAMES,
                name: "SFIO Priority",
                value: Sz("High"),
            },
        ],
        // Directiva documentada "Let Windows apps run in the background" = Force deny (V21).
        // Microsoft la da para Pro, Enterprise, Education e IoT: en Home no se ofrece.
        Tweak::UwpBackgroundOff => &[RegValue {
            hive: Hive::LocalMachine,
            path: r"Software\Policies\Microsoft\Windows\AppPrivacy",
            name: "LetAppsRunInBackground",
            value: Dword(2),
        }],
        // Learn da `EnableVirtualizationBasedSecurity = 1` y HVCI `Enabled = 1` para activarlos,
        // y HVCI `Enabled = 0` para apagarlo (Troubleshooting); la referencia pone los dos a 0.
        // El hipervisor va aparte, en el arranque (`bcd`).
        Tweak::VbsHvciOff => &[
            RegValue {
                hive: Hive::LocalMachine,
                path: DEVICE_GUARD,
                name: "EnableVirtualizationBasedSecurity",
                value: Dword(0),
            },
            RegValue {
                hive: Hive::LocalMachine,
                path: HVCI,
                name: "Enabled",
                value: Dword(0),
            },
        ],
    }
}

/// `vbs-hvci-off` no se puede quitar desde el registro si está con bloqueo UEFI (`Locked = 1`;
/// Learn: hay que desactivar Secure Boot) ni si lo pone una directiva de grupo.
fn vbs_locked() -> bool {
    let locked = |path| registry::read_dword(Hive::LocalMachine, path, "Locked");
    let policy = registry::read_dword(
        Hive::LocalMachine,
        DEVICE_GUARD_POLICY,
        "EnableVirtualizationBasedSecurity",
    );
    // Si no se puede leer, no se ofrece (R8: mejor ocultarlo que fallar al reiniciar).
    !matches!(
        (locked(DEVICE_GUARD), locked(HVCI), policy),
        (Ok(None | Some(0)), Ok(None | Some(0)), Ok(None))
    )
}

impl SystemTweaks for WindowsPlatform {
    /// Todos, salvo `uwp-background-off` en las ediciones Home, donde su directiva no aplica
    /// (o si no se puede leer la edición, R8).
    fn supported_tweaks(&self) -> Vec<Tweak> {
        let home = is_home_edition().unwrap_or(true);
        Tweak::ALL
            .into_iter()
            .filter(|t| !(home && *t == Tweak::UwpBackgroundOff))
            .filter(|t| *t != Tweak::VbsHvciOff || !vbs_locked())
            .collect()
    }

    /// Pide admin todo ajuste que escribe en HKLM.
    fn tweak_requires_admin(&self, tweak: Tweak) -> bool {
        reg_values(tweak)
            .iter()
            .any(|v| v.hive == Hive::LocalMachine)
    }

    fn read_tweak(&self, tweak: Tweak) -> Result<Vec<SavedValue>> {
        if tweak == Tweak::VisualEffectsPerformance {
            return effects::read();
        }
        let mut saved = reg_values(tweak)
            .iter()
            .map(|v| {
                Ok(SavedValue {
                    location: v.location(),
                    value: v.read()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if tweak == Tweak::VbsHvciOff {
            // bcdedit pide admin hasta para leer: sin elevar, lo lee el helper.
            match (bcd::read(), &self.helper) {
                (Err(PlatformError::AccessDenied(_)), Some(h)) => return h.read_tweak(tweak),
                (result, _) => saved.push(SavedValue {
                    location: bcd::LOCATION.into(),
                    value: result?.map(SavedData::Text),
                }),
            }
        }
        Ok(saved)
    }

    fn apply_tweak(&self, tweak: Tweak) -> Result<()> {
        if let Some(h) = self.helper_for(tweak) {
            return h.apply_tweak(tweak);
        }
        if tweak == Tweak::VisualEffectsPerformance {
            return effects::apply();
        }
        reg_values(tweak).iter().try_for_each(RegValue::apply)?;
        if tweak == Tweak::VbsHvciOff {
            bcd::set("Off")?;
        }
        Ok(())
    }

    /// Restaura cada valor de la tabla con lo guardado; si no existía, lo borra.
    fn restore_tweak(&self, tweak: Tweak, saved: &[SavedValue]) -> Result<()> {
        if let Some(h) = self.helper_for(tweak) {
            return h.restore_tweak(tweak, saved);
        }
        if tweak == Tweak::VisualEffectsPerformance {
            return effects::restore(saved);
        }
        for v in reg_values(tweak) {
            let location = v.location();
            let Some(s) = saved.iter().find(|s| s.location == location) else {
                return Err(PlatformError::Os(format!(
                    "el journal no tiene el valor previo de {location}"
                )));
            };
            match &s.value {
                Some(data) => v.write(data)?,
                None => registry::delete_value(v.hive, v.path, v.name)?,
            }
        }
        if tweak == Tweak::VbsHvciOff {
            let Some(s) = saved.iter().find(|s| s.location == bcd::LOCATION) else {
                return Err(PlatformError::Os(format!(
                    "el journal no tiene el valor previo de {}",
                    bcd::LOCATION
                )));
            };
            match &s.value {
                Some(SavedData::Text(v)) => bcd::set(v)?,
                None => bcd::delete()?,
                Some(other) => {
                    return Err(PlatformError::Os(format!(
                        "{}: valor guardado no válido {other:?}",
                        bcd::LOCATION
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Windows Home (`PRODUCT_CORE*` de `GetProductInfo`), o `None` si no se puede leer.
fn is_home_edition() -> Option<bool> {
    use windows_sys::Win32::System::SystemInformation::{
        GetProductInfo, PRODUCT_CORE, PRODUCT_CORE_COUNTRYSPECIFIC, PRODUCT_CORE_N,
        PRODUCT_CORE_SINGLELANGUAGE,
    };
    let mut product = 0u32;
    // SAFETY: `product` es un local vivo que recibe el tipo de producto.
    if unsafe { GetProductInfo(10, 0, 0, 0, &mut product) } == 0 {
        return None;
    }
    Some(matches!(
        product,
        PRODUCT_CORE | PRODUCT_CORE_N | PRODUCT_CORE_COUNTRYSPECIFIC | PRODUCT_CORE_SINGLELANGUAGE
    ))
}

impl MemoryControl for WindowsPlatform {
    /// `EmptyWorkingSet`. Los procesos de otros usuarios y del sistema necesitan el helper.
    fn trim_working_set(&self, target: &Target) -> Result<()> {
        if let Some(helper) = &self.helper {
            return helper.trim_working_set(target);
        }
        let h = self.open(
            target,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA,
        )?;
        // SAFETY: `h` es un handle abierto con los permisos que pide EmptyWorkingSet.
        if unsafe { EmptyWorkingSet(handle(&h)) } == 0 {
            return Err(last_error(target));
        }
        Ok(())
    }

    /// `NtSetSystemInformation` con `SeProfileSingleProcessPrivilege`, que solo tiene un
    /// administrador (V4).
    fn purge_standby_list(&self) -> Result<()> {
        if let Some(helper) = &self.helper {
            return helper.purge_standby_list();
        }
        enable_privilege(SE_PROF_SINGLE_PROCESS_NAME)?;
        let mut command = MEMORY_PURGE_STANDBY_LIST;
        // SAFETY: `command` es un i32 vivo durante la llamada y `len` es su tamaño.
        let status = unsafe {
            NtSetSystemInformation(
                SYSTEM_MEMORY_LIST_INFORMATION,
                (&raw mut command).cast(),
                size_of::<i32>() as u32,
            )
        };
        if status == STATUS_PRIVILEGE_NOT_HELD {
            return Err(PlatformError::AccessDenied(
                "purgar la lista standby pide admin".into(),
            ));
        }
        nt_status(status)
    }
}

/// Activa un privilegio en el token de este proceso. Falla si el token no lo tiene.
fn enable_privilege(name: windows_sys::core::PCWSTR) -> Result<()> {
    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    // SAFETY: `name` es una cadena constante terminada en nulo y `luid` un local vivo.
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), name, &mut luid) } == 0 {
        return Err(PlatformError::Os(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let mut raw: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess devuelve un pseudo-handle; `raw` es un local vivo.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut raw) } == 0 {
        return Err(PlatformError::Os(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    // SAFETY: `raw` es un handle nuevo que nos pertenece; OwnedHandle lo cierra al soltarlo.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `token` tiene TOKEN_ADJUST_PRIVILEGES; sin estado previo, los punteros de salida
    // pueden ser nulos.
    let ok = unsafe {
        AdjustTokenPrivileges(
            handle(&token),
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    // AdjustTokenPrivileges devuelve éxito aunque no asigne el privilegio; lo dice GetLastError.
    // SAFETY: GetLastError no toma argumentos.
    if ok == 0 || unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(PlatformError::AccessDenied(
            "el proceso no tiene el privilegio (pide admin)".into(),
        ));
    }
    Ok(())
}

impl Capabilities for WindowsPlatform {
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            set_priority: true,
            suspend: true,
            kill: true,
            services: true,
            power_plans: true,
            custom_power_plan: true,
            refresh_rate: true,
            affinity: true,
            memory: true,
            defender_exclusions: true,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Solo actúan sobre un proceso hijo lanzado por el propio test.

    use super::*;
    use std::process::{Child, Command, Stdio};

    struct Victim {
        child: Child,
        target: Target,
    }

    impl Drop for Victim {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// `ping` a localhost durante un minuto: un proceso propio, inofensivo y siempre presente.
    fn victim() -> Victim {
        let child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("ping existe en Windows");
        let pid = child.id();
        let target = scan_processes()
            .unwrap()
            .into_iter()
            .find(|p| p.target.pid == pid)
            .expect("el hijo aparece en el scan")
            .target;
        Victim { child, target }
    }

    #[test]
    fn priority_round_trip() {
        let w = WindowsPlatform::new();
        let v = victim();
        // El hijo hereda la clase del proceso del test, que en CI no tiene por qué ser Normal.
        let original = w.priority(&v.target).unwrap();
        let other = if original == Priority::Idle {
            Priority::BelowNormal
        } else {
            Priority::Idle
        };
        w.set_priority(&v.target, other).unwrap();
        assert_eq!(w.priority(&v.target).unwrap(), other);
        w.set_priority(&v.target, original).unwrap();
        assert_eq!(w.priority(&v.target).unwrap(), original);
    }

    #[test]
    fn suspend_and_resume_a_child() {
        let w = WindowsPlatform::new();
        let v = victim();
        w.suspend(&v.target).unwrap();
        w.resume(&v.target).unwrap();
        assert!(w.is_same_process(&v.target).unwrap());
    }

    #[test]
    fn affinity_round_trip() {
        let w = WindowsPlatform::new();
        let v = victim();
        let original = w.affinity(&v.target).unwrap();
        assert!(!original.is_empty());
        w.set_affinity(&v.target, &original[..1]).unwrap();
        assert_eq!(w.affinity(&v.target).unwrap(), original[..1]);
        w.set_affinity(&v.target, &original).unwrap();
        assert!(w.set_affinity(&v.target, &[]).is_err());
    }

    #[test]
    fn kill_ends_the_child() {
        let w = WindowsPlatform::new();
        let mut v = victim();
        w.kill(&v.target).unwrap();
        v.child.wait().unwrap();
        assert!(!w.is_same_process(&v.target).unwrap());
    }

    #[test]
    fn a_changed_target_is_rejected() {
        let w = WindowsPlatform::new();
        let v = victim();
        let original = w.priority(&v.target).unwrap();
        let mut stale = v.target.clone();
        stale.start_time.0 += 1;
        assert_eq!(
            w.set_priority(&stale, Priority::Idle),
            Err(PlatformError::TargetChanged { pid: stale.pid })
        );
        assert_eq!(w.priority(&v.target).unwrap(), original);
    }

    #[test]
    fn game_files_keep_and_clear_the_read_only_attribute() {
        let w = WindowsPlatform::new();
        let dir = std::env::temp_dir().join(format!("framerush-game-files-{}", std::process::id()));
        let path = dir.join("cfg").join("video.txt");
        assert_eq!(w.read_game_file(&path).unwrap(), None);
        w.write_game_file(&path, "a", true).unwrap();
        assert_eq!(w.read_game_file(&path).unwrap(), Some(("a".into(), true)));
        // Reescribir un archivo de solo lectura funciona y puede quitarle el atributo.
        w.write_game_file(&path, "b", false).unwrap();
        assert_eq!(w.read_game_file(&path).unwrap(), Some(("b".into(), false)));
        w.write_game_file(&path, "c", true).unwrap();
        w.remove_game_file(&path).unwrap();
        assert_eq!(w.read_game_file(&path).unwrap(), None);
        w.remove_game_file(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No se puede forzar el primer plano desde un test; solo se comprueba que lo que devuelve
    /// es un proceso vivo y revalidable.
    #[test]
    fn foreground_is_a_live_process() {
        let w = WindowsPlatform::new();
        if let Some(t) = w.foreground().unwrap() {
            assert!(w.is_same_process(&t).unwrap());
        }
    }

    /// Con admin: aplica `vbs-hvci-off` y lo restaura enseguida, sin reiniciar (nada surte
    /// efecto hasta el arranque). Restaura antes de comprobar nada.
    #[test]
    #[ignore = "pide admin; toca VBS/HVCI y el arranque y los deja como estaban"]
    fn vbs_hvci_off_round_trip() {
        let w = WindowsPlatform::new();
        let before = w.read_tweak(Tweak::VbsHvciOff).unwrap();
        let applied = w.apply_tweak(Tweak::VbsHvciOff);
        let during = w.read_tweak(Tweak::VbsHvciOff);
        w.restore_tweak(Tweak::VbsHvciOff, &before).unwrap();
        assert_eq!(w.read_tweak(Tweak::VbsHvciOff).unwrap(), before);
        applied.unwrap();
        let during = during.unwrap();
        assert_eq!(during.len(), 3, "{during:?}");
        for v in &during {
            let off = match &v.value {
                Some(SavedData::Number(n)) => *n == 0,
                Some(SavedData::Text(t)) => t.eq_ignore_ascii_case("Off"),
                None => false,
            };
            assert!(off, "{v:?}");
        }
    }

    /// Solo lectura: no cambia los valores reales del usuario.
    #[test]
    fn read_tweak_reports_every_value_it_touches() {
        let w = WindowsPlatform::new();
        for t in w
            .supported_tweaks()
            .into_iter()
            .filter(|t| *t != Tweak::VisualEffectsPerformance)
        {
            let mut expected: Vec<String> = reg_values(t).iter().map(RegValue::location).collect();
            if t == Tweak::VbsHvciOff {
                // El arranque solo se lee elevado; sin helper, se deniega.
                match w.read_tweak(t) {
                    Err(PlatformError::AccessDenied(_)) => continue,
                    _ => expected.push(bcd::LOCATION.into()),
                }
            }
            let saved = w.read_tweak(t).unwrap();
            let got: Vec<String> = saved.into_iter().map(|s| s.location).collect();
            assert_eq!(got, expected, "{t:?}");
        }
    }

    /// Solo lectura; una ruta relativa se rechaza antes de tocar el registro.
    #[test]
    fn compat_flags_need_an_absolute_exe_path() {
        let w = WindowsPlatform::new();
        assert!(w.compat_flags(Path::new("game.exe")).is_err());
        assert!(w.disable_fullscreen_optimizations(Path::new("")).is_err());
        let nowhere = Path::new(r"C:\FrameRush\no-existe\game.exe");
        assert_eq!(w.compat_flags(nowhere).unwrap(), None);
        assert_eq!(w.gpu_preference(nowhere).unwrap(), None);
        assert!(w.gpu_preference(Path::new("game.exe")).is_err());
    }

    #[test]
    fn only_hklm_tweaks_require_admin() {
        let w = WindowsPlatform::new();
        let admin: Vec<Tweak> = Tweak::ALL
            .into_iter()
            .filter(|t| w.tweak_requires_admin(*t))
            .collect();
        assert_eq!(
            admin,
            [
                Tweak::GameDvrPolicyOff,
                Tweak::UwpBackgroundOff,
                Tweak::SchedulerForegroundBoost,
                Tweak::PowerThrottlingOff,
                Tweak::MmcssGames,
                Tweak::VbsHvciOff
            ]
        );
    }

    /// Solo lectura: la clave sale del nombre del exe, y lo que no es un `.exe` se rechaza.
    #[test]
    fn persistent_priority_is_keyed_by_exe_name() {
        let w = WindowsPlatform::new();
        let saved = w
            .read_persistent_priority(Path::new(r"C:\Games\FrameRush-no-existe.exe"))
            .unwrap();
        assert_eq!(saved.len(), 1);
        assert!(
            saved[0].location.ends_with(
                r"Image File Execution Options\FrameRush-no-existe.exe\PerfOptions\CpuPriorityClass"
            ),
            "{}",
            saved[0].location
        );
        assert_eq!(saved[0].value, None);
        assert!(w.read_persistent_priority(Path::new(r"C:\Games")).is_err());
        assert!(w.set_persistent_priority(Path::new(".exe")).is_err());
    }

    /// Falla en el primer valor, antes de escribir nada.
    #[test]
    fn restore_without_a_backup_is_refused() {
        let w = WindowsPlatform::new();
        assert!(w.restore_tweak(Tweak::GameDvrOff, &[]).is_err());
    }

    /// El fallback por hilos congela y reanuda el hijo sin matarlo.
    #[test]
    fn thread_fallback_suspends_and_resumes_a_child() {
        let mut v = victim();
        for_each_thread(v.target.pid, SuspendThread).unwrap();
        for_each_thread(v.target.pid, ResumeThread).unwrap();
        assert!(v.child.try_wait().unwrap().is_none(), "sigue vivo");
        assert!(for_each_thread(u32::MAX - 3, SuspendThread).is_err());
    }

    /// El helper, como la propia app, queda fuera de congelar y cerrar.
    #[test]
    fn never_suspends_the_helper() {
        let v = victim();
        let w = WindowsPlatform {
            helper: None,
            helper_pid: Some(v.target.pid),
        };
        assert!(matches!(
            w.suspend(&v.target),
            Err(PlatformError::AccessDenied(_))
        ));
        assert!(matches!(
            w.kill(&v.target),
            Err(PlatformError::AccessDenied(_))
        ));
    }

    #[test]
    fn trims_the_working_set_of_a_child() {
        let w = WindowsPlatform::new();
        let v = victim();
        w.trim_working_set(&v.target).unwrap();
    }

    #[test]
    fn never_suspends_itself() {
        let w = WindowsPlatform::new();
        let me = scan_processes()
            .unwrap()
            .into_iter()
            .find(|p| p.target.pid == std::process::id())
            .unwrap();
        assert!(matches!(
            w.suspend(&me.target),
            Err(PlatformError::AccessDenied(_))
        ));
    }
}
