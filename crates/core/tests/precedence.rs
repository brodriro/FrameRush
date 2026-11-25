//! Tabla de precedencia P1–P15 (PLAN.md §3): un test por regla que ya tiene código.
//!
//! Quedan fuera P4 (`launch-game` con `profile-auto-apply`: la decisión vive en la UI,
//! `apps/gui/ui/app.js`) y P8 (`steamwebhelper` y el overlay de Steam, sin implementar);
//! tareas.md las lista.

use framerush_core::{
    Action, ApplyRequest, Category, Engine, EngineError, FileStore, GameBoost, JournalStore, Level,
    MemoryOp, OsNames, Plan, PlanItem, PlanOptions, Risk, Rule, Scope, Snapshot,
    persistent_priority_default, plan,
};
use framerush_device::{DeviceProfile, samples};
use framerush_platform::mock::MockPlatform;
use framerush_platform::{
    GameTuning, Os, Priority, ProcessControl, ServiceControl, ServiceState, Tweak,
};
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
        memory: vec![],
        power_plan: None,
        custom_power_plan: None,
        cstates_off: false,
        refresh_rates: vec![],
        game_files: vec![],
        defender_exclusions: vec![],
        items: vec![],
        tweaks: vec![],
        game: Some(GameBoost {
            targets: vec![game],
            priority,
            realtime_confirmed: confirmed,
            fullscreen_optimizations_off: false,
            affinity: None,
            high_performance_gpu: false,
            persistent_priority: false,
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
    let game = confirmed.game.unwrap().targets.remove(0);
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

/// P10: `vbs-hvci-off` queda bloqueado con Vanguard o FACEIT y desmarcado con otro anticheat.
#[test]
fn p10_vbs_hvci_off_yields_to_anticheats() {
    let m = MockPlatform::new(4);
    let mut device = samples::hybrid_laptop();
    device.vbs_hvci_enabled = Some(true);
    let elevated = PlanOptions {
        elevated: true,
        ..PlanOptions::default()
    };
    let with_vbs = |m: &MockPlatform| Snapshot {
        tweaks: vec![Tweak::VbsHvciOff],
        admin_tweaks: vec![Tweak::VbsHvciOff],
        ..snapshot(m)
    };
    let clean = plan(Level::Extremo, &with_vbs(&m), &[], &device, &elevated);
    assert_eq!(clean.tweaks, [Tweak::VbsHvciOff]);

    m.spawn(1, "BEService.exe", "/BEService.exe", 1);
    let other = plan(Level::Extremo, &with_vbs(&m), &[], &device, &elevated);
    assert_eq!(other.optional_tweaks, [Tweak::VbsHvciOff]);
    assert!(other.blocked_tweaks.is_empty());

    m.spawn(2, "vgc.exe", "/vgc.exe", 1);
    let vanguard = plan(Level::Extremo, &with_vbs(&m), &[], &device, &elevated);
    assert!(!vanguard.optional_tweaks.contains(&Tweak::VbsHvciOff));
    assert_eq!(vanguard.blocked_tweaks[0].0, Tweak::VbsHvciOff);
}

/// P5 y §3: al salir del juego solo se revierte lo de sesión; lo persistente espera a
/// Restaurar. Un valor que no existía se borra al restaurar.
#[test]
fn p5_persistent_changes_survive_the_session_revert() {
    let m = MockPlatform::new(4);
    let s = store("p5");
    let engine = Engine::new(&m, &s);
    let updater = m.spawn(1, "updater.exe", "/updater.exe", 1);
    let rules = [rule(
        "u",
        "updater",
        Action::Suspend,
        Level::Normal,
        Risk::Low,
    )];
    let p = plan_for(
        Level::Normal,
        &snapshot(&m),
        &rules,
        &PlanOptions::default(),
    );
    let loc = r"mock\game-dvr-off";
    let request = ApplyRequest {
        memory: vec![],
        power_plan: None,
        custom_power_plan: None,
        cstates_off: false,
        refresh_rates: vec![],
        game_files: vec![],
        defender_exclusions: vec![],
        items: p.checked().collect(),
        tweaks: vec![Tweak::GameDvrOff],
        game: None,
    };
    engine.apply(&request).unwrap();
    assert!(m.is_suspended(&updater).unwrap());
    assert_eq!(m.value(loc), Some(1));

    engine.revert_session().unwrap();
    assert!(!m.is_suspended(&updater).unwrap());
    assert_eq!(m.value(loc), Some(1), "lo persistente sigue aplicado");
    assert!(engine.has_open_session().unwrap());

    engine.revert_all().unwrap();
    assert_eq!(m.value(loc), None, "no existía: Restaurar lo borra");
    assert!(!engine.has_open_session().unwrap());
}

/// §3: `notifications-off` es de sesión y vuelve al cerrar el juego; los demás ajustes no.
#[test]
fn notifications_come_back_with_the_session_revert() {
    let m = MockPlatform::new(4);
    let s = store("notif");
    let engine = Engine::new(&m, &s);
    let request = ApplyRequest {
        memory: vec![],
        tweaks: vec![Tweak::NotificationsOff, Tweak::VisualEffectsPerformance],
        ..ApplyRequest::default()
    };
    engine.apply(&request).unwrap();
    engine.revert_session().unwrap();
    assert_eq!(m.value(r"mock\notifications-off"), None);
    assert_eq!(m.value(r"mock\visual-effects-performance"), Some(1));
    engine.revert_all().unwrap();
    assert_eq!(m.value(r"mock\visual-effects-performance"), None);
}

/// `fullscreen-optimizations-off`: persistente, solo si el exe no tenía flags, y Restaurar
/// los borra.
#[test]
fn fso_off_is_persistent_and_respects_existing_flags() {
    let m = MockPlatform::new(4);
    let s = store("fso");
    let engine = Engine::new(&m, &s);
    let mut request = boost(&m, Priority::High, false);
    let exe = request.game.as_ref().unwrap().targets[0].exe_path.clone();
    if let Some(g) = request.game.as_mut() {
        g.fullscreen_optimizations_off = true;
    }

    let report = engine.apply(&request).unwrap();
    assert_eq!(report.done(), 2, "{report:?}");
    assert_eq!(
        m.compat_flags(&exe).unwrap().as_deref(),
        Some(framerush_platform::mock::MOCK_FSO_FLAG)
    );
    engine.revert_session().unwrap();
    assert!(m.compat_flags(&exe).unwrap().is_some(), "persistente");
    engine.revert_all().unwrap();
    assert_eq!(m.compat_flags(&exe).unwrap(), None);

    // Con flags previos del usuario no se toca nada.
    m.set_compat_flags(&exe.to_string_lossy(), "~ RUNASADMIN");
    let report = engine.apply(&request).unwrap();
    assert_eq!(report.done(), 1, "solo la prioridad: {report:?}");
    engine.revert_all().unwrap();
    assert_eq!(
        m.compat_flags(&exe).unwrap().as_deref(),
        Some("~ RUNASADMIN")
    );
}

/// P1: la prioridad persistente (IFEO) solo pone Alta, viene desde Agresivo, no se aplica con
/// Normal, sobrevive al cierre del juego y Restaurar deja el valor de antes.
#[test]
fn p1_persistent_priority_is_high_only_and_persistent() {
    use framerush_platform::mock::{MOCK_HIGH_PRIORITY, mock_priority_location};
    assert!(!persistent_priority_default(Level::Normal, Priority::High));
    assert!(persistent_priority_default(Level::Agresivo, Priority::High));
    assert!(persistent_priority_default(
        Level::Extremo,
        Priority::Realtime
    ));
    assert!(!persistent_priority_default(
        Level::Extremo,
        Priority::Normal
    ));

    let m = MockPlatform::new(4);
    let s = store("p1-ifeo");
    let engine = Engine::new(&m, &s);
    let mut request = boost(&m, Priority::High, false);
    let exe = request.game.as_ref().unwrap().targets[0].exe_path.clone();
    let loc = mock_priority_location(&exe);
    if let Some(g) = request.game.as_mut() {
        g.persistent_priority = true;
    }
    m.set_value(&loc, 2);
    engine.apply(&request).unwrap();
    assert_eq!(m.value(&loc), Some(MOCK_HIGH_PRIORITY));
    engine.apply(&request).unwrap();
    engine.revert_session().unwrap();
    assert_eq!(m.value(&loc), Some(MOCK_HIGH_PRIORITY), "persistente");
    engine.revert_all().unwrap();
    assert_eq!(m.value(&loc), Some(2), "backup único: el valor de antes");

    if let Some(g) = request.game.as_mut() {
        g.priority = Priority::Normal;
    }
    m.set_value(&loc, 2);
    engine.apply(&request).unwrap();
    assert_eq!(m.value(&loc), Some(2), "con Normal no se escribe");
    engine.revert_all().unwrap();
}

/// `gpu-preference = alto` es persistente y no pisa una preferencia que el usuario ya tenía.
#[test]
fn gpu_preference_is_persistent_and_respects_the_users() {
    use framerush_platform::mock::MOCK_HIGH_PERFORMANCE_GPU;
    let m = MockPlatform::new(4);
    let s = store("gpu");
    let engine = Engine::new(&m, &s);
    let mut request = boost(&m, Priority::High, false);
    let exe = request.game.as_ref().unwrap().targets[0].exe_path.clone();
    if let Some(g) = request.game.as_mut() {
        g.high_performance_gpu = true;
    }
    engine.apply(&request).unwrap();
    assert_eq!(
        m.gpu_preference(&exe).unwrap().as_deref(),
        Some(MOCK_HIGH_PERFORMANCE_GPU)
    );
    engine.revert_session().unwrap();
    assert!(m.gpu_preference(&exe).unwrap().is_some(), "persistente");
    engine.revert_all().unwrap();
    assert_eq!(m.gpu_preference(&exe).unwrap(), None);

    m.set_gpu_preference(&exe.to_string_lossy(), "GpuPreference=1;");
    let report = engine.apply(&request).unwrap();
    assert_eq!(report.done(), 1, "solo la prioridad: {report:?}");
    engine.revert_all().unwrap();
    assert_eq!(
        m.gpu_preference(&exe).unwrap().as_deref(),
        Some("GpuPreference=1;")
    );
}

/// P13: "forzar GPU dedicada" (It5.7) no es un ajuste aparte, es `gpu-preference = alto`, su
/// valor por defecto: la GPU con más VRAM dedicada, y solo con más de una GPU (R8, P15).
#[test]
fn p13_forcing_the_dedicated_gpu_is_gpu_preference_alto() {
    use framerush_core::defaults;
    use framerush_platform::mock::MOCK_HIGH_PERFORMANCE_GPU;

    let mut laptop = samples::hybrid_laptop();
    let alto = |d: &DeviceProfile| defaults::gpu_preference(d).map(|g| g.id.clone());
    assert_eq!(alto(&laptop).as_deref(), Some("gpu-dedicada"));
    // Una integrada que declara algo de VRAM propia no gana a la dedicada.
    laptop.gpus[0].dedicated_vram_bytes = Some(128 * 1024 * 1024);
    assert_eq!(alto(&laptop).as_deref(), Some("gpu-dedicada"));
    // Con una sola GPU no hay nada que forzar: el ajuste no aparece.
    laptop.gpus.remove(0);
    assert_eq!(alto(&laptop), None);

    // `alto` es lo que escribe el motor al pedir la GPU de alto rendimiento para el juego.
    let m = MockPlatform::new(4);
    let s = store("p13");
    let engine = Engine::new(&m, &s);
    let mut request = boost(&m, Priority::High, false);
    let exe = request.game.as_ref().unwrap().targets[0].exe_path.clone();
    if let Some(g) = request.game.as_mut() {
        g.high_performance_gpu = true;
    }
    engine.apply(&request).unwrap();
    assert_eq!(
        m.gpu_preference(&exe).unwrap().as_deref(),
        Some(MOCK_HIGH_PERFORMANCE_GPU)
    );
    engine.revert_all().unwrap();
}

/// Backup único: reaplicar un ajuste no sobrescribe el valor original.
#[test]
fn reapplying_a_tweak_keeps_the_original_backup() {
    let m = MockPlatform::new(4);
    let s = store("backup");
    let engine = Engine::new(&m, &s);
    let loc = r"mock\windows-game-mode";
    m.set_value(loc, 7);
    let request = ApplyRequest {
        memory: vec![],
        tweaks: vec![Tweak::WindowsGameMode],
        ..ApplyRequest::default()
    };
    engine.apply(&request).unwrap();
    let again = engine.apply(&request).unwrap();
    assert_eq!(again.done(), 0);
    engine.revert_all().unwrap();
    assert_eq!(m.value(loc), Some(7));
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

/// P3: `power-plan` elige la base y `custom-power-plan` la duplica como "FrameRush Max" y
/// activa el duplicado; Restaurar activa el plan original y borra el duplicado.
#[test]
fn p3_custom_power_plan_duplicates_the_level_plan() {
    use framerush_core::{CUSTOM_POWER_PLAN_NAME, defaults};
    use framerush_platform::PowerControl;

    let laptop = samples::hybrid_laptop();
    let base = defaults::custom_power_plan_base(Level::Agresivo, &laptop)
        .expect("el portátil de ejemplo tiene el plan del nivel");
    assert_eq!(
        Some(base),
        defaults::power_plan_for(Level::Agresivo, &laptop),
        "la base es el plan del nivel"
    );
    let original = laptop
        .active_power_plan
        .clone()
        .expect("el portátil de ejemplo tiene plan activo");

    let m = MockPlatform::new(4);
    m.set_power_plans(laptop.power_plans.clone(), &original);
    let store = store("p3");
    let engine = Engine::new(&m, &store);
    engine
        .apply(&ApplyRequest {
            power_plan: defaults::power_plan_for(Level::Agresivo, &laptop).cloned(),
            custom_power_plan: Some(base.clone()),
            ..ApplyRequest::default()
        })
        .unwrap();
    assert_eq!(m.active_power_plan().unwrap().name, CUSTOM_POWER_PLAN_NAME);
    assert_eq!(m.power_plans().unwrap().len(), laptop.power_plans.len() + 1);

    engine.revert_all().unwrap();
    assert_eq!(m.active_power_plan().unwrap().id, original);
    assert_eq!(m.power_plans().unwrap(), laptop.power_plans);
}

/// Filas que Turbo manda al motor: las marcadas, con cerrar convertido en congelar. Reproduce
/// `freeze_instead_of_kill` de la CLI y de la GUI, que viven en binarios y no se pueden importar
/// aquí; la CLI prueba la suya en `apps/cli`.
fn freeze_instead_of_kill(mut item: PlanItem) -> PlanItem {
    if item.action == Action::Kill {
        item.action = Action::Suspend;
    }
    item
}

fn turbo_items(p: &Plan) -> Vec<PlanItem> {
    p.checked().cloned().map(freeze_instead_of_kill).collect()
}

fn turbo_request(items: &[PlanItem]) -> ApplyRequest<'_> {
    ApplyRequest {
        items: items.iter().collect(),
        ..ApplyRequest::default()
    }
}

/// P6: Turbo congela las filas marcadas con `suspend` o `kill` (kill pasa a congelar) y nada
/// más: ni las desmarcadas, ni los protegidos, ni `keep-alive-list`; los servicios siguen su
/// propia fila (se detienen, no se congelan). Al salir se reanuda todo lo congelado.
#[test]
fn p6_turbo_freezes_checked_rows_and_kill_becomes_freeze() {
    let m = MockPlatform::new(4);
    let killed = m.spawn(1, "killer.exe", "/killer.exe", 1);
    let frozen = m.spawn(2, "updater.exe", "/updater.exe", 1);
    let unchecked = m.spawn(3, "later.exe", "/later.exe", 1);
    let voice = m.spawn(4, "Discord.exe", "/Discord.exe", 1);
    let critical = m.spawn(5, "csrss.exe", "/csrss.exe", 1);
    m.add_service("WSearch", ServiceState::Running);
    let mut optional = rule("later", "later", Action::Suspend, Level::Normal, Risk::Low);
    optional.optional = true;
    let rules = [
        rule("kill", "killer", Action::Kill, Level::Extremo, Risk::Low),
        rule(
            "freeze",
            "updater",
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        ),
        optional,
        rule("voice", "Discord", Action::Kill, Level::Normal, Risk::Low),
        rule("os", "csrss", Action::Suspend, Level::Normal, Risk::Low),
        rule(
            "search",
            "WSearch",
            Action::StopService,
            Level::Normal,
            Risk::Low,
        ),
    ];
    let snap = Snapshot {
        services: vec![("WSearch".into(), ServiceState::Running)],
        ..snapshot(&m)
    };
    let options = PlanOptions {
        elevated: true,
        keep_alive: vec!["discord".into()],
    };
    let p = plan_for(Level::Extremo, &snap, &rules, &options);
    assert!(
        p.items.iter().all(|i| i.rule_id != "os"),
        "el protegido no entra en el plan"
    );
    let kill_row = p.items.iter().find(|i| i.rule_id == "kill");
    assert_eq!(kill_row.map(|i| i.action), Some(Action::Kill));

    let items = turbo_items(&p);
    let actions: Vec<(&str, Action)> = items
        .iter()
        .map(|i| (i.rule_id.as_str(), i.action))
        .collect();
    assert_eq!(
        actions,
        [
            ("kill", Action::Suspend),
            ("freeze", Action::Suspend),
            ("voice", Action::LowerPriority),
            ("search", Action::StopService),
        ]
    );

    let s = store("p6");
    let engine = Engine::new(&m, &s);
    engine.apply(&turbo_request(&items)).unwrap();
    assert!(m.is_suspended(&killed).unwrap(), "congelado, no cerrado");
    assert!(m.is_suspended(&frozen).unwrap());
    assert!(!m.is_suspended(&unchecked).unwrap(), "desmarcada");
    assert!(!m.is_suspended(&voice).unwrap(), "keep-alive-list");
    assert_ne!(m.priority(&voice).unwrap(), Priority::Normal);
    assert!(!m.is_suspended(&critical).unwrap(), "protegido");
    assert_eq!(
        m.service_state("WSearch").unwrap(),
        Some(ServiceState::Stopped)
    );

    engine.resume_frozen().unwrap();
    assert!(!m.is_suspended(&killed).unwrap(), "lo congelado vuelve");
    assert!(!m.is_suspended(&frozen).unwrap());
    assert_eq!(
        m.service_state("WSearch").unwrap(),
        Some(ServiceState::Stopped),
        "el servicio sigue su fila, no la de los congelados"
    );
    engine.revert_all().unwrap();
}

/// P6 y `turbo-rescan`: las instancias nuevas de las filas congeladas se congelan también; las
/// de filas que no se congelaron, no. Lo ya congelado no se congela dos veces y al salir todo se
/// reanuda.
#[test]
fn p6_turbo_rescan_freezes_new_instances_of_frozen_rows() {
    let m = MockPlatform::new(4);
    let first = m.spawn(1, "updater.exe", "/updater.exe", 1);
    let mut optional = rule("later", "later", Action::Suspend, Level::Normal, Risk::Low);
    optional.optional = true;
    let rules = [
        rule("freeze", "updater", Action::Kill, Level::Normal, Risk::Low),
        optional,
    ];
    let options = PlanOptions {
        elevated: true,
        ..PlanOptions::default()
    };
    let p = plan_for(Level::Extremo, &snapshot(&m), &rules, &options);
    let items = turbo_items(&p);
    let s = store("p6-rescan");
    let engine = Engine::new(&m, &s);
    engine.apply(&turbo_request(&items)).unwrap();
    let rows: Vec<String> = items
        .iter()
        .filter(|i| i.action == Action::Suspend)
        .map(|i| i.rule_id.clone())
        .collect();
    assert_eq!(rows, ["freeze"]);

    // Re-escaneo (como `rescan` de la CLI): aparecen otra instancia de la fila congelada y una
    // de la desmarcada.
    let second = m.spawn(2, "updater.exe", "/updater.exe", 1);
    let later = m.spawn(3, "later.exe", "/later.exe", 1);
    let again = plan_for(Level::Extremo, &snapshot(&m), &rules, &options);
    let rescan: Vec<PlanItem> = again
        .items
        .into_iter()
        .filter(|i| !i.locked && rows.contains(&i.rule_id))
        .map(freeze_instead_of_kill)
        .filter(|i| i.action == Action::Suspend)
        .collect();
    engine.apply(&turbo_request(&rescan)).unwrap();
    assert!(m.is_suspended(&first).unwrap());
    assert!(
        m.is_suspended(&second).unwrap(),
        "instancia nueva congelada"
    );
    assert!(!m.is_suspended(&later).unwrap(), "fila no congelada");

    engine.resume_frozen().unwrap();
    assert!(!m.is_suspended(&first).unwrap(), "no se congeló dos veces");
    assert!(!m.is_suspended(&second).unwrap());
    engine.revert_all().unwrap();
}

/// P5 con Turbo: lo congelado se reanuda al salir con el toggle apagado (`resume_frozen`) y
/// encendido (`revert_session`). El toggle decide el resto de la sesión; lo persistente nunca
/// se revierte al salir.
#[test]
fn p5_turbo_frozen_always_resume_and_the_toggle_rules_the_rest() {
    for auto_revert in [false, true] {
        let m = MockPlatform::new(4);
        let killed = m.spawn(1, "killer.exe", "/killer.exe", 1);
        let lowered = m.spawn(2, "sync.exe", "/sync.exe", 1);
        let rules = [
            rule("kill", "killer", Action::Kill, Level::Extremo, Risk::Low),
            rule(
                "lower",
                "sync",
                Action::LowerPriority,
                Level::Normal,
                Risk::Low,
            ),
        ];
        let p = plan_for(
            Level::Extremo,
            &snapshot(&m),
            &rules,
            &PlanOptions::default(),
        );
        let items = turbo_items(&p);
        let s = store(&format!("p5-turbo-{auto_revert}"));
        let engine = Engine::new(&m, &s);
        let request = ApplyRequest {
            tweaks: vec![Tweak::GameDvrOff],
            ..turbo_request(&items)
        };
        engine.apply(&request).unwrap();
        assert!(m.is_suspended(&killed).unwrap());
        assert_ne!(m.priority(&lowered).unwrap(), Priority::Normal);

        if auto_revert {
            engine.revert_session().unwrap();
        } else {
            engine.resume_frozen().unwrap();
        }
        assert!(
            !m.is_suspended(&killed).unwrap(),
            "congelado reanudado (auto_revert = {auto_revert})"
        );
        assert_eq!(
            m.priority(&lowered).unwrap() == Priority::Normal,
            auto_revert,
            "el toggle gobierna el resto de la sesión"
        );
        assert_eq!(
            m.value(r"mock\game-dvr-off"),
            Some(1),
            "lo persistente no se revierte al salir"
        );
        assert!(engine.has_open_session().unwrap());
        engine.revert_all().unwrap();
        assert_eq!(m.value(r"mock\game-dvr-off"), None);
    }
}

/// P9: Turbo libera memoria solo si `memory-trim`/`standby-purge` están en el plan: desde
/// Agresivo y con elevación. En Normal, o sin admin (candado), Turbo solo congela.
#[test]
fn p9_turbo_frees_memory_only_from_agresivo_with_admin() {
    let cases = [
        (Level::Normal, true, false),
        (Level::Agresivo, false, false),
        (Level::Agresivo, true, true),
        (Level::Extremo, true, true),
    ];
    for (level, elevated, frees) in cases {
        let m = MockPlatform::new(4);
        let frozen = m.spawn(1, "updater.exe", "/updater.exe", 1);
        let background = m.spawn(2, "notes.exe", "/notes.exe", 1);
        let rules = [rule(
            "freeze",
            "updater",
            Action::Suspend,
            Level::Normal,
            Risk::Low,
        )];
        let snap = Snapshot {
            memory: MemoryOp::ALL.to_vec(),
            ..snapshot(&m)
        };
        let options = PlanOptions {
            elevated,
            ..PlanOptions::default()
        };
        let p = plan_for(level, &snap, &rules, &options);
        assert_eq!(!p.memory.is_empty(), frees, "{level:?}, admin {elevated}");
        if !elevated && level != Level::Normal {
            assert_eq!(p.locked_memory, MemoryOp::ALL, "con candado sin admin");
        }

        let items = turbo_items(&p);
        let s = store(&format!("p9-{level:?}-{elevated}"));
        let engine = Engine::new(&m, &s);
        let request = ApplyRequest {
            memory: p.memory.clone(),
            ..turbo_request(&items)
        };
        engine.apply(&request).unwrap();
        assert!(m.is_suspended(&frozen).unwrap(), "Turbo congela siempre");
        let expected = u32::from(frees);
        assert_eq!(m.standby_purges(), expected, "{level:?}, admin {elevated}");
        assert_eq!(m.trims(&background).unwrap(), expected);
        engine.revert_all().unwrap();
    }
}
