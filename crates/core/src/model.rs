//! Tipos base del dominio: nivel, categoría, acción y riesgo (PLAN.md §3, "Modelo de reglas").
//!
//! Los nombres serializados coinciden con los de `rules/*.toml` y con la CLI
//! (`--level agresivo`).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Nivel de optimización.
///
/// Es acumulativo (R1): Extremo ⊇ Agresivo ⊇ Normal. El orden derivado
/// (`Normal < Agresivo < Extremo`) es el que se compara con el `min_level` de cada regla.
/// El nivel solo decide qué filas vienen marcadas; la decisión final es del checklist.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Sin admin: solo ajustes del usuario.
    Normal,
    /// Admin, sin reinicio. Nivel por defecto (PLAN.md §3, "Valores por defecto").
    #[default]
    Agresivo,
    /// Admin y, para algunos ajustes, reinicio.
    Extremo,
}

impl Level {
    /// Todos los niveles, de menor a mayor.
    pub const ALL: [Level; 3] = [Level::Normal, Level::Agresivo, Level::Extremo];

    /// `true` si una regla con `min_level` entra en este nivel.
    pub fn includes(self, min_level: Level) -> bool {
        self >= min_level
    }

    /// Nombre tal como aparece en TOML y en la CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Normal => "normal",
            Level::Agresivo => "agresivo",
            Level::Extremo => "extremo",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error al leer un valor del modelo desde texto.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("valor desconocido para {kind}: {value:?}")]
pub struct ParseError {
    kind: &'static str,
    value: String,
}

impl FromStr for Level {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Level::ALL
            .into_iter()
            .find(|l| l.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| ParseError {
                kind: "nivel",
                value: s.to_owned(),
            })
    }
}

/// Categoría de un proceso o servicio en el checklist.
///
/// Los grupos A y B de servicios no son categorías: se expresan con `min_level`
/// (agresivo y extremo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Telemetry,
    Launcher,
    Updater,
    Overlay,
    Sync,
    Peripheral,
    Browser,
    Service,
}

/// Qué se le hace a un proceso o servicio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Bajar la prioridad de CPU.
    LowerPriority,
    /// Congelar el proceso; se reanuda al revertir.
    Suspend,
    /// Cerrar el proceso. No es reversible.
    Kill,
    /// Detener un servicio durante la sesión, sin cambiar su tipo de inicio.
    StopService,
}

impl Action {
    /// `false` solo para [`Action::Kill`]: un proceso cerrado no se puede restaurar, así que los
    /// tests de `apply + revert` lo excluyen.
    pub fn is_reversible(self) -> bool {
        !matches!(self, Action::Kill)
    }

    /// Acción que se ejecuta de verdad en `level` para una regla con este `risk`.
    ///
    /// Cerrar procesos solo se permite en Extremo y con riesgo bajo; en cualquier otro caso
    /// un `Kill` se degrada a `Suspend`.
    pub fn effective(self, level: Level, risk: Risk) -> Action {
        match self {
            Action::Kill if level == Level::Extremo && risk == Risk::Low => Action::Kill,
            Action::Kill => Action::Suspend,
            other => other,
        }
    }

    /// Todas las acciones sobre procesos y servicios son de sesión (PLAN.md §3).
    pub fn scope(self) -> Scope {
        Scope::Session
    }
}

/// Alcance de un cambio (PLAN.md §3, "Precedencia y alcance").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Lo revierte el watchdog al cerrar el juego (`auto-revert-on-exit`) y Restaurar.
    Session,
    /// Registro, IFEO, config del juego, Defender, `bcdedit`…: solo lo revierte Restaurar.
    /// Algunos piden reiniciar. Todavía no hay ningún cambio persistente (llegan en M1/M5).
    Persistent,
}

/// Riesgo de tocar un proceso o servicio, mostrado en cada fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered_and_cumulative() {
        assert!(Level::Normal < Level::Agresivo && Level::Agresivo < Level::Extremo);
        assert!(Level::Extremo.includes(Level::Normal));
        assert!(Level::Agresivo.includes(Level::Agresivo));
        assert!(!Level::Normal.includes(Level::Agresivo));
    }

    #[test]
    fn default_level_is_agresivo() {
        assert_eq!(Level::default(), Level::Agresivo);
    }

    #[test]
    fn level_parses_from_cli_text() {
        assert_eq!("agresivo".parse(), Ok(Level::Agresivo));
        assert_eq!("EXTREMO".parse(), Ok(Level::Extremo));
        assert!("turbo".parse::<Level>().is_err());
        for level in Level::ALL {
            assert_eq!(level.to_string().parse(), Ok(level));
        }
    }

    #[test]
    fn names_match_the_toml_schema() {
        assert_eq!(
            serde_json::to_string(&Level::Extremo).unwrap(),
            "\"extremo\""
        );
        assert_eq!(
            serde_json::from_str::<Action>("\"stop_service\"").unwrap(),
            Action::StopService
        );
        assert_eq!(
            serde_json::from_str::<Category>("\"peripheral\"").unwrap(),
            Category::Peripheral
        );
        assert_eq!(
            serde_json::from_str::<Risk>("\"medium\"").unwrap(),
            Risk::Medium
        );
    }

    #[test]
    fn only_kill_is_irreversible() {
        assert!(!Action::Kill.is_reversible());
        for a in [Action::LowerPriority, Action::Suspend, Action::StopService] {
            assert!(a.is_reversible());
        }
    }

    #[test]
    fn kill_only_at_extremo_with_low_risk() {
        assert_eq!(
            Action::Kill.effective(Level::Extremo, Risk::Low),
            Action::Kill
        );
        assert_eq!(
            Action::Kill.effective(Level::Extremo, Risk::Medium),
            Action::Suspend
        );
        assert_eq!(
            Action::Kill.effective(Level::Agresivo, Risk::Low),
            Action::Suspend
        );
        assert_eq!(
            Action::Suspend.effective(Level::Normal, Risk::High),
            Action::Suspend
        );
    }
}
