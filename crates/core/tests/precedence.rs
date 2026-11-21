//! Tabla de precedencia P1–P15 (PLAN.md §3): un test por regla que ya tiene código.
//!
//! Las reglas que dependen de piezas que aún no existen (IFEO, planes de energía, Turbo,
//! `steamwebhelper`, VBS, GPU) se prueban en su hito; tareas.md las lista.

use framerush_core::{
    Action, ApplyRequest, Category, Engine, EngineError, FileStore, GameBoost, JournalStore, Level,
    OsNames, Plan, PlanOptions, Risk, Rule, Scope, Snapshot, plan,
};
use framerush_device::{DeviceProfile, samples};
use framerush_platform::mock::MockPlatform;
use framerush_platform::{Os, Priority, ProcessControl, ServiceState};
use std::path::PathBuf;

fn rule(id: &str, name: &str, action: Action, min_level: Level, risk: Risk) -> Rule {
    Rule {
        id: id.into(),
        matches: OsNames {
            windows: vec![name.into()],
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

fn snapshot(m: &MockPlatform) -> Snapshot {
    Snapshot {
        processes: m.processes().unwrap(),
        ..Snapshot::default()
    }
}

fn plan_for(level: Level, snap: &Snapshot, rules: &[Rule], options: &PlanOptions) -> Plan {
    plan(level, snap, rules, &samples::hybrid_laptop(), options)
}

/// Journal propio del test en la carpeta temporal.
fn store(name: &str) -> FileStore {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "framerush-precedence-{name}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    FileStore::new(path)
}

fn boost(m: &MockPlatform, priority: Priority, confirmed: bool) -> ApplyRequest<'static> {
    let game = m.spawn(9, "game.exe", "/game.exe", 1);
    ApplyRequest {
        items: vec![],
        game: Some(GameBoost {
            target: game,
            priority,
            realtime_confirmed: confirmed,
        }),
    }
}

/// P1: Tiempo real pide confirmación explícita y es solo de sesión.
#[test]
fn p1_realtime_needs_confirmation_and_is_session_only() {
    let m = MockPlatform::new(4);
    let s = store("p1");
    let engine = Engine::new(&m, &s);

    let unconfirmed = boost(&m, Priority::Realtime, false);
    assert!(matches!(
        engine.apply(&unconfirmed),
        Err(EngineError::RealtimeNotConfirmed)
    ));
    assert!(!engine.has_open_session().unwrap());

    let mut confirmed = unconfirmed;
    if let Some(g) = confirmed.game.as_mut() {
        g.realtime_confirmed = true;
    }
    engine.apply(&confirmed).unwrap();
    let journal = s.load().unwrap();
    assert!(
        journal
            .entries
            .iter()
            .all(|e| e.change.scope() == Scope::Session)
    );

    engine.revert_session().unwrap();
    let game = confirmed.game.unwrap().target;
    assert_eq!(m.priority(&game).unwrap(), Priority::Normal);
}

/// P2: con un anticheat en ejecución, afinidad y timer quedan desactivados.
#[test]
fn p2_anticheat_turns_off_game_tuning() {
    let m = MockPlatform::new(4);
    m.spawn(1, "game.exe", "/game.exe", 1);
    let clean = plan_for(Level::Extremo, &snapshot(&m), &[], &PlanOptions::default());
    assert!(clean.game_tuning_allowed());

    m.spawn(2, "BEService.exe", "/BEService.exe", 1);
    let guarded = plan_for(Level::Extremo, &snapshot(&m), &[], &PlanOptions::default());
    assert!(!guarded.game_tuning_allowed());
    assert_eq!(guarded.anticheats, ["BEService.exe"]);
}

/// P7: `keep-alive-list` solo impide congelar; bajar la prioridad sigue permitido.
#[test]
fn p7_keep_alive_only_blocks_freezing() {
    let m = MockPlatform::new(4);
    m.spawn(1, "Discord.exe", "/Discord.exe", 1);
    let rules = [
        rule("kill", "Discord", Action::Kill, Level::Normal, Risk::Low),
        rule(
            "freeze",
            "Discord",
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        ),
        rule(
            "lower",
            "Discord",
            Action::LowerPriority,
            Level::Normal,
            Risk::Low,
        ),
    ];
    let options = PlanOptions {
        elevated: true,
        keep_alive: vec!["discord".into()],
    };
    let p = plan_for(Level::Extremo, &snapshot(&m), &rules, &options);
    assert_eq!(p.items.len(), 3);
    assert!(p.items.iter().all(|i| i.action == Action::LowerPriority));
}

/// P11: el plan es un dry-run; no toca la plataforma.
#[test]
fn p11_planning_changes_nothing() {
    let m = MockPlatform::new(4);
    let t = m.spawn(1, "updater.exe", "/updater.exe", 1);
    let rules = [rule(
        "u",
        "updater",
        Action::Suspend,
        Level::Normal,
        Risk::Low,
    )];
    let p = plan_for(
        Level::Extremo,
        &snapshot(&m),
        &rules,
        &PlanOptions::default(),
    );
    assert_eq!(p.checked().count(), 1);
    assert!(!m.is_suspended(&t).unwrap());
    assert_eq!(m.priority(&t).unwrap(), Priority::Normal);
}

/// P12 y P14: lo que pide admin se muestra con candado y desmarcado hasta elevar.
#[test]
fn p12_p14_admin_rows_stay_visible_but_locked() {
    let snap = Snapshot {
        services: vec![("WSearch".into(), ServiceState::Running)],
        ..Snapshot::default()
    };
    let rules = [rule(
        "search",
        "WSearch",
        Action::StopService,
        Level::Agresivo,
        Risk::Low,
    )];
    let without = plan_for(Level::Agresivo, &snap, &rules, &PlanOptions::default());
    assert_eq!(without.items.len(), 1);
    assert!(without.items[0].locked);
    assert!(!without.items[0].default_checked);

    let elevated = PlanOptions {
        elevated: true,
        ..PlanOptions::default()
    };
    let with = plan_for(Level::Agresivo, &snap, &rules, &elevated);
    assert!(!with.items[0].locked);
    assert!(with.items[0].default_checked);
}

/// P15: manda el equipo; lo que no está en él no aparece.
#[test]
fn p15_rows_only_exist_for_what_the_device_has() {
    let m = MockPlatform::new(4);
    m.spawn(1, "updater.exe", "/updater.exe", 1);
    let rules = [
        rule(
            "present",
            "updater",
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        ),
        rule(
            "absent",
            "not-running",
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        ),
    ];
    let snap = snapshot(&m);
    let p = plan_for(Level::Extremo, &snap, &rules, &PlanOptions::default());
    let ids: Vec<&str> = p.items.iter().map(|i| i.rule_id.as_str()).collect();
    assert_eq!(ids, ["present"]);

    // Reglas solo de Windows en un equipo Linux: ninguna fila.
    let linux = DeviceProfile::undetected(Os::Linux);
    let p = plan(
        Level::Extremo,
        &snap,
        &rules,
        &linux,
        &PlanOptions::default(),
    );
    assert!(p.items.is_empty());
}
