//! Dominio puro: niveles, reglas, plan, journal de cambios y motor `apply`/`revert`.
//!
//! No llama al SO: todo pasa por los traits de `framerush-platform`.

pub mod defaults;
mod journal;
mod model;
mod plan;
mod protected;
mod rule;

pub use journal::{Change, Entry, EntryStatus, FileStore, Journal, JournalError, JournalStore};
pub use model::{Action, Category, Level, ParseError, Risk};
pub use plan::{Plan, PlanItem, PlanOptions, Snapshot, Subject, plan};
pub use protected::{ProtectedReason, is_protected, protected_reason};
pub use rule::{OsNames, Rule};
