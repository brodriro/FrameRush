//! Snapshot del plan por nivel con las reglas embebidas y un equipo de ejemplo.

use framerush_core::{Level, PlanOptions, Snapshot, defaults, plan};
use framerush_device::samples;
use framerush_platform::{ProcessInfo, ServiceState, StartTime, Target};
use std::path::PathBuf;

fn proc(pid: u32, name: &str) -> ProcessInfo {
    ProcessInfo {
        target: Target {
            pid,
            start_time: StartTime(1),
            exe_path: PathBuf::from(format!(r"C:\Apps\{name}")),
        },
        name: name.into(),
        memory_bytes: 1,
        cpu_percent: None,
        own: Some(true),
        framerush: false,
    }
}

fn snapshot() -> Snapshot {
    let names = [
        "chrome.exe",
        "OneDrive.exe",
        "EpicGamesLauncher.exe",
        "Discord.exe",
        "Teams.exe",
        "csrss.exe",
        "cs2.exe",
    ];
    let processes: Vec<ProcessInfo> = (1..).zip(names).map(|(pid, n)| proc(pid, n)).collect();
    let game: Vec<_> = processes
        .last()
        .map(|p| p.target.clone())
        .into_iter()
        .collect();
    Snapshot {
        processes,
        services: vec![
            ("WSearch".into(), ServiceState::Running),
            ("SysMain".into(), ServiceState::Running),
            ("wuauserv".into(), ServiceState::Running),
            ("Spooler".into(), ServiceState::Stopped),
        ],
        game,
        tweaks: vec![],
        admin_tweaks: vec![],
        memory: vec![],
    }
}

fn render(level: Level, elevated: bool) -> String {
    let device = samples::hybrid_laptop();
    let rules = framerush_rules::embedded().unwrap();
    let options = PlanOptions {
        elevated,
        keep_alive: defaults::keep_alive(&device).value,
    };
    plan(level, &snapshot(), &rules, &device, &options)
        .items
        .iter()
        .map(|i| {
            format!(
                "[{}] {} {:?}{}",
                if i.default_checked { "x" } else { " " },
                i.rule_id,
                i.action,
                if i.locked { " (candado)" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn normal_without_admin() {
    assert_eq!(
        render(Level::Normal, false),
        "\
[ ] chrome Suspend
[ ] teams Suspend
[ ] epic-launcher Suspend
[ ] sysmain StopService (candado)
[ ] wsearch StopService (candado)
[ ] wuauserv StopService (candado)
[ ] onedrive Suspend"
    );
}

#[test]
fn agresivo_with_admin() {
    assert_eq!(
        render(Level::Agresivo, true),
        "\
[ ] chrome Suspend
[ ] teams Suspend
[x] epic-launcher Suspend
[x] sysmain StopService
[x] wsearch StopService
[ ] wuauserv StopService
[x] onedrive Suspend"
    );
}

#[test]
fn extremo_with_admin() {
    assert_eq!(
        render(Level::Extremo, true),
        "\
[ ] chrome Suspend
[ ] teams Suspend
[x] epic-launcher Suspend
[x] sysmain StopService
[x] wsearch StopService
[x] wuauserv StopService
[x] onedrive Suspend"
    );
}
