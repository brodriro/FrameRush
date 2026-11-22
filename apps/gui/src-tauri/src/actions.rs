//! Aplicar, deshacer y restaurar desde la GUI con `core::Engine`, como la CLI.
//!
//! La UI manda lo que el usuario dejó marcado (ids de reglas y de ajustes) y aquí se vuelve a
//! planificar con el equipo de ahora: nunca se aplica una lista vieja del snapshot. Lo que la
//! UI marca y aún no está conectado (juego, Turbo, ajustes sin backend) vuelve en `not_wired`
//! para que la UI lo diga en vez de darlo por hecho.

use crate::snapshot::{Inputs, data_dir, inputs};
use anyhow::{Context, Result};
use framerush_core::{
    ApplyRequest, FileStore, Level, Outcome, PlanOptions, RefreshRate, Report, defaults, plan,
};
use serde::{Deserialize, Serialize};

/// Ids de design/SETTINGS.md que no son reglas ni ajustes de `Tweak`/`MemoryOp`.
const POWER_PLAN: &str = "power-plan";
const CUSTOM_POWER_PLAN: &str = "custom-power-plan";
const REFRESH_RATE_MAX: &str = "refresh-rate-max";

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
}

/// Resultado para la UI: una línea por acción.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcomes {
    lines: Vec<Line>,
    done: usize,
    failed: usize,
    /// Marcados que la app todavía no sabe aplicar.
    not_wired: Vec<String>,
    /// Marcados que piden admin y no hay helper elevado.
    needs_admin: Vec<String>,
    /// Queda una sesión que deshacer.
    session_open: bool,
}

#[derive(Debug, Serialize)]
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
        snapshot,
        ..
    } = inputs()?;
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

    let handled = |id: &str| {
        id == POWER_PLAN
            || id == CUSTOM_POWER_PLAN
            || id == REFRESH_RATE_MAX
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
        game: None,
        memory,
        power_plan,
        custom_power_plan,
        refresh_rates,
    };
    let report = run(helper, Op::Apply(Box::new(request)))?;
    outcomes(report, not_wired, needs_admin)
}

/// Deshace el último `apply`.
pub fn undo(helper: Option<Elevated>) -> Result<Outcomes> {
    let report = run(helper, Op::Undo)?;
    outcomes(report, vec![], vec![])
}

/// Deshace toda la sesión (Restaurar).
pub fn revert(helper: Option<Elevated>) -> Result<Outcomes> {
    let report = run(helper, Op::Revert)?;
    outcomes(report, vec![], vec![])
}

fn outcomes(report: Report, not_wired: Vec<String>, needs_admin: Vec<String>) -> Result<Outcomes> {
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
        session_open,
    })
}

/// Lo que mantiene abierto el helper elevado.
#[cfg(windows)]
pub type Elevated = framerush_platform::Helper;
#[cfg(not(windows))]
pub type Elevated = ();

// Fuera de Windows `run` no las ejecuta, así que no se leen.
#[cfg_attr(not(windows), allow(dead_code))]
enum Op<'a> {
    Apply(Box<ApplyRequest<'a>>),
    Undo,
    Revert,
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
            r#"{"level":2,"rows":["sync-onedrive"],"settings":["game-dvr-off"],"powerPlan":null}"#,
        )
        .unwrap();
        assert_eq!(level(s.level).unwrap(), Level::Agresivo);
        assert_eq!(s.rows, ["sync-onedrive"]);
        assert_eq!(s.settings, ["game-dvr-off"]);
        assert_eq!(s.power_plan, None);
    }

    #[test]
    fn levels_are_one_based() {
        assert_eq!(level(1).unwrap(), Level::Normal);
        assert_eq!(level(3).unwrap(), Level::Extremo);
        assert!(level(0).is_err());
        assert!(level(4).is_err());
    }
}
