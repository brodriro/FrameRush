//! Lista de procesos en ejecución con `sysinfo` (portable: Windows, Linux y macOS).

use crate::{ProcessInfo, Result, StartTime, Target};
use std::collections::HashMap;
use std::path::PathBuf;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, Uid, UpdateKind};

/// Procesos en ejecución, ordenados por PID.
///
/// `start_time` es el arranque en segundos desde la época que da `sysinfo`. Si el SO no deja
/// leer la ruta del ejecutable (procesos del sistema sin admin), `exe_path` queda vacío; esos
/// procesos se revalidan solo por PID y arranque. El uso de CPU necesita dos muestras y aquí
/// queda en `None`. El dueño se compara con el de este proceso (SID en Windows, uid en Unix) y,
/// en Windows, también el nivel de integridad: un proceso elevado no es propio. `framerush`
/// marca este proceso y los que cuelgan de él.
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
    let tree: HashMap<u32, (Option<u32>, u64)> = sys
        .processes()
        .iter()
        .map(|(pid, p)| (pid.as_u32(), (p.parent().map(Pid::as_u32), p.start_time())))
        .collect();
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
            framerush: descends_from(&tree, pid.as_u32(), std::process::id()),
        })
        .collect();
    out.sort_by_key(|p| p.target.pid);
    Ok(out)
}

/// `Target` de un PID vivo, con la misma fuente que el scan; `None` si ya no existe. Es la
/// revalidación `(pid, start_time, exe_path)` antes de tocar un proceso (It2.4).
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub(crate) fn target_of(pid: u32) -> Option<Target> {
    let spid = Pid::from_u32(pid);
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

/// `pid` es `root` o desciende de él según `tree` (PID → padre y arranque). Como los PID se
/// reutilizan, un padre que arrancó después que su hijo no cuenta (el original ya terminó).
fn descends_from(tree: &HashMap<u32, (Option<u32>, u64)>, mut pid: u32, root: u32) -> bool {
    // Tope por si el SO devolviera un ciclo.
    for _ in 0..tree.len() + 1 {
        if pid == root {
            return true;
        }
        let Some(&(Some(parent), started)) = tree.get(&pid) else {
            return false;
        };
        match tree.get(&parent) {
            Some(&(_, parent_started)) if parent_started <= started => pid = parent,
            _ => return false,
        }
    }
    false
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
        assert!(found.framerush);
    }

    /// Un hijo cuenta; un proceso cuyo «padre» arrancó después (PID reutilizado) no.
    #[test]
    fn descendants_need_an_older_parent() {
        let tree = HashMap::from([
            (1, (None, 10)),
            (2, (Some(1), 11)),
            (3, (Some(2), 12)),
            (4, (Some(1), 5)),
            (5, (Some(5), 1)),
        ]);
        assert!(descends_from(&tree, 3, 1));
        assert!(descends_from(&tree, 1, 1));
        assert!(!descends_from(&tree, 4, 1));
        assert!(!descends_from(&tree, 5, 1));
        assert!(!descends_from(&tree, 9, 1));
    }

    /// Un proceso hijo de este test sale marcado como de FrameRush.
    #[test]
    fn marks_child_processes() {
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 5 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let procs = scan_processes().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let found = procs.iter().find(|p| p.target.pid == child.id()).unwrap();
        assert!(found.framerush);
        assert!(procs.iter().any(|p| !p.framerush));
    }
}
