//! Dominio puro: niveles, reglas, plan, journal de cambios y motor `apply`/`revert`.
//!
//! No llama al SO: todo pasa por los traits de `framerush-platform`.

pub mod model;
pub mod protected;
pub mod rule;

pub use model::{Action, Category, Level, ParseError, Risk};
pub use protected::{ProtectedReason, is_protected, protected_reason};
pub use rule::{OsNames, Rule};
