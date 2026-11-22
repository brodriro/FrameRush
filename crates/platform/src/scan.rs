//! Lista de procesos en ejecución con `sysinfo` (portable: Windows, Linux y macOS).

use crate::{ProcessInfo, Result, StartTime, Target};
use std::path::PathBuf;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, Uid, UpdateKind};

/// Procesos en ejecución, ordenados por PID.
///
/// `start_time` es el arranque en segundos desde la época que da `sysinfo`. Si el SO no deja
/// leer la ruta del ejecutable (procesos del sistema sin admin), `exe_path` queda vacío; esos
/// procesos se revalidan solo por PID y arranque. El uso de CPU necesita dos muestras y aquí
/// queda en `None`. El dueño se compara con el de este proceso (SID en Windows, uid en Unix) y,
/// en Windows, también el nivel de integridad: un proceso elevado no es propio.
pub fn scan_processes() -> Result<Vec<ProcessInfo>> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet),
    );
    let me = sys
        .process(Pid::from_u32(std::process::id()))
        .and_then(|p| p.user_id())
        .cloned();
    let my_level = integrity_level(std::process::id());
    let mut out: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .map(|(pid, p)| ProcessInfo {
            target: Target {
                pid: pid.as_u32(),
                start_time: StartTime(p.start_time()),
                exe_path: p.exe().map(PathBuf::from).unwrap_or_default(),
            },
            name: p.name().to_string_lossy().into_owned(),
            memory_bytes: p.memory(),
            cpu_percent: None,
            own: own(me.as_ref(), p.user_id(), my_level, pid.as_u32()),
        })
        .collect();
    out.sort_by_key(|p| p.target.pid);
    Ok(out)
}

/// Mismo dueño que este proceso y sin más integridad que él (`my_level`).
fn own(me: Option<&Uid>, owner: Option<&Uid>, my_level: Option<u32>, pid: u32) -> Option<bool> {
    if !me.zip(owner).map(|(a, b)| a == b)? {
        return Some(false);
    }
    match my_level {
        Some(mine) => integrity_level(pid).map(|level| level <= mine),
        None => Some(true),
    }
}

/// Nivel de integridad de Windows; los demás SO no lo tienen.
#[cfg(windows)]
fn integrity_level(pid: u32) -> Option<u32> {
    crate::windows::integrity_level(pid)
}

#[cfg(not(windows))]
fn integrity_level(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_current_process() {
        let me = std::process::id();
        let procs = scan_processes().unwrap();
        let found = procs.iter().find(|p| p.target.pid == me).unwrap();
        assert!(!found.name.is_empty());
        assert!(found.memory_bytes > 0);
        assert_eq!(found.own, Some(true));
    }
}
