//! Dominio puro: niveles, reglas, plan, journal de cambios y motor `apply`/`revert`.
//!
//! No llama al SO: todo pasa por los traits de `fpsboost-platform`.

pub mod model;
pub mod protected;

pub use model::{Action, Category, Level, ParseError, Risk};
pub use protected::{ProtectedReason, is_protected, protected_reason};
