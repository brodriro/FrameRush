//! Procesos congelados por otro programa (PLAN.md M3): p. ej. un script «Turbo» externo que
//! se cerró sin reanudarlos. FrameRush los avisa y ofrece reanudarlos.

use crate::{Change, Journal, is_protected};
use framerush_platform::ProcessInfo;

/// De `processes`, los congelados (`suspended`, de `ProcessControl::suspended`) que no congeló
/// FrameRush (no están en el journal), ni son FrameRush ni están protegidos.
pub fn foreign_frozen<'a>(
    processes: &'a [ProcessInfo],
    suspended: &[u32],
    journal: &Journal,
) -> Vec<&'a ProcessInfo> {
    let ours = |p: &ProcessInfo| {
        journal
            .entries
            .iter()
            .any(|e| matches!(&e.change, Change::Suspend { target } if *target == p.target))
    };
    processes
        .iter()
        .filter(|p| suspended.contains(&p.target.pid))
        .filter(|p| !p.framerush && !is_protected(&p.name) && !ours(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, EntryStatus};
    use framerush_platform::{StartTime, Target};

    fn proc(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            target: Target {
                pid,
                start_time: StartTime(u64::from(pid)),
                exe_path: format!("C:/x/{name}").into(),
            },
            name: name.into(),
            memory_bytes: 0,
            cpu_percent: None,
            own: Some(true),
            framerush: false,
        }
    }

    #[test]
    fn keeps_only_frozen_processes_that_framerush_did_not_freeze() {
        let mut framerush = proc(4, "framerush-gui.exe");
        framerush.framerush = true;
        let processes = vec![
            proc(1, "chrome.exe"),
            proc(2, "steam.exe"),
            proc(3, "csrss.exe"),
            framerush,
            proc(5, "discord.exe"),
        ];
        let journal = Journal {
            entries: vec![Entry {
                seq: 1,
                batch: 1,
                status: EntryStatus::Done,
                change: Change::Suspend {
                    target: processes[4].target.clone(),
                },
            }],
        };
        let found = foreign_frozen(&processes, &[1, 3, 4, 5, 99], &journal);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["chrome.exe"]);
    }

    #[test]
    fn a_reused_pid_is_not_ours() {
        let processes = vec![proc(1, "chrome.exe")];
        let mut old = processes[0].target.clone();
        old.start_time = StartTime(0);
        let journal = Journal {
            entries: vec![Entry {
                seq: 1,
                batch: 1,
                status: EntryStatus::Done,
                change: Change::Suspend { target: old },
            }],
        };
        assert_eq!(foreign_frozen(&processes, &[1], &journal).len(), 1);
    }
}
