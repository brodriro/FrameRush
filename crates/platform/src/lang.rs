//! Idioma de los textos del backend.

use serde::{Deserialize, Serialize};

/// Idioma de los textos que genera el backend (informes, estado, motivos, errores). La GUI pasa el
/// suyo; la CLI usa el español, que es el de por defecto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Es,
    En,
}

impl Lang {
    /// El de este idioma entre `es` y `en`.
    pub fn pick<T>(self, es: T, en: T) -> T {
        match self {
            Lang::Es => es,
            Lang::En => en,
        }
    }
}
