//! Dominio puro: niveles, reglas, plan, journal de cambios y motor `apply`/`revert`.
//!
//! No llama al SO: todo pasa por los traits de `framerush-platform`.

pub mod defaults;
mod engine;
mod journal;
mod model;
mod plan;
mod protected;
mod rule;

pub use engine::{
    ApplyRequest, CUSTOM_POWER_PLAN_NAME, Engine, EngineError, GameBoost, LOWERED_PRIORITY,
    Outcome, RefreshRate, Report, ReportLine, StatusLine, describe, status_lines,
};
pub use journal::{Change, Entry, EntryStatus, FileStore, Journal, JournalError, JournalStore};
pub use model::{Action, Category, Level, ParseError, Risk, Scope};
pub use plan::{
    MemoryOp, Plan, PlanItem, PlanOptions, Snapshot, Subject, persistent_priority_default, plan,
    running_anticheats, tweak_min_level, tweak_optional, tweak_scope,
};
pub use protected::{ProtectedReason, is_protected, protected_reason};
pub use rule::{OsNames, Rule};
