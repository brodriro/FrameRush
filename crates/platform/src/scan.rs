//! Lista de procesos en ejecución con `sysinfo` (portable: Windows, Linux y macOS).

use crate::{ProcessInfo, Result, StartTime, Target};
use std::path::PathBuf;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Procesos en ejecución, ordenados por PID.
///
/// `start_time` es el arranque en segundos desde la época que da `sysinfo`. Si el SO no deja
/// leer la ruta del ejecutable (procesos del sistema sin admin), `exe_path` queda vacío; esos
/// procesos se revalidan solo por PID y arranque. El uso de CPU necesita dos muestras y aquí
/// queda en `None`.
pub fn scan_processes() -> Result<Vec<ProcessInfo>> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );
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
        })
        .collect();
    out.sort_by_key(|p| p.target.pid);
    Ok(out)
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
    }
}
