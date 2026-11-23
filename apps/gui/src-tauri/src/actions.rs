//! Aplicar, deshacer y restaurar desde la GUI con `core::Engine`, como la CLI.
//!
//! La UI manda lo que el usuario dejó marcado (ids de reglas y de ajustes) y aquí se vuelve a
//! planificar con el equipo de ahora: nunca se aplica una lista vieja del snapshot. Lo que la
//! UI marca y aún no está conectado (juego, Turbo, ajustes sin backend) vuelve en `not_wired`
//! para que la UI lo diga en vez de darlo por hecho.

use crate::snapshot::{Inputs, data_dir, game_targets, inputs, steam_install};
use anyhow::{Context, Result};
use framerush_core::{
    ApplyRequest, FileStore, GameBoost, Level, Outcome, PlanOptions, RefreshRate, Report, defaults,
    persistent_priority_default, plan,
};
use framerush_platform::{Priority, Target};
use serde::{Deserialize, Serialize};

/// Ids de design/SETTINGS.md que no son reglas ni ajustes de `Tweak`/`MemoryOp`.
const POWER_PLAN: &str = "power-plan";
const CUSTOM_POWER_PLAN: &str = "custom-power-plan";
const REFRESH_RATE_MAX: &str = "refresh-rate-max";
/// Ajustes del exe del juego que llegan como ids de `settings`.
const PERSISTENT_PRIORITY: &str = "game-priority-persistent";
const FSO_OFF: &str = "fullscreen-optimizations-off";
/// `game-config-lock` llega en `settings` (es del nivel); lo aplica el adaptador.
const GAME_CONFIG_LOCK: &str = "game-config-lock";

/// Lo que el usuario dejó marcado en la UI.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    /// 1 = Normal, 2 = Agresivo, 3 = Extremo.
    level: u8,
    /// `rule_id` de las filas marcadas.
    rows: Vec<String>,
    /// `data-setting` de los ajustes marcados del nivel.
    settings: Vec<String>,
    /// Índice del plan de energía elegido en `device.power.plans`.
    power_plan: Option<usize>,
    /// El juego elegido en `game-target` y su panel; `null` sin juegos.
    game: Option<GameSelection>,
    /// `auto-revert-on-exit`: al cerrarse el juego, el watchdog revierte lo de sesión (P5).
    auto_revert: bool,
}

/// Panel del juego (R3).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSelection {
    appid: u32,
    priority: GamePriority,
    /// Segunda confirmación de Tiempo real (It1.5).
    realtime_confirmed: bool,
    /// CPU lógicas de `cpu-affinity-cores` si `cpu-affinity` está marcado.
    affinity: Option<Vec<usize>>,
    /// `gpu-preference = alto`.
    high_performance_gpu: bool,
    /// Panel del adaptador (It5.1); `null` si el juego no tiene.
    adapter: Option<AdapterSelection>,
}

/// Panel del adaptador del juego. `game-config-lock` va en `settings`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterSelection {
    /// `game-config-tune` (opt-in).
    tune: bool,
    /// `fps-cap`: 0 = sin tope.
    fps_cap: u32,
    /// `bench-cfg`.
    bench: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum GamePriority {
    Normal,
    Alta,
    Realtime,
}

impl From<GamePriority> for Priority {
    fn from(p: GamePriority) -> Self {
        match p {
            GamePriority::Normal => Priority::Normal,
            GamePriority::Alta => Priority::High,
            GamePriority::Realtime => Priority::Realtime,
        }
    }
}

/// Resultado para la UI: una línea por acción.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcomes {
    lines: Vec<Line>,
    done: usize,
    failed: usize,
    /// Marcados que la app todavía no sabe aplicar.
    not_wired: Vec<String>,
    /// Marcados que piden admin y no hay helper elevado.
    needs_admin: Vec<String>,
    /// Lo que se pidió y no se aplicó, con el motivo (juego sin procesos, anticheat...).
    notes: Vec<String>,
    /// Queda una sesión que deshacer.
    session_open: bool,
    /// El watchdog vigila el juego para revertir lo de sesión cuando se cierre.
    watching: bool,
    /// Procesos del juego que vigilar (solo para el backend).
    #[serde(skip)]
    watch: Vec<Target>,
}

impl Outcomes {
    /// Procesos que el watchdog debe vigilar; vacío si no hay que vigilar nada.
    pub fn take_watch(&mut self) -> Vec<Target> {
        std::mem::take(&mut self.watch)
    }

    pub fn set_watching(&mut self, watching: bool) {
        self.watching = watching;
    }
}

#[derive(Debug, Clone, Serialize)]
struct Line {
    what: String,
    /// `done`, `skipped` o `failed`.
    outcome: &'static str,
    detail: Option<String>,
}

fn store() -> Result<FileStore> {
    let dir = data_dir().context("no hay carpeta de datos (LOCALAPPDATA)")?;
    Ok(FileStore::new(dir.join("session.json")))
}

fn level(n: u8) -> Result<Level> {
    Level::ALL
        .get(usize::from(n).wrapping_sub(1))
        .copied()
        .with_context(|| format!("nivel desconocido: {n}"))
}

/// Aplica lo marcado. `helper` es el helper elevado si el usuario lo abrió.
pub fn apply(selection: &Selection, helper: Option<Elevated>) -> Result<Outcomes> {
    let level = level(selection.level)?;
    let Inputs {
        device,
        rules,
        processes,
        mut snapshot,
        ..
    } = inputs()?;
    // El juego elegido (si está en marcha) queda fuera del plan aunque Steam no lo dé como el
    // juego en marcha.
    let game_targets = match &selection.game {
        Some(g) => game_targets(g.appid, &processes)?,
        None => vec![],
    };
    if !game_targets.is_empty() {
        snapshot.game = game_targets.clone();
    }
    let options = PlanOptions {
        elevated: helper.is_some(),
        keep_alive: defaults::keep_alive(&device).value,
    };
    let plan = plan(level, &snapshot, &rules, &device, &options);
    let wanted = |id: &str| selection.settings.iter().any(|s| s == id);

    let items: Vec<_> = plan
        .items
        .iter()
        .filter(|i| !i.locked && selection.rows.contains(&i.rule_id))
        .collect();
    let mut needs_admin: Vec<String> = plan
        .items
        .iter()
        .filter(|i| i.locked && selection.rows.contains(&i.rule_id))
        .map(|i| i.rule_id.clone())
        .collect();
    let tweaks: Vec<_> = plan
        .tweaks
        .iter()
        .chain(&plan.optional_tweaks)
        .copied()
        .filter(|t| wanted(t.id()))
        .collect();
    needs_admin.extend(
        plan.locked_tweaks
            .iter()
            .filter(|t| wanted(t.id()))
            .map(|t| t.id().to_owned()),
    );
    let memory: Vec<_> = plan
        .memory
        .iter()
        .copied()
        .filter(|m| wanted(m.id()))
        .collect();
    needs_admin.extend(
        plan.locked_memory
            .iter()
            .filter(|m| wanted(m.id()))
            .map(|m| m.id().to_owned()),
    );
    let custom_power_plan = wanted(CUSTOM_POWER_PLAN)
        .then(|| defaults::custom_power_plan_base(level, &device).cloned())
        .flatten();
    // El plan elegido solo se activa si no es el que ya está.
    let power_plan = selection
        .power_plan
        .and_then(|i| device.power_plans.get(i))
        .filter(|p| device.active_power_plan.as_deref() != Some(p.id.as_str()))
        .filter(|_| custom_power_plan.is_none())
        .cloned();
    let refresh_rates: Vec<RefreshRate> = if wanted(REFRESH_RATE_MAX) {
        defaults::refresh_rate_max(level, &device)
            .into_iter()
            .map(|(d, hz)| RefreshRate {
                display: d.id.clone(),
                hz,
            })
            .collect()
    } else {
        vec![]
    };

    let mut notes = vec![];
    let game = selection.game.as_ref().and_then(|g| {
        if game_targets.is_empty() {
            notes.push("el juego elegido no está en marcha: su prioridad no se toca".to_owned());
            return None;
        }
        let priority = Priority::from(g.priority);
        let affinity = g.affinity.clone().filter(|_| {
            let allowed = plan.game_tuning_allowed();
            if !allowed {
                notes.push(format!(
                    "anticheat en ejecución ({}): afinidad desactivada (P2)",
                    plan.anticheats.join(", ")
                ));
            }
            allowed
        });
        let persistent =
            wanted(PERSISTENT_PRIORITY) && persistent_priority_default(level, priority);
        if persistent && helper.is_none() {
            needs_admin.push(PERSISTENT_PRIORITY.to_owned());
        }
        Some(GameBoost {
            targets: game_targets.clone(),
            priority,
            realtime_confirmed: g.realtime_confirmed,
            fullscreen_optimizations_off: wanted(FSO_OFF),
            affinity,
            high_performance_gpu: g.high_performance_gpu
                && defaults::gpu_preference(&device).is_some(),
            persistent_priority: persistent && helper.is_some(),
        })
    });
    if game.is_none() {
        for id in [FSO_OFF, PERSISTENT_PRIORITY] {
            if wanted(id) {
                notes.push(format!("{id}: no hay juego en marcha"));
            }
        }
    }

    let game_files = match selection
        .game
        .as_ref()
        .zip(selection.game.as_ref().and_then(|g| g.adapter))
    {
        Some((g, a)) => {
            let options = framerush_games::GameOptions {
                tune: a.tune,
                lock: wanted(GAME_CONFIG_LOCK),
                fps_cap: a.fps_cap,
                bench: a.bench,
            };
            adapter_files(
                g.appid,
                &device,
                options,
                !game_targets.is_empty(),
                &mut notes,
            )?
        }
        None => vec![],
    };

    let handled = |id: &str| {
        id == POWER_PLAN
            || id == GAME_CONFIG_LOCK
            || id == CUSTOM_POWER_PLAN
            || id == REFRESH_RATE_MAX
            || id == FSO_OFF
            || id == PERSISTENT_PRIORITY
            || tweaks.iter().any(|t| t.id() == id)
            || memory.iter().any(|m| m.id() == id)
            || needs_admin.iter().any(|n| n == id)
    };
    let not_wired: Vec<String> = selection
        .settings
        .iter()
        .filter(|id| !handled(id))
        .cloned()
        .collect();

    let request = ApplyRequest {
        items,
        tweaks,
        game,
        memory,
        power_plan,
        custom_power_plan,
        refresh_rates,
        game_files,
    };
    let report = run(helper, Op::Apply(Box::new(request)))?;
    let mut out = outcomes(report, not_wired, needs_admin, notes)?;
    // Solo hay algo que revertir al cerrar el juego si quedó una sesión abierta.
    if selection.auto_revert && out.session_open {
        out.watch = game_targets;
    }
    Ok(out)
}

/// Lo que el adaptador del juego quiere escribir. Sin adaptador o sin Steam, nada.
fn adapter_files(
    appid: u32,
    device: &framerush_device::DeviceProfile,
    options: framerush_games::GameOptions,
    running: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<framerush_core::GameFileWrite>> {
    let Some(adapter) = framerush_games::adapter(appid) else {
        return Ok(vec![]);
    };
    let Some((root, app)) = steam_install(appid)? else {
        notes.push(format!("{}: no está instalado en Steam", adapter.name()));
        return Ok(vec![]);
    };
    let install = framerush_games::Install {
        dir: &app.dir,
        steam_root: &root,
    };
    let plan = adapter
        .plan(install, device, options)
        .with_context(|| format!("no se pudo leer la config de {}", adapter.name()))?;
    notes.extend(plan.notes);
    // El juego reescribe su config al salir (referencia CS2-FPS-Boost): sin bloqueo se pierde.
    if running && options.tune && !options.lock {
        notes.push(format!(
            "{} está abierto: al cerrarse puede reescribir su config de vídeo (ciérralo antes o marca «Bloquear config»)",
            adapter.name()
        ));
    }
    Ok(plan.writes)
}

/// Watchdog: el juego se cerró y se deshace lo de sesión (lo persistente queda hasta
/// Restaurar, P5).
pub fn revert_session(helper: Option<Elevated>) -> Result<Outcomes> {
    let report = run(helper, Op::RevertSession)?;
    outcomes(report, vec![], vec![], vec![])
}

/// Deshace el último `apply`.
pub fn undo(helper: Option<Elevated>) -> Result<Outcomes> {
    let report = run(helper, Op::Undo)?;
    outcomes(report, vec![], vec![], vec![])
}

/// Deshace toda la sesión (Restaurar).
pub fn revert(helper: Option<Elevated>) -> Result<Outcomes> {
    let report = run(helper, Op::Revert)?;
    outcomes(report, vec![], vec![], vec![])
}

fn outcomes(
    report: Report,
    not_wired: Vec<String>,
    needs_admin: Vec<String>,
    notes: Vec<String>,
) -> Result<Outcomes> {
    let session_open = framerush_core::JournalStore::load(&store()?)
        .map(|j| j.is_open())
        .unwrap_or(false);
    Ok(Outcomes {
        done: report.done(),
        failed: report.failed(),
        lines: report
            .lines
            .into_iter()
            .map(|l| {
                let (outcome, detail) = match l.outcome {
                    Outcome::Done => ("done", None),
                    Outcome::Skipped(why) => ("skipped", Some(why)),
                    Outcome::Failed(e) => ("failed", Some(e.to_string())),
                };
                Line {
                    what: l.what,
                    outcome,
                    detail,
                }
            })
            .collect(),
        not_wired,
        needs_admin,
        notes,
        session_open,
        watching: false,
        watch: vec![],
    })
}

/// Lo que mantiene abierto el helper elevado.
#[cfg(windows)]
pub type Elevated = framerush_platform::Helper;
/// Fuera de Windows aún no hay helper: el tipo no tiene valores y `Option<Elevated>` siempre es
/// `None`.
#[cfg(not(windows))]
#[derive(Clone)]
pub enum Elevated {}

// Fuera de Windows `run` no las ejecuta, así que no se leen.
#[cfg_attr(not(windows), allow(dead_code))]
enum Op<'a> {
    Apply(Box<ApplyRequest<'a>>),
    Undo,
    Revert,
    RevertSession,
}

#[cfg(windows)]
fn run(helper: Option<Elevated>, op: Op<'_>) -> Result<Report> {
    let platform = match helper {
        Some(h) => framerush_platform::WindowsPlatform::with_helper(h),
        None => framerush_platform::WindowsPlatform::new(),
    };
    let store = store()?;
    let engine = framerush_core::Engine::new(&platform, &store);
    Ok(match op {
        Op::Apply(request) => engine.apply(&request).context("no se pudo aplicar")?,
        Op::Undo => engine.undo_last().context("no se pudo deshacer")?,
        Op::Revert => engine.revert_all().context("no se pudo restaurar")?,
        Op::RevertSession => engine
            .revert_session()
            .context("no se pudo revertir la sesión")?,
    })
}

#[cfg(not(windows))]
fn run(_helper: Option<Elevated>, _op: Op<'_>) -> Result<Report> {
    anyhow::bail!("aplicar, deshacer y restaurar solo están implementados en Windows por ahora")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El JSON que manda `api.apply` en app.js.
    #[test]
    fn reads_the_ui_selection() {
        let s: Selection = serde_json::from_str(
            r#"{"level":2,"rows":["sync-onedrive"],"settings":["game-dvr-off"],"powerPlan":null,
                "game":{"appid":70,"priority":"realtime","realtimeConfirmed":true,
                        "affinity":[2,3],"highPerformanceGpu":false},
                "autoRevert":true}"#,
        )
        .unwrap();
        assert_eq!(level(s.level).unwrap(), Level::Agresivo);
        assert_eq!(s.rows, ["sync-onedrive"]);
        assert_eq!(s.settings, ["game-dvr-off"]);
        assert_eq!(s.power_plan, None);
        assert!(s.auto_revert);
        let g = s.game.unwrap();
        assert_eq!(
            (g.appid, Priority::from(g.priority)),
            (70, Priority::Realtime)
        );
        assert!(g.realtime_confirmed && !g.high_performance_gpu);
        assert_eq!(g.affinity, Some(vec![2, 3]));
    }

    #[test]
    fn levels_are_one_based() {
        assert_eq!(level(1).unwrap(), Level::Normal);
        assert_eq!(level(3).unwrap(), Level::Extremo);
        assert!(level(0).is_err());
        assert!(level(4).is_err());
    }
}
