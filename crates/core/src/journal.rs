//! Journal persistente de cambios (PLAN.md It1.1, It5.5).
//!
//! Cada cambio se guarda como `Pending` *antes* de ejecutarse y pasa a `Done` o `Failed`
//! después. Si la app se cierra de golpe, el journal sigue en disco y al arrancar se ofrece
//! "Restaurar sesión anterior".

use crate::Scope;
use framerush_platform::{Priority, SavedValue, Target, Tweak};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Un cambio aplicado al sistema, con lo necesario para deshacerlo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    /// Prioridad cambiada. `previous` es el estado original y no se sobrescribe al reaplicar.
    Priority {
        target: Target,
        previous: Priority,
        applied: Priority,
    },
    Suspend {
        target: Target,
    },
    /// Proceso cerrado: se registra, pero no se puede deshacer.
    Kill {
        target: Target,
    },
    /// Servicio detenido durante la sesión; al revertir se vuelve a arrancar.
    StopService {
        name: String,
    },
    /// Ajuste persistente. `previous` son los valores de antes (backup único: reaplicar no lo
    /// sobrescribe).
    Tweak {
        tweak: Tweak,
        previous: Vec<SavedValue>,
    },
}

impl Change {
    pub fn is_reversible(&self) -> bool {
        !matches!(self, Change::Kill { .. })
    }

    /// Prioridad (incluida la del juego), congelados, cierres y servicios son de sesión; los
    /// ajustes del sistema son persistentes.
    pub fn scope(&self) -> Scope {
        match self {
            Change::Priority { .. }
            | Change::Suspend { .. }
            | Change::Kill { .. }
            | Change::StopService { .. } => Scope::Session,
            Change::Tweak { .. } => Scope::Persistent,
        }
    }

    /// Proceso al que afecta, si es un cambio de proceso.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Change::Priority { target, .. }
            | Change::Suspend { target }
            | Change::Kill { target } => Some(target),
            Change::StopService { .. } | Change::Tweak { .. } => None,
        }
    }
}

/// Estado de una entrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// Registrada; la acción puede haberse ejecutado o no (la app se cerró a mitad).
    Pending,
    Done,
    Failed,
}

/// Una entrada del journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Orden global de aplicación.
    pub seq: u64,
    /// Cada `apply` es un lote; `undo-last` deshace solo el último.
    pub batch: u64,
    pub change: Change,
    pub status: EntryStatus,
}

/// Contenido del journal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

impl Journal {
    /// Hay cambios que deshacer (una sesión sin cerrar).
    pub fn is_open(&self) -> bool {
        self.entries.iter().any(|e| e.status != EntryStatus::Failed)
    }

    pub fn next_seq(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| e.seq)
            .max()
            .map_or(1, |s| s + 1)
    }

    pub fn last_batch(&self) -> Option<u64> {
        self.entries.iter().map(|e| e.batch).max()
    }
}

/// Error del almacenamiento del journal.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("no se pudo leer o escribir el journal {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("el journal {} está dañado: {source}", path.display())]
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// Dónde se guarda el journal.
pub trait JournalStore {
    /// Journal guardado, o uno vacío si no hay.
    fn load(&self) -> Result<Journal, JournalError>;
    fn save(&self, journal: &Journal) -> Result<(), JournalError>;
}

/// Journal en un archivo JSON. Se escribe en un temporal y se renombra, así que un cierre a
/// mitad de escritura deja la versión anterior entera. Un journal vacío borra el archivo.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn io(&self, source: std::io::Error) -> JournalError {
        JournalError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

impl JournalStore for FileStore {
    fn load(&self) -> Result<Journal, JournalError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| JournalError::Corrupt {
                path: self.path.clone(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(e) => Err(self.io(e)),
        }
    }

    fn save(&self, journal: &Journal) -> Result<(), JournalError> {
        if journal.entries.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(self.io(e)),
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| self.io(e))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(journal).map_err(|source| JournalError::Corrupt {
            path: self.path.clone(),
            source,
        })?;
        std::fs::write(&tmp, bytes).map_err(|e| self.io(e))?;
        // En Windows, `rename` sustituye el destino si existe.
        std::fs::rename(&tmp, &self.path).map_err(|e| self.io(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_platform::StartTime;

    fn sample() -> Journal {
        Journal {
            entries: vec![Entry {
                seq: 1,
                batch: 1,
                change: Change::Priority {
                    target: Target {
                        pid: 4242,
                        start_time: StartTime(7),
                        exe_path: r"C:\Apps\OneDrive.exe".into(),
                    },
                    previous: Priority::Normal,
                    applied: Priority::BelowNormal,
                },
                status: EntryStatus::Done,
            }],
        }
    }

    #[test]
    fn file_store_round_trips_and_clears() {
        let dir = std::env::temp_dir().join(format!("framerush-journal-{}", std::process::id()));
        let store = FileStore::new(dir.join("session.json"));
        assert_eq!(store.load().unwrap(), Journal::default());
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), sample());
        assert!(!store.path().with_extension("json.tmp").exists());
        store.save(&Journal::default()).unwrap();
        assert!(!store.path().exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_journal_is_an_error_not_an_empty_session() {
        let dir = std::env::temp_dir().join(format!("framerush-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = FileStore::new(dir.join("session.json"));
        std::fs::write(store.path(), "{ no es json").unwrap();
        assert!(matches!(store.load(), Err(JournalError::Corrupt { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_session_ignores_failed_entries() {
        let mut j = sample();
        assert!(j.is_open());
        j.entries[0].status = EntryStatus::Failed;
        assert!(!j.is_open());
        assert_eq!(j.next_seq(), 2);
    }
}
