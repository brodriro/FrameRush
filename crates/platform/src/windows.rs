//! Implementación de Windows (M1): prioridad, suspender/reanudar, cerrar, afinidad y proceso en
//! primer plano.
//!
//! Ajustes persistentes de HKCU (`SystemTweaks`, módulo `registry`).
//!
//! Servicios y planes de energía llegan en M5 y M2; hasta entonces devuelven `Unsupported` y
//! `capabilities()` lo refleja.
//!
//! Antes de cada operación se revalida el `Target` (PID, arranque y ruta) con `sysinfo`, la
//! misma fuente del scan, para no tocar un PID reutilizado. Queda una ventana mínima entre la
//! comprobación y la llamada al SO.
#![allow(unsafe_code)]

mod registry;

use crate::{
    Capabilities, CapabilitySet, GameTuning, PlatformError, PowerControl, PowerPlan, Priority,
    ProcessControl, ProcessInfo, Result, SavedValue, ServiceControl, ServiceState, StartTime,
    SystemTweaks, Target, Tweak, scan_processes,
};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE};
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE, REALTIME_PRIORITY_CLASS, SetPriorityClass,
    SetProcessAffinityMask, TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

// API no documentada de ntdll (PLAN.md §6, V17). `raw-dylib` evita necesitar `ntdll.lib`.
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

/// Plataforma de Windows.
#[derive(Debug, Default)]
pub struct WindowsPlatform;

impl WindowsPlatform {
    pub fn new() -> Self {
        Self
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

    /// Nunca se congela ni se cierra el proceso que tiene que reanudar al resto (V17).
    fn refuse_self(target: &Target) -> Result<()> {
        if target.pid == std::process::id() {
            Err(PlatformError::AccessDenied(
                "FrameRush no actúa sobre su propio proceso".into(),
            ))
        } else {
            Ok(())
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
        let h = self.open(target, PROCESS_SET_INFORMATION)?;
        // SAFETY: `h` es un handle abierto con PROCESS_SET_INFORMATION.
        if unsafe { SetPriorityClass(handle(&h), to_class(priority)) } == 0 {
            return Err(last_error(target));
        }
        Ok(())
    }

    fn suspend(&self, target: &Target) -> Result<()> {
        Self::refuse_self(target)?;
        let h = self.open(target, PROCESS_SUSPEND_RESUME)?;
        // SAFETY: `h` es un handle abierto con PROCESS_SUSPEND_RESUME.
        nt_status(unsafe { NtSuspendProcess(handle(&h)) })
    }

    fn resume(&self, target: &Target) -> Result<()> {
        let h = self.open(target, PROCESS_SUSPEND_RESUME)?;
        // SAFETY: `h` es un handle abierto con PROCESS_SUSPEND_RESUME.
        nt_status(unsafe { NtResumeProcess(handle(&h)) })
    }

    fn kill(&self, target: &Target) -> Result<()> {
        Self::refuse_self(target)?;
        let h = self.open(target, PROCESS_TERMINATE)?;
        // SAFETY: `h` es un handle abierto con PROCESS_TERMINATE.
        if unsafe { TerminateProcess(handle(&h), 1) } == 0 {
            return Err(last_error(target));
        }
        Ok(())
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

impl GameTuning for WindowsPlatform {
    /// Solo el grupo de procesadores del proceso (hasta 64 CPU lógicas).
    fn affinity(&self, target: &Target) -> Result<Vec<usize>> {
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

    fn set_affinity(&self, target: &Target, cpus: &[usize]) -> Result<()> {
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
        registry::read_string(COMPAT_LAYERS, &exe_value_name(exe)?)
    }

    fn disable_fullscreen_optimizations(&self, exe: &Path) -> Result<()> {
        registry::write_string(COMPAT_LAYERS, &exe_value_name(exe)?, FSO_OFF_FLAG)
    }

    fn clear_compat_flags(&self, exe: &Path) -> Result<()> {
        registry::delete_value(COMPAT_LAYERS, &exe_value_name(exe)?)
    }
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

impl ServiceControl for WindowsPlatform {
    fn service_state(&self, _name: &str) -> Result<Option<ServiceState>> {
        Err(PlatformError::Unsupported("servicios (M5)"))
    }

    fn stop_service(&self, _name: &str) -> Result<()> {
        Err(PlatformError::Unsupported("servicios (M5)"))
    }

    fn start_service(&self, _name: &str) -> Result<()> {
        Err(PlatformError::Unsupported("servicios (M5)"))
    }
}

impl PowerControl for WindowsPlatform {
    fn power_plans(&self) -> Result<Vec<PowerPlan>> {
        Err(PlatformError::Unsupported("planes de energía (M2)"))
    }

    fn active_power_plan(&self) -> Result<PowerPlan> {
        Err(PlatformError::Unsupported("planes de energía (M2)"))
    }

    fn set_active_power_plan(&self, _id: &str) -> Result<()> {
        Err(PlatformError::Unsupported("planes de energía (M2)"))
    }
}

/// Un valor DWORD de HKCU que pone un ajuste.
#[derive(Debug)]
struct RegValue {
    path: &'static str,
    name: &'static str,
    value: u32,
}

impl RegValue {
    fn location(&self) -> String {
        format!(r"HKCU\{}\{}", self.path, self.name)
    }
}

const GAME_CONFIG_STORE: &str = r"System\GameConfigStore";
const GAME_DVR: &str = r"Software\Microsoft\Windows\CurrentVersion\GameDVR";
const GAME_BAR: &str = r"Software\Microsoft\GameBar";

/// Valores de cada ajuste, de la referencia CS2-FPS-Boost (`$RegNormal`, probada en hardware
/// real). De su bloque de Modo juego no se toma `UseNexusForGameBarEnabled`: cambia lo que hace
/// el botón Xbox del mando y SETTINGS.md no lo incluye.
fn reg_values(tweak: Tweak) -> &'static [RegValue] {
    match tweak {
        Tweak::GameDvrOff => &[
            RegValue {
                path: GAME_CONFIG_STORE,
                name: "GameDVR_Enabled",
                value: 0,
            },
            RegValue {
                path: GAME_DVR,
                name: "AppCaptureEnabled",
                value: 0,
            },
        ],
        Tweak::WindowsGameMode => &[
            RegValue {
                path: GAME_BAR,
                name: "AutoGameModeEnabled",
                value: 1,
            },
            RegValue {
                path: GAME_BAR,
                name: "AllowAutoGameMode",
                value: 1,
            },
            RegValue {
                path: GAME_BAR,
                name: "ShowStartupPanel",
                value: 0,
            },
        ],
        Tweak::NotificationsOff => &[RegValue {
            path: r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings",
            name: "NOC_GLOBAL_SETTING_TOAST_ENABLED",
            value: 0,
        }],
        Tweak::VisualEffectsPerformance => &[
            RegValue {
                path: r"Software\Microsoft\Windows\CurrentVersion\Explorer\VisualEffects",
                name: "VisualFXSetting",
                value: 2,
            },
            RegValue {
                path: r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                name: "EnableTransparency",
                value: 0,
            },
        ],
        Tweak::UwpBackgroundOff => &[RegValue {
            path: r"Software\Microsoft\Windows\CurrentVersion\BackgroundAccessApplications",
            name: "GlobalUserDisabled",
            value: 1,
        }],
    }
}

impl SystemTweaks for WindowsPlatform {
    fn supported_tweaks(&self) -> Vec<Tweak> {
        Tweak::ALL.to_vec()
    }

    fn read_tweak(&self, tweak: Tweak) -> Result<Vec<SavedValue>> {
        reg_values(tweak)
            .iter()
            .map(|v| {
                Ok(SavedValue {
                    location: v.location(),
                    value: registry::read_dword(v.path, v.name)?,
                })
            })
            .collect()
    }

    fn apply_tweak(&self, tweak: Tweak) -> Result<()> {
        reg_values(tweak)
            .iter()
            .try_for_each(|v| registry::write_dword(v.path, v.name, v.value))
    }

    /// Restaura cada valor de la tabla con lo guardado; si no existía, lo borra.
    fn restore_tweak(&self, tweak: Tweak, saved: &[SavedValue]) -> Result<()> {
        for v in reg_values(tweak) {
            let location = v.location();
            let Some(s) = saved.iter().find(|s| s.location == location) else {
                return Err(PlatformError::Os(format!(
                    "el journal no tiene el valor previo de {location}"
                )));
            };
            match s.value {
                Some(x) => registry::write_dword(v.path, v.name, x)?,
                None => registry::delete_value(v.path, v.name)?,
            }
        }
        Ok(())
    }
}

impl Capabilities for WindowsPlatform {
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            set_priority: true,
            suspend: true,
            kill: true,
            services: false,
            power_plans: false,
            affinity: true,
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
        assert_eq!(w.priority(&v.target).unwrap(), Priority::Normal);
        w.set_priority(&v.target, Priority::BelowNormal).unwrap();
        assert_eq!(w.priority(&v.target).unwrap(), Priority::BelowNormal);
        w.set_priority(&v.target, Priority::Normal).unwrap();
        assert_eq!(w.priority(&v.target).unwrap(), Priority::Normal);
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
        let mut stale = v.target.clone();
        stale.start_time.0 += 1;
        assert_eq!(
            w.set_priority(&stale, Priority::Idle),
            Err(PlatformError::TargetChanged { pid: stale.pid })
        );
        assert_eq!(w.priority(&v.target).unwrap(), Priority::Normal);
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

    /// Solo lectura: no cambia los valores reales del usuario.
    #[test]
    fn read_tweak_reports_every_value_it_touches() {
        let w = WindowsPlatform::new();
        for t in w.supported_tweaks() {
            let saved = w.read_tweak(t).unwrap();
            let expected: Vec<String> = reg_values(t).iter().map(RegValue::location).collect();
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
        let nowhere = Path::new(
            r"C:\FrameRush
o-existe\game.exe",
        );
        assert_eq!(w.compat_flags(nowhere).unwrap(), None);
    }

    /// Falla en el primer valor, antes de escribir nada.
    #[test]
    fn restore_without_a_backup_is_refused() {
        let w = WindowsPlatform::new();
        assert!(w.restore_tweak(Tweak::GameDvrOff, &[]).is_err());
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
