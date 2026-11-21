//! Motor `apply` / `revert` / `undo-last` (PLAN.md §3 "Flujo principal", It1.1, It2.4, It5.5).
//!
//! Reglas que cumple:
//! - **Journal antes de actuar**: cada cambio se guarda como `Pending` antes de llamar al SO;
//!   si no se puede guardar, no se ejecuta.
//! - **Backup una sola vez**: reaplicar no sobrescribe el estado original de un proceso que ya
//!   está en el journal, y lo que ya está aplicado no se repite.
//! - **Revalidación**: antes de deshacer, se comprueba que el PID sigue siendo el mismo proceso.
//! - **Orden inverso** al revertir. Kill no se deshace: se informa.
//! - **Tiempo real** solo con confirmación explícita.

use crate::journal::{Change, Entry, EntryStatus, Journal, JournalError, JournalStore};
use crate::protected::is_protected;
use crate::{Action, PlanItem, Subject};
use framerush_platform::{Platform, PlatformError, Priority, Target};

/// Prioridad que se pone al bajar la de un proceso de fondo (propuesta de diseño, PLAN.md §6).
pub const LOWERED_PRIORITY: Priority = Priority::BelowNormal;

/// Prioridad que se le da al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameBoost {
    pub target: Target,
    pub priority: Priority,
    /// Confirmación explícita para Tiempo real (It1.5).
    pub realtime_confirmed: bool,
}

/// Qué aplicar: las filas que el usuario dejó marcadas y, opcionalmente, la prioridad del juego.
#[derive(Debug, Clone, Default)]
pub struct ApplyRequest<'a> {
    pub items: Vec<&'a PlanItem>,
    pub game: Option<GameBoost>,
}

/// Resultado de una acción.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// No se hizo nada, con el motivo (ya aplicado, proceso cerrado, PID reutilizado…).
    Skipped(String),
    Failed(PlatformError),
}

/// Una línea del informe: qué se intentó y cómo terminó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportLine {
    pub what: String,
    pub outcome: Outcome,
}

/// Informe de `apply` o `revert`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<ReportLine>,
}

impl Report {
    pub fn done(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.outcome == Outcome::Done)
            .count()
    }

    pub fn failed(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l.outcome, Outcome::Failed(_)))
            .count()
    }

    fn push(&mut self, what: String, outcome: Outcome) {
        self.lines.push(ReportLine { what, outcome });
    }
}

/// Error que impide aplicar o revertir.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("la prioridad Tiempo real necesita confirmación explícita")]
    RealtimeNotConfirmed,
}

/// Texto de un cambio para el informe.
pub fn describe(change: &Change) -> String {
    let proc = |t: &Target| format!("{} (pid {})", t.exe_path.display(), t.pid);
    match change {
        Change::Priority {
            target, applied, ..
        } => format!("prioridad {applied:?} → {}", proc(target)),
        Change::Suspend { target } => format!("suspender {}", proc(target)),
        Change::Kill { target } => format!("cerrar {}", proc(target)),
        Change::StopService { name } => format!("detener el servicio {name}"),
    }
}

/// Motor sobre una plataforma y un journal.
#[derive(Debug)]
pub struct Engine<'a, P, S> {
    platform: &'a P,
    store: &'a S,
}

impl<'a, P: Platform, S: JournalStore> Engine<'a, P, S> {
    pub fn new(platform: &'a P, store: &'a S) -> Self {
        Self { platform, store }
    }

    /// Hay una sesión sin cerrar (para ofrecer "Restaurar sesión anterior" al arrancar).
    pub fn has_open_session(&self) -> Result<bool, EngineError> {
        Ok(self.store.load()?.is_open())
    }

    /// Aplica las filas marcadas y la prioridad del juego como un lote nuevo.
    pub fn apply(&self, request: &ApplyRequest<'_>) -> Result<Report, EngineError> {
        if let Some(g) = &request.game
            && g.priority == Priority::Realtime
            && !g.realtime_confirmed
        {
            return Err(EngineError::RealtimeNotConfirmed);
        }
        let mut journal = self.store.load()?;
        let batch = journal.last_batch().map_or(1, |b| b + 1);
        let mut report = Report::default();

        for item in &request.items {
            match &item.subject {
                Subject::Processes(ps) => {
                    for p in ps.iter().filter(|p| !is_protected(&p.name)) {
                        let t = &p.target;
                        match item.action {
                            Action::LowerPriority => self.set_priority(
                                &mut journal,
                                batch,
                                t,
                                LOWERED_PRIORITY,
                                &mut report,
                            )?,
                            Action::Suspend => {
                                let c = Change::Suspend { target: t.clone() };
                                self.run(&mut journal, batch, c, &mut report)?
                            }
                            Action::Kill => {
                                let c = Change::Kill { target: t.clone() };
                                self.run(&mut journal, batch, c, &mut report)?
                            }
                            Action::StopService => {}
                        }
                    }
                }
                Subject::Services(names) => {
                    for name in names {
                        let c = Change::StopService { name: name.clone() };
                        self.run(&mut journal, batch, c, &mut report)?;
                    }
                }
            }
        }
        if let Some(g) = &request.game {
            self.set_priority(&mut journal, batch, &g.target, g.priority, &mut report)?;
        }
        Ok(report)
    }

    /// Deshace toda la sesión. Lo que no se pudo deshacer queda en el journal.
    pub fn revert_all(&self) -> Result<Report, EngineError> {
        self.revert_where(|_| true)
    }

    /// Deshace solo el último lote (`undo-last`).
    pub fn undo_last(&self) -> Result<Report, EngineError> {
        match self.store.load()?.last_batch() {
            Some(last) => self.revert_where(|e| e.batch == last),
            None => Ok(Report::default()),
        }
    }

    /// Cambio de prioridad con backup único: si el proceso ya está en el journal, se conserva
    /// su prioridad original en vez de leer la actual (que ya es la que pusimos).
    fn set_priority(
        &self,
        journal: &mut Journal,
        batch: u64,
        target: &Target,
        applied: Priority,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let original = journal.entries.iter().find_map(|e| match &e.change {
            Change::Priority {
                target: t,
                previous,
                ..
            } if t == target && e.status == EntryStatus::Done => Some(*previous),
            _ => None,
        });
        let previous = match original {
            Some(p) => p,
            None => match self.platform.priority(target) {
                Ok(p) => p,
                Err(e) => {
                    let what = format!("leer la prioridad de {}", target.exe_path.display());
                    report.push(what, Outcome::Failed(e));
                    return Ok(());
                }
            },
        };
        let change = Change::Priority {
            target: target.clone(),
            previous,
            applied,
        };
        self.run(journal, batch, change, report)
    }

    /// Registra el cambio, lo ejecuta y anota el resultado.
    fn run(
        &self,
        journal: &mut Journal,
        batch: u64,
        change: Change,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let what = describe(&change);
        if already_applied(journal, &change) {
            report.push(what, Outcome::Skipped("ya aplicado en esta sesión".into()));
            return Ok(());
        }
        let index = journal.entries.len();
        journal.entries.push(Entry {
            seq: journal.next_seq(),
            batch,
            change: change.clone(),
            status: EntryStatus::Pending,
        });
        self.store.save(journal)?;

        let result = match &change {
            Change::Priority {
                target, applied, ..
            } => self.platform.set_priority(target, *applied),
            Change::Suspend { target } => self.platform.suspend(target),
            Change::Kill { target } => self.platform.kill(target),
            Change::StopService { name } => self.platform.stop_service(name),
        };
        journal.entries[index].status = match result {
            Ok(()) => EntryStatus::Done,
            Err(_) => EntryStatus::Failed,
        };
        self.store.save(journal)?;
        report.push(
            what,
            result.map_or_else(Outcome::Failed, |()| Outcome::Done),
        );
        Ok(())
    }

    fn revert_where(&self, select: impl Fn(&Entry) -> bool) -> Result<Report, EngineError> {
        let mut journal = self.store.load()?;
        let mut report = Report::default();
        let mut selected: Vec<Entry> = journal
            .entries
            .iter()
            .filter(|e| select(e) && e.status != EntryStatus::Failed)
            .cloned()
            .collect();
        selected.sort_by_key(|e| std::cmp::Reverse(e.seq));

        let mut keep = vec![];
        for entry in selected {
            let outcome = self.undo(&entry.change);
            if matches!(outcome, Outcome::Failed(_)) {
                keep.push(entry.seq);
            }
            report.push(describe(&entry.change), outcome);
        }
        journal
            .entries
            .retain(|e| !select(e) || keep.contains(&e.seq));
        self.store.save(&journal)?;
        Ok(report)
    }

    fn undo(&self, change: &Change) -> Outcome {
        if let Some(target) = change.target().filter(|_| change.is_reversible()) {
            match self.platform.is_same_process(target) {
                Ok(true) => {}
                Ok(false) => {
                    return Outcome::Skipped(
                        "el proceso ya no existe o su PID es de otro proceso".into(),
                    );
                }
                Err(e) => return Outcome::Failed(e),
            }
        }
        let result = match change {
            Change::Kill { .. } => {
                return Outcome::Skipped("cerrar un proceso no se puede deshacer".into());
            }
            Change::Priority {
                target, previous, ..
            } => self.platform.set_priority(target, *previous),
            Change::Suspend { target } => self.platform.resume(target),
            Change::StopService { name } => self.platform.start_service(name),
        };
        result.map_or_else(Outcome::Failed, |()| Outcome::Done)
    }
}

/// `true` si el mismo cambio ya está aplicado en esta sesión.
fn already_applied(journal: &Journal, change: &Change) -> bool {
    journal
        .entries
        .iter()
        .filter(|e| e.status == EntryStatus::Done)
        .any(|e| match (&e.change, change) {
            (
                Change::Priority {
                    target: a,
                    applied: x,
                    ..
                },
                Change::Priority {
                    target: b,
                    applied: y,
                    ..
                },
            ) => a == b && x == y,
            (Change::Suspend { target: a }, Change::Suspend { target: b }) => a == b,
            (Change::StopService { name: a }, Change::StopService { name: b }) => a == b,
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, Level, Risk};
    use framerush_platform::mock::MockPlatform;
    use framerush_platform::{ProcessControl, ProcessInfo, ServiceControl, ServiceState};
    use std::sync::Mutex;

    /// Journal en memoria; con `fail_saves` simula un disco que no se puede escribir.
    #[derive(Debug, Default)]
    struct MemoryStore {
        journal: Mutex<Journal>,
        fail_saves: bool,
    }

    impl JournalStore for MemoryStore {
        fn load(&self) -> Result<Journal, JournalError> {
            Ok(self.journal.lock().unwrap().clone())
        }

        fn save(&self, journal: &Journal) -> Result<(), JournalError> {
            if self.fail_saves {
                return Err(JournalError::Io {
                    path: "memoria".into(),
                    source: std::io::Error::other("disco lleno"),
                });
            }
            *self.journal.lock().unwrap() = journal.clone();
            Ok(())
        }
    }

    fn info(m: &MockPlatform, t: &Target) -> ProcessInfo {
        m.processes()
            .unwrap()
            .into_iter()
            .find(|p| &p.target == t)
            .unwrap()
    }

    fn item(action: Action, subject: Subject) -> PlanItem {
        PlanItem {
            rule_id: format!("{action:?}"),
            category: Category::Updater,
            subject,
            action,
            risk: Risk::Low,
            min_level: Level::Normal,
            description: String::new(),
            requires_admin: false,
            locked: false,
            default_checked: true,
        }
    }

    fn procs(m: &MockPlatform, ts: &[&Target]) -> Subject {
        Subject::Processes(ts.iter().map(|t| info(m, t)).collect())
    }

    fn request(items: &[PlanItem]) -> ApplyRequest<'_> {
        ApplyRequest {
            items: items.iter().collect(),
            game: None,
        }
    }

    fn boost(target: &Target, priority: Priority, confirmed: bool) -> ApplyRequest<'static> {
        ApplyRequest {
            items: vec![],
            game: Some(GameBoost {
                target: target.clone(),
                priority,
                realtime_confirmed: confirmed,
            }),
        }
    }

    /// Estado observable del mock: prioridad y congelado por proceso, y estado de servicios.
    fn state(m: &MockPlatform, ts: &[&Target], services: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = ts
            .iter()
            .map(|t| {
                let p = m.priority(t).unwrap();
                let s = m.is_suspended(t).unwrap();
                format!("{} {p:?} {s}", t.pid)
            })
            .collect();
        for s in services {
            v.push(format!("{s} {:?}", m.service_state(s).unwrap()));
        }
        v
    }

    #[test]
    fn apply_then_revert_restores_the_initial_state() {
        let m = MockPlatform::new(8);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let b = m.spawn(2, "b.exe", "/b", 1);
        let c = m.spawn(3, "c.exe", "/c", 1);
        let game = m.spawn(9, "game.exe", "/game", 1);
        m.add_service("WSearch", ServiceState::Running);
        m.add_service("SysMain", ServiceState::Running);
        let all = [&a, &b, &c, &game];
        let svcs = ["WSearch", "SysMain"];
        let initial = state(&m, &all, &svcs);

        let items = [
            item(Action::LowerPriority, procs(&m, &[&a, &b])),
            item(Action::Suspend, procs(&m, &[&b, &c])),
            item(
                Action::StopService,
                Subject::Services(svcs.iter().map(|s| (*s).to_owned()).collect()),
            ),
        ];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let mut req = request(&items);
        req.game = boost(&game, Priority::High, false).game;
        let report = engine.apply(&req).unwrap();
        assert_eq!(report.done(), 7, "{report:?}");
        assert!(m.is_suspended(&c).unwrap());
        assert_eq!(m.priority(&game).unwrap(), Priority::High);
        assert_ne!(state(&m, &all, &svcs), initial);

        let report = engine.revert_all().unwrap();
        assert_eq!(report.failed(), 0);
        assert_eq!(state(&m, &all, &svcs), initial);
        assert!(!engine.has_open_session().unwrap());
    }

    #[test]
    fn nothing_runs_if_the_journal_cannot_be_written() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let items = [item(Action::Suspend, procs(&m, &[&a]))];
        let store = MemoryStore {
            fail_saves: true,
            ..MemoryStore::default()
        };
        assert!(Engine::new(&m, &store).apply(&request(&items)).is_err());
        assert!(!m.is_suspended(&a).unwrap());
    }

    #[test]
    fn a_new_engine_restores_a_session_left_open() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let items = [item(Action::Suspend, procs(&m, &[&a]))];
        let store = MemoryStore::default();
        Engine::new(&m, &store).apply(&request(&items)).unwrap();
        // "Reinicio" de la app: otro motor sobre el mismo journal.
        let after_crash = Engine::new(&m, &store);
        assert!(after_crash.has_open_session().unwrap());
        after_crash.revert_all().unwrap();
        assert!(!m.is_suspended(&a).unwrap());
    }

    #[test]
    fn a_reused_pid_is_never_touched_on_revert() {
        let m = MockPlatform::new(4);
        let old = m.spawn(5, "old.exe", "/old", 1);
        let items = [item(Action::LowerPriority, procs(&m, &[&old]))];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        engine.apply(&request(&items)).unwrap();
        let new = m.spawn(5, "new.exe", "/new", 1);
        m.set_priority(&new, Priority::AboveNormal).unwrap();
        let report = engine.revert_all().unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Skipped(_)));
        assert_eq!(m.priority(&new).unwrap(), Priority::AboveNormal);
        assert!(!engine.has_open_session().unwrap());
    }

    #[test]
    fn the_original_state_is_backed_up_only_once() {
        let m = MockPlatform::new(4);
        let game = m.spawn(1, "game.exe", "/game", 1);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        engine.apply(&boost(&game, Priority::High, false)).unwrap();
        let again = engine.apply(&boost(&game, Priority::High, false)).unwrap();
        assert!(matches!(again.lines[0].outcome, Outcome::Skipped(_)));
        engine
            .apply(&boost(&game, Priority::AboveNormal, false))
            .unwrap();
        engine.revert_all().unwrap();
        assert_eq!(m.priority(&game).unwrap(), Priority::Normal);
    }

    #[test]
    fn undo_last_only_reverts_the_last_batch() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let b = m.spawn(2, "b.exe", "/b", 1);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        for t in [&a, &b] {
            let items = [item(Action::Suspend, procs(&m, &[t]))];
            engine.apply(&request(&items)).unwrap();
        }
        engine.undo_last().unwrap();
        assert!(m.is_suspended(&a).unwrap());
        assert!(!m.is_suspended(&b).unwrap());
        assert!(engine.has_open_session().unwrap());
    }

    #[test]
    fn kill_is_recorded_but_not_reverted() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let items = [item(Action::Kill, procs(&m, &[&a]))];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        engine.apply(&request(&items)).unwrap();
        assert!(!m.is_same_process(&a).unwrap());
        let report = engine.revert_all().unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Skipped(_)));
    }

    #[test]
    fn realtime_needs_explicit_confirmation() {
        let m = MockPlatform::new(4);
        let game = m.spawn(1, "game.exe", "/game", 1);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        assert!(matches!(
            engine.apply(&boost(&game, Priority::Realtime, false)),
            Err(EngineError::RealtimeNotConfirmed)
        ));
        assert_eq!(m.priority(&game).unwrap(), Priority::Normal);
        engine
            .apply(&boost(&game, Priority::Realtime, true))
            .unwrap();
        assert_eq!(m.priority(&game).unwrap(), Priority::Realtime);
    }

    #[test]
    fn protected_processes_are_skipped_even_if_planned() {
        let m = MockPlatform::new(4);
        let csrss = m.spawn(1, "csrss.exe", "/csrss", 1);
        let items = [item(Action::Suspend, procs(&m, &[&csrss]))];
        let store = MemoryStore::default();
        let report = Engine::new(&m, &store).apply(&request(&items)).unwrap();
        assert!(report.lines.is_empty());
        assert!(!m.is_suspended(&csrss).unwrap());
    }
}
