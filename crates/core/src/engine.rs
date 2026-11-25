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
use crate::{Action, MemoryOp, PlanItem, Scope, Subject};
use framerush_platform::{
    DefenderExclusion, Platform, PlatformError, PowerPlan, Priority, Target, Tweak,
};
use std::path::Path;

/// Nombre del plan de energía propio (`custom-power-plan`, PLAN.md P3).
pub const CUSTOM_POWER_PLAN_NAME: &str = "FrameRush Max";

/// Prioridad que se pone al bajar la de un proceso de fondo (propuesta de diseño, PLAN.md §6).
pub const LOWERED_PRIORITY: Priority = Priority::BelowNormal;

/// `refresh-rate-max` para un monitor ([`crate::defaults::refresh_rate_max`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshRate {
    pub display: String,
    pub hz: u32,
}

/// Lo que se le hace al juego.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameBoost {
    /// Procesos del juego: reciben prioridad y afinidad. Los ajustes por exe (FSO, GPU, IFEO)
    /// se aplican una vez por exe distinto.
    pub targets: Vec<Target>,
    pub priority: Priority,
    /// Confirmación explícita para Tiempo real (It1.5).
    pub realtime_confirmed: bool,
    /// `fullscreen-optimizations-off` para el exe del juego (opt-in, persistente, V10).
    pub fullscreen_optimizations_off: bool,
    /// `gpu-preference = alto` para el exe del juego (persistente, P13, V22).
    pub high_performance_gpu: bool,
    /// `cpu-affinity` con las CPU de `cpu-affinity-cores` (opt-in, de sesión). Quien arma la
    /// petición la deja en `None` si hay anticheat (P2, [`crate::Plan::game_tuning_allowed`]).
    pub affinity: Option<Vec<usize>>,
    /// `game-priority-persistent`: Alta para el exe en cada arranque (IFEO). Se ignora si
    /// `priority` es Normal (P1); ver [`crate::persistent_priority_default`].
    pub persistent_priority: bool,
}

/// Archivo que el adaptador del juego quiere dejar así (`crates/games`, It5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameFileWrite {
    pub path: std::path::PathBuf,
    pub content: String,
    /// `game-config-lock`: de solo lectura para que el juego no lo revierta.
    pub read_only: bool,
}

/// Qué aplicar: las filas que el usuario dejó marcadas, los ajustes persistentes marcados y,
/// opcionalmente, la prioridad del juego.
#[derive(Debug, Clone, Default)]
pub struct ApplyRequest<'a> {
    pub items: Vec<&'a PlanItem>,
    pub tweaks: Vec<Tweak>,
    pub game: Option<GameBoost>,
    /// Se hacen antes que el resto, como en Turbo (PLAN.md §5, P9).
    pub memory: Vec<MemoryOp>,
    /// Plan de energía que se activa durante la sesión (R5; [`crate::power_plan_for`]).
    pub power_plan: Option<PowerPlan>,
    /// `custom-power-plan` (P3): plan base que se duplica como [`CUSTOM_POWER_PLAN_NAME`] y se
    /// activa ese duplicado ([`crate::defaults::custom_power_plan_base`]). Si está, sustituye a
    /// `power_plan`.
    pub custom_power_plan: Option<PowerPlan>,
    /// `cstates-off`: el plan propio se crea con los C-states desactivados (solo CA). Sin
    /// `custom_power_plan` no hace nada.
    pub cstates_off: bool,
    /// Frecuencias que se ponen durante la sesión, una por monitor.
    pub refresh_rates: Vec<RefreshRate>,
    /// Config del juego que escribe su adaptador (persistente).
    pub game_files: Vec<GameFileWrite>,
    /// `defender-exclusions` (It5.9, persistente): carpeta y exe del juego. Una que ya estaba
    /// no se toca, ni al aplicar ni al restaurar.
    pub defender_exclusions: Vec<DefenderExclusion>,
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
        Change::Affinity {
            target, applied, ..
        } => format!("afinidad a {} CPU → {}", applied.len(), proc(target)),
        Change::Suspend { target } => format!("suspender {}", proc(target)),
        Change::Kill { target } => format!("cerrar {}", proc(target)),
        Change::StopService { name } => format!("detener el servicio {name}"),
        Change::Tweak { tweak, .. } if crate::tweak_requires_reboot(*tweak) => {
            format!("ajuste {} (surte efecto al reiniciar)", tweak.id())
        }
        Change::Tweak { tweak, .. } => format!("ajuste {}", tweak.id()),
        Change::FullscreenOptimizationsOff { exe } => {
            format!("sin optimizaciones de pantalla completa: {}", exe.display())
        }
        Change::PersistentPriority { exe, .. } => {
            format!("prioridad Alta persistente: {}", exe.display())
        }
        Change::HighPerformanceGpu { exe } => {
            format!("GPU de alto rendimiento: {}", exe.display())
        }
        Change::GameFile { path, .. } => format!("config del juego: {}", path.display()),
        Change::DefenderExclusion { exclusion } => match exclusion {
            DefenderExclusion::Path(p) => format!("exclusión de Defender: carpeta {}", p.display()),
            DefenderExclusion::Process(p) => {
                format!("exclusión de Defender: proceso {}", p.display())
            }
        },
        Change::PowerPlan { applied, .. } => format!("plan de energía «{}»", applied.name),
        Change::RefreshRate {
            display, applied, ..
        } => format!("refresco de {display} a {applied} Hz"),
        Change::CustomPowerPlan {
            base, cpu_idle_off, ..
        } => {
            let idle = if *cpu_idle_off {
                ", C-states desactivados"
            } else {
                ""
            };
            format!(
                "plan de energía «{CUSTOM_POWER_PLAN_NAME}» (sobre «{}»{idle})",
                base.name
            )
        }
    }
}

/// Una línea de la vista Estado (PLAN.md It5.6): un cambio de la sesión, con su alcance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StatusLine {
    pub batch: u64,
    pub what: String,
    pub scope: Scope,
    pub status: EntryStatus,
}

/// Todo lo que la sesión tiene aplicado o pendiente, en el orden en que se aplicó. Los cambios
/// fallidos también salen: el journal los conserva para que se vean.
pub fn status_lines(journal: &Journal) -> Vec<StatusLine> {
    let mut entries: Vec<&Entry> = journal.entries.iter().collect();
    entries.sort_by_key(|e| e.seq);
    entries
        .into_iter()
        .map(|e| StatusLine {
            batch: e.batch,
            what: describe(&e.change),
            scope: e.change.scope(),
            status: e.status,
        })
        .collect()
}

/// Un paso terminado de `apply` o de deshacer, para mostrar el progreso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress<'s> {
    /// Pasos terminados hasta ahora, este incluido.
    pub done: usize,
    /// Total de pasos si se sabe de antemano (al deshacer, lo que hay en el journal).
    pub total: Option<usize>,
    /// Lo que se hizo, como en el informe ([`describe`]).
    pub what: &'s str,
}

/// Recibe cada [`Progress`].
pub type OnProgress<'a> = &'a dyn Fn(Progress<'_>);

/// Motor sobre una plataforma y un journal.
pub struct Engine<'a, P, S> {
    platform: &'a P,
    store: &'a S,
    progress: Option<OnProgress<'a>>,
}

impl<P: std::fmt::Debug, S: std::fmt::Debug> std::fmt::Debug for Engine<'_, P, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("platform", &self.platform)
            .field("store", &self.store)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

impl<'a, P: Platform, S: JournalStore> Engine<'a, P, S> {
    pub fn new(platform: &'a P, store: &'a S) -> Self {
        Self {
            platform,
            store,
            progress: None,
        }
    }

    /// Avisa a `on` de cada paso terminado.
    #[must_use]
    pub fn with_progress(mut self, on: OnProgress<'a>) -> Self {
        self.progress = Some(on);
        self
    }

    /// Anota el resultado en el informe y avisa del progreso.
    fn note(&self, report: &mut Report, what: String, outcome: Outcome) {
        self.note_of(report, what, outcome, None);
    }

    fn note_of(&self, report: &mut Report, what: String, outcome: Outcome, total: Option<usize>) {
        report.push(what, outcome);
        if let (Some(on), Some(line)) = (self.progress, report.lines.last()) {
            on(Progress {
                done: report.lines.len(),
                total,
                what: &line.what,
            });
        }
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

        for op in &request.memory {
            self.free_memory(*op, request.game.as_ref(), &mut report);
        }
        if let Some(base) = &request.custom_power_plan {
            self.set_custom_power_plan(
                &mut journal,
                batch,
                base,
                request.cstates_off,
                &mut report,
            )?;
        } else if let Some(plan) = &request.power_plan {
            self.set_power_plan(&mut journal, batch, plan, &mut report)?;
        }
        for rate in &request.refresh_rates {
            self.set_refresh_rate(&mut journal, batch, rate, &mut report)?;
        }
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
        for tweak in &request.tweaks {
            self.apply_tweak(&mut journal, batch, *tweak, &mut report)?;
        }
        if let Some(g) = &request.game {
            // Un protegido (p. ej. el anticheat en la carpeta del juego) nunca se toca.
            let targets = g
                .targets
                .iter()
                .filter(|t| !is_protected(&t.exe_path.to_string_lossy()));
            for target in targets {
                self.set_priority(&mut journal, batch, target, g.priority, &mut report)?;
                if let Some(cpus) = &g.affinity {
                    self.set_affinity(&mut journal, batch, target, cpus, &mut report)?;
                }
            }
            let mut exes: Vec<&Path> = g
                .targets
                .iter()
                .map(|t| t.exe_path.as_path())
                .filter(|exe| !is_protected(&exe.to_string_lossy()))
                .collect();
            exes.sort();
            exes.dedup();
            for exe in exes {
                self.apply_game_exe(&mut journal, batch, g, exe, &mut report)?;
            }
        }
        for file in &request.game_files {
            self.write_game_file(&mut journal, batch, file, &mut report)?;
        }
        for exclusion in &request.defender_exclusions {
            self.exclude_from_defender(&mut journal, batch, exclusion, &mut report)?;
        }
        Ok(report)
    }

    /// Exclusión de Defender solo si no estaba: una del usuario no entra en el journal, así que
    /// Restaurar no la quita.
    fn exclude_from_defender(
        &self,
        journal: &mut Journal,
        batch: u64,
        exclusion: &DefenderExclusion,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let change = Change::DefenderExclusion {
            exclusion: exclusion.clone(),
        };
        if !already_applied(journal, &change) {
            match self.platform.has_defender_exclusion(exclusion) {
                Ok(false) => {}
                Ok(true) => {
                    let why = "ya estaba en Defender (no la puso FrameRush)".to_owned();
                    self.note(report, describe(&change), Outcome::Skipped(why));
                    return Ok(());
                }
                Err(e) => {
                    self.note(report, describe(&change), Outcome::Failed(e));
                    return Ok(());
                }
            }
        }
        self.run(journal, batch, change, report)
    }

    /// Un archivo del juego con backup único: si ya se escribió en esta sesión, el original es
    /// el de la primera vez, no el que dejamos.
    fn write_game_file(
        &self,
        journal: &mut Journal,
        batch: u64,
        file: &GameFileWrite,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let original = journal.entries.iter().find_map(|e| match &e.change {
            Change::GameFile {
                path,
                previous,
                was_read_only,
                ..
            } if *path == file.path && e.status == EntryStatus::Done => {
                Some((previous.clone(), *was_read_only))
            }
            _ => None,
        });
        let (previous, was_read_only) = match original {
            Some(o) => o,
            None => match self.platform.read_game_file(&file.path) {
                Ok(Some((text, read_only))) => (Some(text), read_only),
                Ok(None) => (None, false),
                Err(e) => {
                    let what = format!("leer {}", file.path.display());
                    self.note(report, what, Outcome::Failed(e));
                    return Ok(());
                }
            },
        };
        let change = Change::GameFile {
            path: file.path.clone(),
            previous,
            was_read_only,
            content: file.content.clone(),
            read_only: file.read_only,
        };
        self.run(journal, batch, change, report)
    }

    /// Ajustes persistentes de un exe del juego.
    fn apply_game_exe(
        &self,
        journal: &mut Journal,
        batch: u64,
        g: &GameBoost,
        exe: &Path,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        if g.fullscreen_optimizations_off {
            let change = Change::FullscreenOptimizationsOff {
                exe: exe.to_path_buf(),
            };
            let current = || self.platform.compat_flags(exe);
            self.apply_if_unset(journal, batch, change, current, report)?;
        }
        if g.high_performance_gpu {
            let change = Change::HighPerformanceGpu {
                exe: exe.to_path_buf(),
            };
            let current = || self.platform.gpu_preference(exe);
            self.apply_if_unset(journal, batch, change, current, report)?;
        }
        if g.persistent_priority && g.priority != Priority::Normal {
            self.persist_priority(journal, batch, exe, report)?;
        }
        Ok(())
    }

    /// Una operación de memoria. No hay nada que revertir, así que no pasa por el journal, y un
    /// fallo queda en el informe sin parar el resto.
    fn free_memory(&self, op: MemoryOp, game: Option<&GameBoost>, report: &mut Report) {
        let outcome = match op {
            MemoryOp::StandbyPurge => match self.platform.purge_standby_list() {
                Ok(()) => Outcome::Done,
                Err(e) => Outcome::Failed(e),
            },
            MemoryOp::Trim => match self.platform.processes() {
                Err(e) => Outcome::Failed(e),
                Ok(processes) => {
                    // De fondo: ni el juego, ni FrameRush y sus hijos, ni la lista de protegidos.
                    let background = processes.iter().filter(|p| {
                        !is_protected(&p.name)
                            && !p.framerush
                            && p.target.pid != std::process::id()
                            && game.is_none_or(|g| !g.targets.contains(&p.target))
                    });
                    let (mut done, mut failed) = (0usize, 0usize);
                    for p in background {
                        match self.platform.trim_working_set(&p.target) {
                            Ok(()) => done += 1,
                            Err(_) => failed += 1,
                        }
                    }
                    self.note(
                        report,
                        format!("recorte de working set: {done} procesos ({failed} sin acceso)"),
                        Outcome::Done,
                    );
                    return;
                }
            },
        };
        self.note(report, op.id().to_owned(), outcome);
    }

    /// Cambio de un valor por exe (FSO, GPU) que solo se hace si el exe no tiene ya uno: no hay
    /// una definición documentada de cómo combinarlos y sobrescribirlo perdería el del usuario.
    fn apply_if_unset(
        &self,
        journal: &mut Journal,
        batch: u64,
        change: Change,
        current: impl FnOnce() -> Result<Option<String>, PlatformError>,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        if !already_applied(journal, &change) {
            match current() {
                Ok(None) => {}
                Ok(Some(value)) => {
                    let why = format!("el exe ya tiene un valor propio (`{value}`)");
                    self.note(report, describe(&change), Outcome::Skipped(why));
                    return Ok(());
                }
                Err(e) => {
                    self.note(report, describe(&change), Outcome::Failed(e));
                    return Ok(());
                }
            }
        }
        self.run(journal, batch, change, report)
    }

    /// IFEO del exe del juego, con backup único: si ya está en el journal, no se vuelve a leer.
    fn persist_priority(
        &self,
        journal: &mut Journal,
        batch: u64,
        exe: &Path,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let mut change = Change::PersistentPriority {
            exe: exe.to_path_buf(),
            previous: vec![],
        };
        if !already_applied(journal, &change) {
            match self.platform.read_persistent_priority(exe) {
                Ok(previous) => {
                    change = Change::PersistentPriority {
                        exe: exe.to_path_buf(),
                        previous,
                    };
                }
                Err(e) => {
                    self.note(report, describe(&change), Outcome::Failed(e));
                    return Ok(());
                }
            }
        }
        self.run(journal, batch, change, report)
    }

    /// Deshace toda la sesión. Lo que no se pudo deshacer queda en el journal.
    pub fn revert_all(&self) -> Result<Report, EngineError> {
        self.revert_where(|_| true)
    }

    /// Deshace los cambios de sesión y deja los persistentes (watchdog al cerrar el juego,
    /// `auto-revert-on-exit`). Restaurar es [`Engine::revert_all`].
    pub fn revert_session(&self) -> Result<Report, EngineError> {
        self.revert_where(|e| e.change.scope() == Scope::Session)
    }

    /// Deshace solo los cambios de los procesos de `targets` (reanudar una fila suelta) y deja
    /// el resto de la sesión como está. Volver a limitarlos es otro `apply` con esa fila.
    pub fn revert_processes(&self, targets: &[Target]) -> Result<Report, EngineError> {
        self.revert_where(|e| e.change.target().is_some_and(|t| targets.contains(t)))
    }

    /// Reanuda todo lo congelado y deja el resto de la sesión (Turbo al cerrar el juego sin
    /// `auto-revert-on-exit`, P5: los congelados siempre se reanudan).
    pub fn resume_frozen(&self) -> Result<Report, EngineError> {
        self.revert_where(|e| matches!(e.change, Change::Suspend { .. }))
    }

    /// Reanuda procesos que congeló otro programa ([`crate::foreign_frozen`]). No pasa por el
    /// journal: no es un cambio de FrameRush y no hay nada que deshacer. Lo que sí congeló
    /// FrameRush se salta (se reanuda al deshacer su sesión).
    pub fn resume_foreign(&self, targets: &[Target]) -> Result<Report, EngineError> {
        let journal = self.store.load()?;
        let mut report = Report::default();
        for target in targets {
            let what = format!(
                "reanudar {} (pid {})",
                target.exe_path.display(),
                target.pid
            );
            let ours = journal
                .entries
                .iter()
                .any(|e| matches!(&e.change, Change::Suspend { target: t } if t == target));
            let outcome = if ours {
                Outcome::Skipped("lo congeló FrameRush: se reanuda al deshacer la sesión".into())
            } else {
                match self.platform.is_same_process(target) {
                    Ok(false) => Outcome::Skipped(
                        "el proceso ya no existe o su PID es de otro proceso".into(),
                    ),
                    Ok(true) => match self.platform.resume(target) {
                        Ok(()) => Outcome::Done,
                        Err(e) => Outcome::Failed(e),
                    },
                    Err(e) => Outcome::Failed(e),
                }
            };
            self.note(&mut report, what, outcome);
        }
        Ok(report)
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
                    self.note(report, what, Outcome::Failed(e));
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

    /// Plan de energía con backup único: el original es el que estaba activo la primera vez.
    fn set_power_plan(
        &self,
        journal: &mut Journal,
        batch: u64,
        applied: &PowerPlan,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let previous = match original_power_plan(journal) {
            Some(p) => p,
            None => match self.platform.active_power_plan() {
                Ok(active) if active.id == applied.id => {
                    let what = format!("plan de energía «{}»", applied.name);
                    self.note(
                        report,
                        what,
                        Outcome::Skipped("ya es el plan activo".into()),
                    );
                    return Ok(());
                }
                Ok(active) => active,
                Err(e) => {
                    self.note(
                        report,
                        "leer el plan de energía activo".into(),
                        Outcome::Failed(e),
                    );
                    return Ok(());
                }
            },
        };
        let change = Change::PowerPlan {
            previous,
            applied: applied.clone(),
        };
        self.run(journal, batch, change, report)
    }

    /// Plan propio (P3): la entrada se guarda antes de crear el duplicado y se completa con su
    /// id en cuanto existe, para que un cierre a mitad no deje un plan sin borrar.
    fn set_custom_power_plan(
        &self,
        journal: &mut Journal,
        batch: u64,
        base: &PowerPlan,
        cpu_idle_off: bool,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let change = Change::CustomPowerPlan {
            previous: base.clone(),
            base: base.clone(),
            created: None,
            cpu_idle_off,
        };
        let what = describe(&change);
        if already_applied(journal, &change) {
            self.note(
                report,
                what,
                Outcome::Skipped("ya aplicado en esta sesión".into()),
            );
            return Ok(());
        }
        let previous = match original_power_plan(journal) {
            Some(p) => p,
            None => match self.platform.active_power_plan() {
                Ok(active) => active,
                Err(e) => {
                    self.note(
                        report,
                        "leer el plan de energía activo".into(),
                        Outcome::Failed(e),
                    );
                    return Ok(());
                }
            },
        };
        let index = journal.entries.len();
        journal.entries.push(Entry {
            seq: journal.next_seq(),
            batch,
            change: Change::CustomPowerPlan {
                previous,
                base: base.clone(),
                created: None,
                cpu_idle_off,
            },
            status: EntryStatus::Pending,
        });
        self.store.save(journal)?;

        let result = match self.platform.create_custom_power_plan(
            &base.id,
            CUSTOM_POWER_PLAN_NAME,
            cpu_idle_off,
        ) {
            Err(e) => Err(e),
            Ok(plan) => {
                if let Change::CustomPowerPlan { created, .. } = &mut journal.entries[index].change
                {
                    *created = Some(plan.clone());
                }
                self.store.save(journal)?;
                self.platform
                    .set_active_power_plan(&plan.id)
                    .inspect_err(|_| {
                        // Sin activar no sirve: se borra. Si no se puede, la entrada queda
                        // `Pending` y `revert` lo borrará.
                        if self.platform.delete_power_plan(&plan.id).is_ok()
                            && let Change::CustomPowerPlan { created, .. } =
                                &mut journal.entries[index].change
                        {
                            *created = None;
                        }
                    })
            }
        };
        let deleted = matches!(
            &journal.entries[index].change,
            Change::CustomPowerPlan { created: None, .. }
        );
        journal.entries[index].status = match &result {
            Ok(()) => EntryStatus::Done,
            Err(_) if deleted => EntryStatus::Failed,
            Err(_) => EntryStatus::Pending,
        };
        self.store.save(journal)?;
        self.note(
            report,
            what,
            result.map_or_else(Outcome::Failed, |()| Outcome::Done),
        );
        Ok(())
    }

    /// Frecuencia de un monitor con backup único: la original es la de la primera vez.
    fn set_refresh_rate(
        &self,
        journal: &mut Journal,
        batch: u64,
        rate: &RefreshRate,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let original = journal
            .entries
            .iter()
            .filter(|e| e.status != EntryStatus::Failed)
            .find_map(|e| match &e.change {
                Change::RefreshRate {
                    display, previous, ..
                } if *display == rate.display => Some(*previous),
                _ => None,
            });
        let previous = match original {
            Some(hz) => hz,
            None => {
                let current = self.platform.displays().map(|ds| {
                    ds.into_iter()
                        .find(|d| d.id == rate.display)
                        .map(|d| d.current.hz)
                });
                match current {
                    Ok(Some(hz)) if hz == rate.hz => {
                        let what = format!("refresco de {} a {} Hz", rate.display, rate.hz);
                        self.note(
                            report,
                            what,
                            Outcome::Skipped("ya está a esa frecuencia".into()),
                        );
                        return Ok(());
                    }
                    Ok(Some(hz)) => hz,
                    Ok(None) => {
                        let e = PlatformError::NotFound(format!("pantalla {}", rate.display));
                        self.note(report, format!("leer {}", rate.display), Outcome::Failed(e));
                        return Ok(());
                    }
                    Err(e) => {
                        self.note(report, format!("leer {}", rate.display), Outcome::Failed(e));
                        return Ok(());
                    }
                }
            }
        };
        let change = Change::RefreshRate {
            display: rate.display.clone(),
            previous,
            applied: rate.hz,
        };
        self.run(journal, batch, change, report)
    }

    /// Borra el plan propio de la entrada o, si no llegó a anotarse, los que tengan su nombre.
    /// Un plan que ya no existe no es un error.
    fn delete_custom_power_plan(&self, created: Option<&PowerPlan>) -> Result<(), PlatformError> {
        let plans = self.platform.power_plans()?;
        let stale = plans.iter().filter(|p| match created {
            Some(c) => p.id == c.id,
            None => p.name == CUSTOM_POWER_PLAN_NAME,
        });
        for plan in stale {
            self.platform.delete_power_plan(&plan.id)?;
        }
        Ok(())
    }

    /// Afinidad con backup único: la original es la de la primera vez en la sesión.
    fn set_affinity(
        &self,
        journal: &mut Journal,
        batch: u64,
        target: &Target,
        cpus: &[usize],
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let mut applied = cpus.to_vec();
        applied.sort_unstable();
        applied.dedup();
        let original = journal.entries.iter().find_map(|e| match &e.change {
            Change::Affinity {
                target: t,
                previous,
                ..
            } if t == target && e.status == EntryStatus::Done => Some(previous.clone()),
            _ => None,
        });
        let previous = match original {
            Some(p) => p,
            None => match self.platform.affinity(target) {
                Ok(p) => p,
                Err(e) => {
                    let what = format!("leer la afinidad de {}", target.exe_path.display());
                    self.note(report, what, Outcome::Failed(e));
                    return Ok(());
                }
            },
        };
        let change = Change::Affinity {
            target: target.clone(),
            previous,
            applied,
        };
        self.run(journal, batch, change, report)
    }

    /// Ajuste persistente con backup único: si ya está aplicado no se vuelve a leer el estado
    /// (sería el que pusimos nosotros).
    fn apply_tweak(
        &self,
        journal: &mut Journal,
        batch: u64,
        tweak: Tweak,
        report: &mut Report,
    ) -> Result<(), EngineError> {
        let applied = journal.entries.iter().any(|e| {
            e.status == EntryStatus::Done
                && matches!(&e.change, Change::Tweak { tweak: t, .. } if *t == tweak)
        });
        if applied {
            let what = format!("ajuste {}", tweak.id());
            self.note(
                report,
                what,
                Outcome::Skipped("ya aplicado en esta sesión".into()),
            );
            return Ok(());
        }
        let previous = match self.platform.read_tweak(tweak) {
            Ok(p) => p,
            Err(e) => {
                self.note(
                    report,
                    format!("leer el ajuste {}", tweak.id()),
                    Outcome::Failed(e),
                );
                return Ok(());
            }
        };
        self.run(journal, batch, Change::Tweak { tweak, previous }, report)
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
            self.note(
                report,
                what,
                Outcome::Skipped("ya aplicado en esta sesión".into()),
            );
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
            Change::Affinity {
                target, applied, ..
            } => self.platform.set_affinity(target, applied),
            Change::Suspend { target } => self.platform.suspend(target),
            Change::Kill { target } => self.platform.kill(target),
            Change::StopService { name } => self.platform.stop_service(name),
            Change::Tweak { tweak, .. } => self.platform.apply_tweak(*tweak),
            Change::FullscreenOptimizationsOff { exe } => {
                self.platform.disable_fullscreen_optimizations(exe)
            }
            Change::PersistentPriority { exe, .. } => self.platform.set_persistent_priority(exe),
            Change::HighPerformanceGpu { exe } => self.platform.prefer_high_performance_gpu(exe),
            Change::DefenderExclusion { exclusion } => {
                self.platform.add_defender_exclusion(exclusion)
            }
            Change::PowerPlan { applied, .. } => self.platform.set_active_power_plan(&applied.id),
            Change::RefreshRate {
                display, applied, ..
            } => self.platform.set_refresh_rate(display, *applied),
            Change::GameFile {
                path,
                content,
                read_only,
                ..
            } => self.platform.write_game_file(path, content, *read_only),
            // Lo crea `set_custom_power_plan`; aquí solo se puede activar uno ya creado.
            Change::CustomPowerPlan { created, .. } => match created {
                Some(plan) => self.platform.set_active_power_plan(&plan.id),
                None => Err(PlatformError::NotFound("plan de energía propio".into())),
            },
        };
        journal.entries[index].status = match result {
            Ok(()) => EntryStatus::Done,
            Err(_) => EntryStatus::Failed,
        };
        self.store.save(journal)?;
        self.note(
            report,
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
        let total = Some(selected.len());
        for entry in selected {
            let outcome = self.undo(&entry.change);
            if matches!(outcome, Outcome::Failed(_)) {
                keep.push(entry.seq);
            }
            self.note_of(&mut report, describe(&entry.change), outcome, total);
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
            Change::Affinity {
                target, previous, ..
            } => self.platform.set_affinity(target, previous),
            Change::Suspend { target } => self.platform.resume(target),
            Change::StopService { name } => self.platform.start_service(name),
            Change::Tweak { tweak, previous } => self.platform.restore_tweak(*tweak, previous),
            Change::FullscreenOptimizationsOff { exe } => self.platform.clear_compat_flags(exe),
            Change::GameFile {
                path,
                previous,
                was_read_only,
                ..
            } => match previous {
                Some(text) => self.platform.write_game_file(path, text, *was_read_only),
                None => self.platform.remove_game_file(path),
            },
            Change::PersistentPriority { exe, previous } => {
                self.platform.restore_persistent_priority(exe, previous)
            }
            Change::HighPerformanceGpu { exe } => self.platform.clear_gpu_preference(exe),
            Change::DefenderExclusion { exclusion } => {
                self.platform.remove_defender_exclusion(exclusion)
            }
            Change::PowerPlan { previous, .. } => self.platform.set_active_power_plan(&previous.id),
            Change::RefreshRate {
                display, previous, ..
            } => self.platform.set_refresh_rate(display, *previous),
            Change::CustomPowerPlan {
                previous, created, ..
            } => self
                .platform
                .set_active_power_plan(&previous.id)
                .and_then(|()| self.delete_custom_power_plan(created.as_ref())),
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
            (
                Change::Affinity {
                    target: a,
                    applied: x,
                    ..
                },
                Change::Affinity {
                    target: b,
                    applied: y,
                    ..
                },
            ) => a == b && x == y,
            (Change::Suspend { target: a }, Change::Suspend { target: b }) => a == b,
            (Change::StopService { name: a }, Change::StopService { name: b }) => a == b,
            (
                Change::FullscreenOptimizationsOff { exe: a },
                Change::FullscreenOptimizationsOff { exe: b },
            ) => a == b,
            (
                Change::PersistentPriority { exe: a, .. },
                Change::PersistentPriority { exe: b, .. },
            ) => a == b,
            (Change::HighPerformanceGpu { exe: a }, Change::HighPerformanceGpu { exe: b }) => {
                a == b
            }
            (
                Change::DefenderExclusion { exclusion: a },
                Change::DefenderExclusion { exclusion: b },
            ) => a == b,
            (Change::PowerPlan { applied: a, .. }, Change::PowerPlan { applied: b, .. }) => {
                a.id == b.id
            }
            (
                Change::RefreshRate {
                    display: a,
                    applied: x,
                    ..
                },
                Change::RefreshRate {
                    display: b,
                    applied: y,
                    ..
                },
            ) => a == b && x == y,
            // Hay un solo plan propio por sesión.
            (Change::CustomPowerPlan { .. }, Change::CustomPowerPlan { .. }) => true,
            (
                Change::GameFile {
                    path: a,
                    content: x,
                    read_only: r,
                    ..
                },
                Change::GameFile {
                    path: b,
                    content: y,
                    read_only: s,
                    ..
                },
            ) => a == b && x == y && r == s,
            _ => false,
        })
}

/// Plan que estaba activo antes de que la sesión tocara la energía (backup único), si ya la
/// tocó.
fn original_power_plan(journal: &Journal) -> Option<PowerPlan> {
    journal
        .entries
        .iter()
        .filter(|e| e.status != EntryStatus::Failed)
        .find_map(|e| match &e.change {
            Change::PowerPlan { previous, .. } | Change::CustomPowerPlan { previous, .. } => {
                Some(previous.clone())
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, Level, Risk};
    use framerush_platform::mock::MockPlatform;
    use framerush_platform::{
        CapabilitySet, DisplayControl, GameTuning, PowerControl, ProcessControl, ProcessInfo,
        ServiceControl, ServiceState,
    };
    use std::path::PathBuf;
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
            memory: vec![],
            power_plan: None,
            custom_power_plan: None,
            cstates_off: false,
            refresh_rates: vec![],
            game_files: vec![],
            defender_exclusions: vec![],
            items: items.iter().collect(),
            tweaks: vec![],
            game: None,
        }
    }

    fn boost(target: &Target, priority: Priority, confirmed: bool) -> ApplyRequest<'static> {
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
                targets: vec![target.clone()],
                priority,
                realtime_confirmed: confirmed,
                fullscreen_optimizations_off: false,
                affinity: None,
                high_performance_gpu: false,
                persistent_priority: false,
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

    /// La vista Estado lista la sesión en orden de aplicación, con alcance y estado.
    #[test]
    fn status_lines_list_the_session_in_order() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let game = m.spawn(9, "game.exe", "/game", 1);
        let items = [item(Action::LowerPriority, procs(&m, &[&a]))];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        assert!(status_lines(&store.load().unwrap()).is_empty());

        let mut req = request(&items);
        req.game = boost(&game, Priority::High, false).game;
        engine.apply(&req).unwrap();
        let lines = status_lines(&store.load().unwrap());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.status == EntryStatus::Done));
        assert!(lines.iter().all(|l| l.scope == Scope::Session));
        assert!(lines[0].what.ends_with("/a (pid 1)"), "{lines:?}");
        assert!(lines[1].what.ends_with("/game (pid 9)"), "{lines:?}");

        engine.revert_all().unwrap();
        assert!(status_lines(&store.load().unwrap()).is_empty());
    }

    /// Cada paso avisa al terminar; al deshacer se sabe el total de antemano.
    #[test]
    fn progress_reports_each_step() {
        use std::cell::RefCell;
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let game = m.spawn(9, "game.exe", "/game", 1);
        let items = [item(Action::LowerPriority, procs(&m, &[&a]))];
        let store = MemoryStore::default();
        let seen = RefCell::new(vec![]);
        let on = |p: Progress<'_>| seen.borrow_mut().push((p.done, p.total, p.what.to_owned()));
        let engine = Engine::new(&m, &store).with_progress(&on);

        let mut req = request(&items);
        req.game = boost(&game, Priority::High, false).game;
        let report = engine.apply(&req).unwrap();
        let applied: Vec<_> = seen.borrow_mut().drain(..).collect();
        assert_eq!(applied.len(), report.lines.len());
        assert_eq!(
            applied.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(),
            [(1, None), (2, None)]
        );
        assert_eq!(applied[1].2, report.lines[1].what);

        engine.revert_all().unwrap();
        let reverted: Vec<_> = seen.borrow().iter().map(|s| (s.0, s.1)).collect();
        assert_eq!(reverted, [(1, Some(2)), (2, Some(2))]);
    }

    /// Memoria: se recortan los procesos de fondo (ni el juego ni los protegidos), se purga
    /// la standby y el journal no cambia.
    #[test]
    fn memory_ops_trim_the_background_and_leave_no_journal() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let csrss = m.spawn(2, "csrss.exe", "/csrss", 1);
        let game = m.spawn(9, "game.exe", "/game", 1);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let mut req = request(&[]);
        req.memory = MemoryOp::ALL.to_vec();
        req.game = boost(&game, Priority::Normal, false).game;
        let report = engine.apply(&req).unwrap();
        assert_eq!(report.failed(), 0, "{report:?}");
        assert_eq!(m.trims(&a).unwrap(), 1);
        assert_eq!(m.trims(&csrss).unwrap(), 0, "protegido");
        assert_eq!(m.trims(&game).unwrap(), 0, "el juego");
        assert_eq!(m.standby_purges(), 1);
        assert!(
            store
                .load()
                .unwrap()
                .entries
                .iter()
                .all(|e| matches!(e.change, Change::Priority { .. })),
            "solo la prioridad del juego"
        );
    }

    /// Todos los procesos del juego suben de prioridad salvo los protegidos (su anticheat); los
    /// ajustes por exe van una vez por exe distinto.
    #[test]
    fn every_game_process_is_boosted_and_each_exe_once() {
        let m = MockPlatform::new(4);
        let main = m.spawn(9, "game.exe", "/game/game.exe", 1);
        let second = m.spawn(10, "game.exe", "/game/game.exe", 1);
        let helper = m.spawn(11, "helper.exe", "/game/helper.exe", 1);
        let anticheat = m.spawn(12, "EasyAntiCheat.exe", "/game/EasyAntiCheat.exe", 1);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let mut req = boost(&main, Priority::High, false);
        if let Some(g) = req.game.as_mut() {
            g.targets = vec![
                main.clone(),
                second.clone(),
                helper.clone(),
                anticheat.clone(),
            ];
            g.fullscreen_optimizations_off = true;
        }
        let report = engine.apply(&req).unwrap();
        assert_eq!(report.failed(), 0, "{report:?}");
        for t in [&main, &second, &helper] {
            assert_eq!(m.priority(t).unwrap(), Priority::High);
        }
        assert_eq!(
            m.priority(&anticheat).unwrap(),
            Priority::Normal,
            "protegido"
        );
        let fso = store
            .load()
            .unwrap()
            .entries
            .iter()
            .filter(|e| matches!(e.change, Change::FullscreenOptimizationsOff { .. }))
            .count();
        assert_eq!(fso, 2, "game.exe y helper.exe");
    }

    fn files(writes: &[(&str, &str, bool)]) -> ApplyRequest<'static> {
        ApplyRequest {
            game_files: writes
                .iter()
                .map(|(path, content, read_only)| GameFileWrite {
                    path: PathBuf::from(path),
                    content: (*content).into(),
                    read_only: *read_only,
                })
                .collect(),
            ..ApplyRequest::default()
        }
    }

    /// La config del juego se escribe, reaplicar otra no pierde la original y Restaurar la
    /// devuelve con su atributo; un archivo que no existía se borra. Es persistente: el
    /// watchdog no la toca.
    /// `defender-exclusions`: se añade lo que falta y Restaurar lo quita; una exclusión que ya
    /// tenía el usuario ni se registra ni se quita. Es persistente: el watchdog no la toca.
    #[test]
    fn defender_exclusions_leave_the_users_own() {
        use framerush_platform::SecurityTweaks;
        let m = MockPlatform::new(4);
        let folder = DefenderExclusion::Path("/games/cs2".into());
        let exe = DefenderExclusion::Process("/games/cs2/cs2.exe".into());
        m.add_defender_exclusion(&folder).unwrap();
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let req = ApplyRequest {
            defender_exclusions: vec![folder.clone(), exe.clone()],
            ..request(&[])
        };
        let report = engine.apply(&req).unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Skipped(_)));
        assert_eq!(report.lines[1].outcome, Outcome::Done);
        assert!(m.has_defender_exclusion(&exe).unwrap());

        engine.revert_session().unwrap();
        assert!(m.has_defender_exclusion(&exe).unwrap());
        assert_eq!(engine.revert_all().unwrap().failed(), 0);
        assert!(!m.has_defender_exclusion(&exe).unwrap());
        assert!(m.has_defender_exclusion(&folder).unwrap());
        assert!(!engine.has_open_session().unwrap());
    }

    #[test]
    fn game_files_round_trip_keeps_the_original() {
        let m = MockPlatform::new(4);
        m.put_game_file("/cfg/video.txt", "original", false);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let req = files(&[
            ("/cfg/video.txt", "tuned", true),
            ("/cfg/own.cfg", "fps_max 0", false),
        ]);
        assert_eq!(engine.apply(&req).unwrap().failed(), 0);
        assert_eq!(
            m.game_file(Path::new("/cfg/video.txt")),
            Some(("tuned".into(), true))
        );
        assert_eq!(
            m.game_file(Path::new("/cfg/own.cfg")),
            Some(("fps_max 0".into(), false))
        );

        // Igual: ya aplicado. Distinto: se escribe, pero el original sigue siendo el primero.
        let again = engine.apply(&req).unwrap();
        assert!(
            again
                .lines
                .iter()
                .all(|l| matches!(l.outcome, Outcome::Skipped(_)))
        );
        engine
            .apply(&files(&[("/cfg/video.txt", "retuned", false)]))
            .unwrap();
        assert_eq!(
            m.game_file(Path::new("/cfg/video.txt")),
            Some(("retuned".into(), false))
        );

        engine.revert_session().unwrap();
        assert_eq!(
            m.game_file(Path::new("/cfg/video.txt")),
            Some(("retuned".into(), false))
        );

        let report = engine.revert_all().unwrap();
        assert_eq!(report.failed(), 0, "{report:?}");
        assert_eq!(
            m.game_file(Path::new("/cfg/video.txt")),
            Some(("original".into(), false))
        );
        assert_eq!(m.game_file(Path::new("/cfg/own.cfg")), None);
    }

    /// Afinidad del juego: se aplica, reaplicar otra no pierde la original, y revertir la
    /// devuelve.
    #[test]
    fn affinity_round_trip_keeps_the_original() {
        let m = MockPlatform::new(8);
        let game = m.spawn(9, "game.exe", "/game", 1);
        let original = m.affinity(&game).unwrap();
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let mut req = boost(&game, Priority::Normal, false);
        if let Some(g) = req.game.as_mut() {
            g.affinity = Some(vec![5, 2, 3, 2]);
        }
        engine.apply(&req).unwrap();
        assert_eq!(m.affinity(&game).unwrap(), [2, 3, 5]);
        if let Some(g) = req.game.as_mut() {
            g.affinity = Some(vec![4]);
        }
        engine.apply(&req).unwrap();
        assert_eq!(m.affinity(&game).unwrap(), [4]);
        engine.revert_session().unwrap();
        assert_eq!(m.affinity(&game).unwrap(), original);
    }

    #[test]
    fn power_plan_round_trip_keeps_the_original() {
        let m = MockPlatform::new(4);
        let plan = |id: &str| PowerPlan {
            id: id.into(),
            name: format!("plan {id}"),
        };
        m.set_power_plans(vec![plan("a"), plan("b"), plan("c")], "a");
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let req = |id: &str| ApplyRequest {
            power_plan: Some(plan(id)),
            ..Default::default()
        };

        // El activo no se toca ni se registra.
        let report = engine.apply(&req("a")).unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Skipped(_)));
        assert!(store.load().unwrap().entries.is_empty());

        engine.apply(&req("b")).unwrap();
        engine.apply(&req("c")).unwrap();
        assert_eq!(m.active_power_plan().unwrap().id, "c");
        // Backup único: el original sigue siendo `a`, no `b`.
        let journal = store.load().unwrap();
        assert!(journal.entries.iter().all(|e| matches!(
            &e.change,
            Change::PowerPlan { previous, .. } if previous.id == "a"
        )));
        assert_eq!(journal.entries[0].change.scope(), Scope::Session);

        engine.revert_session().unwrap();
        assert_eq!(m.active_power_plan().unwrap().id, "a");
        assert!(store.load().unwrap().entries.is_empty());
    }

    fn plans(m: &MockPlatform) -> Vec<String> {
        m.power_plans().unwrap().into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn refresh_rate_round_trip_keeps_the_original() {
        let m = MockPlatform::new(4);
        let mode = |hz| framerush_platform::Mode {
            width: 1920,
            height: 1080,
            hz,
        };
        m.set_displays(vec![framerush_platform::Display {
            id: "d1".into(),
            current: mode(60),
            modes: vec![mode(60), mode(120), mode(144)],
            primary: true,
        }]);
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let req = |hz| ApplyRequest {
            refresh_rates: vec![RefreshRate {
                display: "d1".into(),
                hz,
            }],
            ..Default::default()
        };
        let hz = || m.displays().unwrap()[0].current.hz;

        // Ya está a 60 Hz: no se toca ni se registra.
        let report = engine.apply(&req(60)).unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Skipped(_)));
        assert!(store.load().unwrap().entries.is_empty());

        engine.apply(&req(120)).unwrap();
        engine.apply(&req(144)).unwrap();
        assert_eq!(hz(), 144);
        assert!(
            store
                .load()
                .unwrap()
                .entries
                .iter()
                .all(|e| matches!(e.change, Change::RefreshRate { previous: 60, .. }))
        );

        engine.revert_session().unwrap();
        assert_eq!(hz(), 60);
        assert!(store.load().unwrap().entries.is_empty());

        // Un modo que no existe falla y no deja nada que revertir.
        let report = engine.apply(&req(75)).unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Failed(_)));
        assert_eq!(hz(), 60);
    }

    #[test]
    fn custom_power_plan_round_trip() {
        let m = MockPlatform::new(4);
        let plan = |id: &str| PowerPlan {
            id: id.into(),
            name: format!("plan {id}"),
        };
        m.set_power_plans(vec![plan("a"), plan("b"), plan("c")], "a");
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let req = ApplyRequest {
            custom_power_plan: Some(plan("b")),
            // P3: el plan propio sustituye al del nivel.
            power_plan: Some(plan("c")),
            cstates_off: true,
            ..Default::default()
        };

        let report = engine.apply(&req).unwrap();
        assert!(report.lines[0].what.contains("C-states"), "{report:?}");
        let active = m.active_power_plan().unwrap();
        assert_eq!(active.name, CUSTOM_POWER_PLAN_NAME);
        assert!(m.cpu_idle_off(&active.id));
        let journal = store.load().unwrap();
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].status, EntryStatus::Done);
        assert!(matches!(
            &journal.entries[0].change,
            Change::CustomPowerPlan { previous, created: Some(c), .. }
                if previous.id == "a" && *c == active
        ));

        // Reaplicar no crea otro.
        let report = engine.apply(&req).unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Skipped(_)));
        assert_eq!(plans(&m).len(), 4);

        // Un `power set` posterior guarda como original el de antes de la sesión.
        engine
            .apply(&ApplyRequest {
                power_plan: Some(plan("c")),
                ..Default::default()
            })
            .unwrap();
        assert!(store.load().unwrap().entries.iter().all(|e| matches!(
            &e.change,
            Change::PowerPlan { previous, .. } | Change::CustomPowerPlan { previous, .. }
                if previous.id == "a"
        )));

        engine.revert_session().unwrap();
        assert_eq!(m.active_power_plan().unwrap().id, "a");
        assert_eq!(plans(&m), ["a", "b", "c"]);
        assert!(store.load().unwrap().entries.is_empty());
    }

    #[test]
    fn custom_power_plan_left_by_a_crash_is_deleted_on_revert() {
        let m = MockPlatform::new(4);
        let base = PowerPlan {
            id: "a".into(),
            name: "plan a".into(),
        };
        m.set_power_plans(vec![base.clone()], "a");
        // La app se cerró después de crear el plan y antes de anotar su id.
        m.create_custom_power_plan("a", CUSTOM_POWER_PLAN_NAME, false)
            .unwrap();
        let store = MemoryStore::default();
        let mut journal = Journal::default();
        journal.entries.push(Entry {
            seq: 1,
            batch: 1,
            change: Change::CustomPowerPlan {
                previous: base.clone(),
                base,
                created: None,
                cpu_idle_off: false,
            },
            status: EntryStatus::Pending,
        });
        store.save(&journal).unwrap();

        Engine::new(&m, &store).revert_session().unwrap();
        assert_eq!(plans(&m), ["a"]);
        assert!(store.load().unwrap().entries.is_empty());
    }

    #[test]
    fn custom_power_plan_that_cannot_be_activated_is_deleted() {
        let m = MockPlatform::new(4);
        let base = PowerPlan {
            id: "a".into(),
            name: "plan a".into(),
        };
        m.set_power_plans(vec![base.clone()], "a");
        m.set_capabilities(CapabilitySet {
            custom_power_plan: true,
            ..CapabilitySet::default()
        });
        let store = MemoryStore::default();
        let report = Engine::new(&m, &store)
            .apply(&ApplyRequest {
                custom_power_plan: Some(base),
                ..Default::default()
            })
            .unwrap();
        assert!(matches!(report.lines[0].outcome, Outcome::Failed(_)));
        assert_eq!(plans(&m), ["a"]);
        let journal = store.load().unwrap();
        assert_eq!(journal.entries[0].status, EntryStatus::Failed);
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

    /// Todos los cambios que existen hoy son de sesión: el watchdog los deshace todos.
    #[test]
    fn revert_session_undoes_every_session_change() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let game = m.spawn(9, "game.exe", "/game", 1);
        let initial = state(&m, &[&a, &game], &[]);
        let items = [item(Action::Suspend, procs(&m, &[&a]))];
        assert!(items.iter().all(|i| i.scope() == Scope::Session));
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let mut req = request(&items);
        req.game = boost(&game, Priority::High, false).game;
        engine.apply(&req).unwrap();
        assert!(
            store
                .load()
                .unwrap()
                .entries
                .iter()
                .all(|e| e.change.scope() == Scope::Session)
        );

        let report = engine.revert_session().unwrap();
        assert_eq!(report.failed(), 0);
        assert_eq!(state(&m, &[&a, &game], &[]), initial);
        assert!(!engine.has_open_session().unwrap());
    }

    /// Reanudar una fila deshace solo sus procesos; el resto de la sesión sigue aplicado y la
    /// fila se puede volver a limitar.
    #[test]
    fn revert_processes_leaves_the_rest_of_the_session() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let b = m.spawn(2, "b.exe", "/b", 1);
        let game = m.spawn(9, "game.exe", "/game", 1);
        let items = [
            item(Action::Suspend, procs(&m, &[&a])),
            item(Action::Suspend, procs(&m, &[&b])),
        ];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        let mut req = request(&items);
        req.game = boost(&game, Priority::High, false).game;
        engine.apply(&req).unwrap();

        let report = engine.revert_processes(std::slice::from_ref(&a)).unwrap();
        assert_eq!((report.done(), report.failed()), (1, 0), "{report:?}");
        assert!(!m.is_suspended(&a).unwrap());
        assert!(m.is_suspended(&b).unwrap());
        assert_eq!(m.priority(&game).unwrap(), Priority::High);
        let journal = store.load().unwrap();
        assert!(
            journal
                .entries
                .iter()
                .all(|e| e.change.target() != Some(&a))
        );

        engine.apply(&request(&items[..1])).unwrap();
        assert!(m.is_suspended(&a).unwrap());
    }

    /// P5: al salir del juego Turbo reanuda los congelados aunque el resto de la sesión se quede.
    #[test]
    fn resume_frozen_keeps_the_other_session_changes() {
        let m = MockPlatform::new(4);
        let a = m.spawn(1, "a.exe", "/a", 1);
        let b = m.spawn(2, "b.exe", "/b", 1);
        let items = [
            item(Action::Suspend, procs(&m, &[&a])),
            item(Action::LowerPriority, procs(&m, &[&b])),
        ];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        engine.apply(&request(&items)).unwrap();
        let lowered = m.priority(&b).unwrap();

        let report = engine.resume_frozen().unwrap();
        assert_eq!((report.done(), report.failed()), (1, 0), "{report:?}");
        assert!(!m.is_suspended(&a).unwrap());
        assert_eq!(m.priority(&b).unwrap(), lowered);
        assert!(engine.has_open_session().unwrap());
    }

    /// Congelados por otro programa: se reanudan sin tocar el journal; los de FrameRush, no.
    #[test]
    fn resume_foreign_skips_what_framerush_froze() {
        let m = MockPlatform::new(4);
        let ours = m.spawn(1, "a.exe", "/a", 1);
        let other = m.spawn(2, "b.exe", "/b", 1);
        let items = [item(Action::Suspend, procs(&m, &[&ours]))];
        let store = MemoryStore::default();
        let engine = Engine::new(&m, &store);
        engine.apply(&request(&items)).unwrap();
        m.suspend(&other).unwrap();
        let before = store.load().unwrap();

        let report = engine
            .resume_foreign(&[other.clone(), ours.clone()])
            .unwrap();
        assert_eq!((report.done(), report.failed()), (1, 0), "{report:?}");
        assert!(!m.is_suspended(&other).unwrap());
        assert!(m.is_suspended(&ours).unwrap());
        assert_eq!(store.load().unwrap(), before);
        assert_eq!(m.suspended().unwrap(), [ours.pid]);
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
