//! `plan(level, snapshot, rules, device)`: qué se propone tocar y qué viene marcado (PLAN.md §3,
//! "Flujo principal").
//!
//! El plan no toca el sistema. Solo aparecen las reglas que coinciden con algo que existe en
//! el equipo (P15). El nivel solo decide `default_checked`, y la decisión final es del checklist.

use crate::protected::{ProtectedReason, is_protected, normalize, protected_reason};
use crate::{Action, Category, Level, Risk, Rule, Scope};
use framerush_device::DeviceProfile;
use framerush_platform::{ProcessInfo, ServiceState, Target};

/// Estado del equipo en el momento del `scan`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub processes: Vec<ProcessInfo>,
    /// Servicios que existen y su estado.
    pub services: Vec<(String, ServiceState)>,
    /// Proceso del juego detectado; nunca entra en el plan.
    pub game: Option<Target>,
}

/// Opciones que no dependen del nivel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanOptions {
    /// Hay elevación (helper disponible); sin ella, lo que pide admin sale con candado (P14).
    pub elevated: bool,
    /// Procesos que no se congelan (`keep-alive-list`). Bajarles la prioridad sigue permitido (P7).
    pub keep_alive: Vec<String>,
}

/// A qué afecta una fila.
#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    /// Instancias en ejecución de los procesos de la regla.
    Processes(Vec<ProcessInfo>),
    /// Servicios de la regla que existen y están en marcha.
    Services(Vec<String>),
}

/// Una fila del checklist.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanItem {
    pub rule_id: String,
    pub category: Category,
    pub subject: Subject,
    /// Acción efectiva en este nivel (un kill fuera de Extremo pasa a suspender).
    pub action: Action,
    pub risk: Risk,
    pub min_level: Level,
    pub description: String,
    pub requires_admin: bool,
    /// Pide admin y no hay elevación: la fila se muestra con candado.
    pub locked: bool,
    pub default_checked: bool,
}

impl PlanItem {
    /// Memoria que ocupan los procesos de la fila (para la estimación de It3.4).
    pub fn memory_bytes(&self) -> u64 {
        match &self.subject {
            Subject::Processes(ps) => ps.iter().map(|p| p.memory_bytes).sum(),
            Subject::Services(_) => 0,
        }
    }

    pub fn scope(&self) -> Scope {
        self.action.scope()
    }
}

/// Plan para un nivel.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub level: Level,
    pub items: Vec<PlanItem>,
    /// Procesos de anticheat en ejecución (It1.3), sin repetir. Ver [`running_anticheats`].
    pub anticheats: Vec<String>,
}

impl Plan {
    /// Filas que vienen marcadas.
    pub fn checked(&self) -> impl Iterator<Item = &PlanItem> {
        self.items.iter().filter(|i| i.default_checked)
    }

    /// Afinidad y timer del juego (P2): el anticheat manda y los desactiva aunque el usuario
    /// los haya pedido. Al juego solo se le toca la prioridad.
    pub fn game_tuning_allowed(&self) -> bool {
        self.anticheats.is_empty()
    }
}

/// Anticheats en ejecución: los procesos protegidos con motivo `AntiCheat`, por nombre y sin
/// repetir (PLAN.md It1.3).
///
/// Es un criterio de todo el equipo, no del juego: un anticheat que arranca con Windows
/// (Vanguard) cuenta para cualquier juego. Es la opción conservadora hasta que los adaptadores
/// de juego (M4) digan qué anticheat usa cada uno. Los anticheats sin proceso propio (Ricochet
/// es un driver que se carga con el juego, V19) no se detectan aquí.
pub fn running_anticheats(processes: &[ProcessInfo]) -> Vec<String> {
    let mut names: Vec<String> = processes
        .iter()
        .filter(|p| protected_reason(&p.name) == Some(ProtectedReason::AntiCheat))
        .map(|p| p.name.clone())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// Calcula el plan de `level` para este equipo.
pub fn plan(
    level: Level,
    snapshot: &Snapshot,
    rules: &[Rule],
    device: &DeviceProfile,
    options: &PlanOptions,
) -> Plan {
    let os = device.os;
    let keep_alive: Vec<String> = options.keep_alive.iter().map(|n| normalize(n)).collect();
    let items = rules
        .iter()
        .filter_map(|rule| {
            let subject = if rule.action == Action::StopService {
                let running: Vec<String> = snapshot
                    .services
                    .iter()
                    .filter(|(name, state)| {
                        *state == ServiceState::Running && rule.matches_service(os, name)
                    })
                    .map(|(name, _)| name.clone())
                    .collect();
                (!running.is_empty()).then_some(Subject::Services(running))?
            } else {
                let procs: Vec<ProcessInfo> = snapshot
                    .processes
                    .iter()
                    .filter(|p| rule.matches_process(os, &p.name))
                    .filter(|p| {
                        !is_protected(&p.name)
                            && !is_protected(&p.target.exe_path.to_string_lossy())
                    })
                    .filter(|p| snapshot.game.as_ref() != Some(&p.target))
                    .cloned()
                    .collect();
                (!procs.is_empty()).then_some(Subject::Processes(procs))?
            };

            let mut action = rule.action.effective(level, rule.risk);
            let frozen = matches!(action, Action::Suspend | Action::Kill);
            if frozen
                && matches!(&subject, Subject::Processes(ps)
                    if ps.iter().any(|p| keep_alive.contains(&normalize(&p.name))))
            {
                action = Action::LowerPriority;
            }

            let requires_admin = rule.requires_admin();
            let locked = requires_admin && !options.elevated;
            Some(PlanItem {
                rule_id: rule.id.clone(),
                category: rule.category,
                subject,
                action,
                risk: rule.risk,
                min_level: rule.min_level,
                description: rule.description.clone(),
                requires_admin,
                locked,
                default_checked: level.includes(rule.min_level) && !rule.optional && !locked,
            })
        })
        .collect();
    Plan {
        level,
        items,
        anticheats: running_anticheats(&snapshot.processes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OsNames;
    use framerush_device::samples;
    use framerush_platform::StartTime;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn proc(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            target: Target {
                pid,
                start_time: StartTime(pid.into()),
                exe_path: PathBuf::from(format!(r"C:\Apps\{name}")),
            },
            name: name.into(),
            memory_bytes: 10,
            cpu_percent: None,
        }
    }

    fn rule(id: &str, names: &[&str], action: Action, min_level: Level, risk: Risk) -> Rule {
        Rule {
            id: id.into(),
            matches: OsNames {
                windows: names.iter().map(|n| (*n).into()).collect(),
                ..OsNames::default()
            },
            category: if action == Action::StopService {
                Category::Service
            } else {
                Category::Updater
            },
            action,
            min_level,
            risk,
            description: id.into(),
            optional: false,
            elevated: false,
        }
    }

    /// Una regla por cada combinación de acción, nivel mínimo, riesgo, opcional y elevada,
    /// con su proceso o servicio en ejecución: cubre todo el espacio de entrada.
    fn every_combination() -> (Vec<Rule>, Snapshot) {
        let mut rules = vec![];
        let mut snap = Snapshot::default();
        let mut pid = 100;
        for action in [
            Action::LowerPriority,
            Action::Suspend,
            Action::Kill,
            Action::StopService,
        ] {
            for min_level in Level::ALL {
                for risk in [Risk::Low, Risk::Medium, Risk::High] {
                    for (optional, elevated) in [(false, false), (true, false), (false, true)] {
                        pid += 1;
                        let name = format!("p{pid}");
                        let mut r = rule(&name, &[&name], action, min_level, risk);
                        r.optional = optional;
                        r.elevated = elevated;
                        rules.push(r);
                        if action == Action::StopService {
                            snap.services.push((name, ServiceState::Running));
                        } else {
                            snap.processes.push(proc(pid, &name));
                        }
                    }
                }
            }
        }
        (rules, snap)
    }

    fn checked_ids(p: &Plan) -> BTreeSet<String> {
        p.checked().map(|i| i.rule_id.clone()).collect()
    }

    #[test]
    fn extremo_is_superset() {
        let (rules, snap) = every_combination();
        let device = samples::hybrid_laptop();
        for elevated in [false, true] {
            let opts = PlanOptions {
                elevated,
                ..PlanOptions::default()
            };
            let [n, a, e] = Level::ALL.map(|l| plan(l, &snap, &rules, &device, &opts));
            assert!(checked_ids(&n).is_subset(&checked_ids(&a)));
            assert!(checked_ids(&a).is_subset(&checked_ids(&e)));
            // El nivel solo cambia lo marcado: las filas son las mismas.
            let rows = |p: &Plan| {
                p.items
                    .iter()
                    .map(|i| i.rule_id.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(rows(&n), rows(&e));
            assert_eq!(n.items.len(), rules.len());
        }
    }

    #[test]
    fn default_checked_follows_level_optional_and_lock() {
        let (rules, snap) = every_combination();
        let device = samples::hybrid_laptop();
        for level in Level::ALL {
            for elevated in [false, true] {
                let opts = PlanOptions {
                    elevated,
                    ..PlanOptions::default()
                };
                for item in plan(level, &snap, &rules, &device, &opts).items {
                    let rule = rules.iter().find(|r| r.id == item.rule_id).unwrap();
                    assert_eq!(item.locked, rule.requires_admin() && !elevated);
                    assert_eq!(
                        item.default_checked,
                        level.includes(rule.min_level) && !rule.optional && !item.locked
                    );
                }
            }
        }
    }

    #[test]
    fn kill_is_only_effective_at_extremo_with_low_risk() {
        let (rules, snap) = every_combination();
        let device = samples::hybrid_laptop();
        for level in Level::ALL {
            for item in plan(level, &snap, &rules, &device, &PlanOptions::default()).items {
                if item.action == Action::Kill {
                    assert_eq!((level, item.risk), (Level::Extremo, Risk::Low));
                }
            }
        }
    }

    #[test]
    fn protected_processes_and_the_game_are_never_planned() {
        let device = samples::hybrid_laptop();
        let names = ["csrss", "dwm", "EasyAntiCheat", "game", "OneDrive"];
        let rules = vec![rule(
            "all",
            &names,
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        )];
        let processes: Vec<ProcessInfo> = names
            .iter()
            .enumerate()
            .map(|(i, n)| proc(i as u32 + 1, &format!("{n}.exe")))
            .collect();
        let game = processes[3].target.clone();
        let snap = Snapshot {
            processes,
            game: Some(game),
            ..Snapshot::default()
        };
        for level in Level::ALL {
            let p = plan(level, &snap, &rules, &device, &PlanOptions::default());
            let Subject::Processes(ps) = &p.items[0].subject else {
                panic!("se esperaban procesos")
            };
            let planned: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(planned, ["OneDrive.exe"]);
        }
    }

    #[test]
    fn rules_without_a_match_on_this_device_do_not_appear() {
        let device = samples::hybrid_laptop();
        let rules = vec![
            rule(
                "absent",
                &["NotRunning"],
                Action::Suspend,
                Level::Normal,
                Risk::Low,
            ),
            rule(
                "svc",
                &["WSearch", "SysMain"],
                Action::StopService,
                Level::Agresivo,
                Risk::Low,
            ),
            rule(
                "stopped",
                &["Spooler"],
                Action::StopService,
                Level::Extremo,
                Risk::Low,
            ),
        ];
        let snap = Snapshot {
            services: vec![
                ("WSearch".into(), ServiceState::Running),
                ("Spooler".into(), ServiceState::Stopped),
            ],
            ..Snapshot::default()
        };
        let p = plan(
            Level::Extremo,
            &snap,
            &rules,
            &device,
            &PlanOptions::default(),
        );
        assert_eq!(p.items.len(), 1);
        assert_eq!(
            p.items[0].subject,
            Subject::Services(vec!["WSearch".into()])
        );
    }

    #[test]
    fn keep_alive_is_never_frozen_but_can_lose_priority() {
        let device = samples::hybrid_laptop();
        let rules = vec![rule(
            "voice",
            &["Discord"],
            Action::Kill,
            Level::Normal,
            Risk::Low,
        )];
        let snap = Snapshot {
            processes: vec![proc(1, "Discord.exe")],
            ..Snapshot::default()
        };
        let opts = PlanOptions {
            elevated: true,
            keep_alive: vec!["discord".into()],
        };
        let p = plan(Level::Extremo, &snap, &rules, &device, &opts);
        assert_eq!(p.items[0].action, Action::LowerPriority);
    }

    #[test]
    fn an_anticheat_disables_game_tuning() {
        let device = samples::hybrid_laptop();
        let opts = PlanOptions::default();
        let clean = Snapshot {
            processes: vec![proc(1, "game.exe")],
            ..Snapshot::default()
        };
        let p = plan(Level::Normal, &clean, &[], &device, &opts);
        assert!(p.anticheats.is_empty());
        assert!(p.game_tuning_allowed());

        let guarded = Snapshot {
            processes: vec![
                proc(1, "game.exe"),
                proc(2, "EasyAntiCheat.exe"),
                proc(3, "easyanticheat.exe"),
                proc(4, "BEService.exe"),
            ],
            ..Snapshot::default()
        };
        let p = plan(Level::Normal, &guarded, &[], &device, &opts);
        assert_eq!(p.anticheats, ["BEService.exe", "EasyAntiCheat.exe"]);
        assert!(!p.game_tuning_allowed());
    }

    #[test]
    fn rules_only_match_the_device_os() {
        let rules = vec![rule(
            "w",
            &["OneDrive"],
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        )];
        let snap = Snapshot {
            processes: vec![proc(1, "OneDrive")],
            ..Snapshot::default()
        };
        let linux = samples::amd_desktop_linux();
        assert!(
            plan(
                Level::Extremo,
                &snap,
                &rules,
                &linux,
                &PlanOptions::default()
            )
            .items
            .is_empty()
        );
    }
}
