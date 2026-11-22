//! Perfiles por juego (It3.2): lo elegido en la UI para un juego, guardado por `appid` en
//! `profiles.json` dentro de la carpeta de datos.
//!
//! El contenido (nivel, checklist, ajustes y panel del juego) lo decide la UI: el backend lo
//! guarda tal cual y solo responde de que el archivo no se corrompa.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::snapshot::data_dir;

/// Perfil de un juego.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Nombre del juego al guardarlo, para mostrarlo aunque el juego ya no esté instalado.
    pub name: String,
    /// Estado de la UI (formato de `apps/gui/ui/app.js`, `profileState`).
    pub data: serde_json::Value,
}

/// Perfiles por `appid`.
pub type Profiles = BTreeMap<u32, Profile>;

fn path() -> Result<PathBuf> {
    let dir = data_dir().context("no hay carpeta de datos (LOCALAPPDATA)")?;
    Ok(dir.join("profiles.json"))
}

/// Todos los perfiles guardados; sin archivo, ninguno.
pub fn load() -> Result<Profiles> {
    read(&path()?)
}

/// Guarda (o sustituye) el perfil de `appid`.
pub fn save(appid: u32, profile: Profile) -> Result<()> {
    let path = path()?;
    let mut all = read(&path)?;
    all.insert(appid, profile);
    write(&path, &all)
}

fn read(path: &Path) -> Result<Profiles> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Profiles::new()),
        Err(e) => return Err(e).with_context(|| format!("no se pudo leer {}", path.display())),
    };
    // Un archivo roto es un error: no se sobrescribe sin avisar.
    serde_json::from_str(&text).with_context(|| format!("{} no es válido", path.display()))
}

/// Escribe en un temporal y lo renombra: un cierre a mitad no deja el archivo a medias.
fn write(path: &Path, all: &Profiles) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("no se pudo crear {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(all).context("no se pudo serializar los perfiles")?;
    std::fs::write(&tmp, text).with_context(|| format!("no se pudo escribir {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("no se pudo guardar {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("framerush-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn missing_file_is_no_profiles_and_save_round_trips() {
        let path = dir("roundtrip").join("profiles.json");
        assert!(read(&path).unwrap().is_empty());
        let mut all = Profiles::new();
        all.insert(
            730,
            Profile {
                name: "Counter-Strike 2".into(),
                data: serde_json::json!({"level": 3, "rows": {"onedrive": false}}),
            },
        );
        write(&path, &all).unwrap();
        assert_eq!(read(&path).unwrap(), all);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn a_broken_file_is_an_error_not_an_empty_list() {
        let d = dir("broken");
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("profiles.json");
        std::fs::write(&path, "{ roto").unwrap();
        assert!(read(&path).is_err());
    }
}
