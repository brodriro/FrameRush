//! Regla del checklist tal como sale de `rules/*.toml` (PLAN.md §3, "Modelo de reglas").
//!
//! El crate `framerush-rules` carga y valida los archivos; aquí solo vive el tipo, para que
//! `core` no dependa del formato.

use crate::protected::normalize;
use crate::{Action, Category, Level, Risk};
use framerush_platform::Os;
use serde::{Deserialize, Serialize};

/// Nombres de proceso o servicio que activan la regla, por SO.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OsNames {
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub linux: Vec<String>,
    #[serde(default)]
    pub macos: Vec<String>,
}

impl OsNames {
    pub fn for_os(&self, os: Os) -> &[String] {
        match os {
            Os::Windows => &self.windows,
            Os::Linux => &self.linux,
            Os::Macos => &self.macos,
        }
    }

    /// Todos los nombres, de cualquier SO.
    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.windows.iter().chain(&self.linux).chain(&self.macos)
    }
}

/// Una regla: qué procesos o servicios toca, con qué acción, desde qué nivel y con qué riesgo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    /// Procesos (o servicios, con `stop_service`) que activan la regla.
    #[serde(rename = "match")]
    pub matches: OsNames,
    pub category: Category,
    pub action: Action,
    pub min_level: Level,
    pub risk: Risk,
    pub description: String,
    /// La fila aparece desmarcada en todos los niveles (p. ej. navegadores).
    #[serde(default)]
    pub optional: bool,
    /// El proceso pertenece al sistema o a otro usuario y hace falta admin (P12).
    #[serde(default)]
    pub elevated: bool,
}

impl Rule {
    /// `true` si `process_name` (nombre o ruta) es uno de los procesos de la regla en `os`.
    /// No distingue mayúsculas y no tiene en cuenta el `.exe`.
    pub fn matches_process(&self, os: Os, process_name: &str) -> bool {
        let name = normalize(process_name);
        self.matches.for_os(os).iter().any(|n| normalize(n) == name)
    }

    /// `true` si `service` es uno de los servicios de la regla en `os` (sin distinguir
    /// mayúsculas, como el SCM de Windows).
    pub fn matches_service(&self, os: Os, service: &str) -> bool {
        self.matches
            .for_os(os)
            .iter()
            .any(|n| n.eq_ignore_ascii_case(service))
    }

    /// Hace falta admin para aplicarla: servicios siempre, procesos si `elevated`.
    pub fn requires_admin(&self) -> bool {
        self.elevated || self.action == Action::StopService
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> Rule {
        Rule {
            id: "onedrive".into(),
            matches: OsNames {
                windows: vec!["OneDrive".into()],
                ..OsNames::default()
            },
            category: Category::Sync,
            action: Action::Suspend,
            min_level: Level::Agresivo,
            risk: Risk::Low,
            description: "Sync".into(),
            optional: false,
            elevated: false,
        }
    }

    #[test]
    fn process_match_ignores_case_path_and_exe() {
        let r = rule();
        assert!(r.matches_process(Os::Windows, r"C:\Program Files\OneDrive\onedrive.EXE"));
        assert!(!r.matches_process(Os::Linux, "OneDrive"));
        assert!(!r.matches_process(Os::Windows, "OneDriveSetup.exe"));
    }

    #[test]
    fn services_always_require_admin() {
        let mut r = rule();
        assert!(!r.requires_admin());
        r.action = Action::StopService;
        assert!(r.requires_admin());
    }
}
