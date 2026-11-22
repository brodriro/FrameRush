//! Localiza Steam, parsea VDF/ACF y detecta el `appid` en ejecución.
//!
//! - [`locate`]: la carpeta de Steam del equipo (hoy solo Windows; Linux y macOS, PLAN §6).
//! - [`installed_apps`]: los juegos de todas las bibliotecas, leídos de
//!   `steamapps/libraryfolders.vdf` y de cada `steamapps/appmanifest_<appid>.acf`.
//!
//! El formato de estos archivos no está documentado por Valve: se leen solo las claves que se
//! usan (`path`, `appid`, `name`, `installdir`) y un archivo ilegible se informa sin abortar.

mod vdf;

use std::path::{Path, PathBuf};

/// Error al leer Steam.
#[derive(Debug, thiserror::Error)]
pub enum SteamError {
    #[error("no se pudo leer {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {reason}")]
    Format { path: PathBuf, reason: String },
    #[cfg(windows)]
    #[error(transparent)]
    Platform(#[from] framerush_platform::PlatformError),
}

pub type Result<T> = std::result::Result<T, SteamError>;

/// Un juego instalado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub appid: u32,
    pub name: String,
    /// Carpeta del juego: `<biblioteca>/steamapps/common/<installdir>`.
    pub dir: PathBuf,
}

/// Juegos instalados y los manifiestos que no se pudieron leer.
#[derive(Debug, Default)]
pub struct Apps {
    pub apps: Vec<App>,
    pub errors: Vec<SteamError>,
}

/// Carpeta de Steam, o `None` si no está instalado (o en un SO aún sin soporte).
pub fn locate() -> Result<Option<PathBuf>> {
    #[cfg(windows)]
    return Ok(framerush_platform::steam_root()?);
    #[cfg(not(windows))]
    Ok(None)
}

/// Carpetas de las bibliotecas según `steamapps/libraryfolders.vdf`. Sin ese archivo, la
/// biblioteca es la propia carpeta de Steam.
pub fn libraries(root: &Path) -> Result<Vec<PathBuf>> {
    let path = root.join("steamapps").join("libraryfolders.vdf");
    match std::fs::read_to_string(&path) {
        Ok(text) => vdf::library_paths(&text).map_err(|reason| SteamError::Format { path, reason }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![root.to_path_buf()]),
        Err(source) => Err(SteamError::Io { path, source }),
    }
}

/// Juegos de todas las bibliotecas, ordenados por `appid`. Una biblioteca que ya no existe (un
/// disco desconectado) no es un error.
pub fn installed_apps(root: &Path) -> Result<Apps> {
    let mut out = Apps::default();
    for library in libraries(root)? {
        let steamapps = library.join("steamapps");
        let entries = match std::fs::read_dir(&steamapps) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                out.errors.push(SteamError::Io {
                    path: steamapps,
                    source,
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_manifest(&path) {
                continue;
            }
            match read_manifest(&path, &steamapps) {
                Ok(app) => out.apps.push(app),
                Err(e) => out.errors.push(e),
            }
        }
    }
    out.apps.sort_by_key(|a| a.appid);
    out.apps.dedup_by_key(|a| a.appid);
    Ok(out)
}

fn is_manifest(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf"))
}

fn read_manifest(path: &Path, steamapps: &Path) -> Result<App> {
    let text = std::fs::read_to_string(path).map_err(|source| SteamError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let m = vdf::manifest(&text).map_err(|reason| SteamError::Format {
        path: path.to_path_buf(),
        reason,
    })?;
    Ok(App {
        appid: m.appid,
        name: m.name,
        dir: steamapps.join("common").join(m.installdir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_steam(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("framerush-steam-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        root
    }

    fn manifest(appid: u32, name: &str, installdir: &str) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{installdir}\"\n}}\n"
        )
    }

    #[test]
    fn reads_every_library_and_skips_missing_ones() {
        let root = temp_steam("libs");
        let second = root.join("second");
        std::fs::create_dir_all(second.join("steamapps")).unwrap();
        let escaped = |p: &Path| p.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"2\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                escaped(&root),
                escaped(&second),
                escaped(&root.join("unplugged")),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("steamapps/appmanifest_730.acf"),
            manifest(730, "Counter-Strike 2", "Counter-Strike Global Offensive"),
        )
        .unwrap();
        std::fs::write(
            second.join("steamapps/appmanifest_70.acf"),
            manifest(70, "Half-Life", "Half-Life"),
        )
        .unwrap();
        std::fs::write(second.join("steamapps/appmanifest_1.acf"), "roto").unwrap();
        std::fs::write(second.join("steamapps/otro.acf"), "no es un manifiesto").unwrap();

        let found = installed_apps(&root).unwrap();
        assert_eq!(
            found.apps,
            [
                App {
                    appid: 70,
                    name: "Half-Life".into(),
                    dir: second.join("steamapps").join("common").join("Half-Life"),
                },
                App {
                    appid: 730,
                    name: "Counter-Strike 2".into(),
                    dir: root
                        .join("steamapps")
                        .join("common")
                        .join("Counter-Strike Global Offensive"),
                },
            ]
        );
        assert_eq!(found.errors.len(), 1, "{:?}", found.errors);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn without_libraryfolders_the_root_is_the_library() {
        let root = temp_steam("nolibs");
        assert_eq!(libraries(&root).unwrap(), vec![root.clone()]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// En este equipo: lista lo que haya, o nada si Steam no está instalado.
    #[test]
    fn reads_this_machine() {
        match locate().unwrap() {
            Some(root) => {
                let found = installed_apps(&root).unwrap();
                for app in &found.apps {
                    println!("{} {} {}", app.appid, app.name, app.dir.display());
                }
                for e in &found.errors {
                    println!("error: {e}");
                }
            }
            None => println!("Steam no está instalado"),
        }
    }
}
