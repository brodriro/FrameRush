//! Steam en Linux: su carpeta y el `appid` del juego en marcha (V12).
//!
//! Fuentes:
//! - Carpeta: `$XDG_DATA_HOME/Steam` (`~/.local/share/Steam` si `XDG_DATA_HOME` no está, vacía o
//!   es relativa; XDG Base Directory Specification) y, si ahí no hay `steamapps`, `~/.steam/root`
//!   y `~/.steam/steam` (Steam Support, FAQ 68D2-35AB-09A9-7678: las rutas de Linux y macOS).
//! - Juego en marcha: Steam pone `SteamAppId` en el entorno del juego (steam-runtime-tools,
//!   `docs/steam-compat-tool-interface.md`: «The environment variable `STEAM_COMPAT_APP_ID` is
//!   set to the app-ID of the game. The environment variable `SteamAppId` has the same value»).
//!   Se lee de `/proc/<pid>/environ`, entradas `CLAVE=VALOR` separadas por NUL
//!   (proc_pid_environ(5)). El `RunningAppID` de `registry.vdf` que usa Windows no es de fiar en
//!   Linux y ese archivo no está documentado, así que no se lee.

use std::path::{Path, PathBuf};

/// Carpeta de Steam, o `None` si no está instalada (ninguna de sus rutas tiene `steamapps`).
pub fn steam_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    let data_home = super::data_home()?;
    installed(root_candidates(Path::new(&home), &data_home))
}

/// Rutas de Steam por orden de preferencia: la de XDG primero y después las de `~/.steam`.
fn root_candidates(home: &Path, data_home: &Path) -> Vec<PathBuf> {
    vec![
        data_home.join("Steam"),
        home.join(".steam").join("root"),
        home.join(".steam").join("steam"),
    ]
}

/// La primera carpeta de `candidates` con `steamapps` dentro.
fn installed(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|dir| dir.join("steamapps").is_dir())
}

/// `appid` del juego que Steam tiene en marcha, o `None` si ningún proceso del usuario lleva
/// `SteamAppId`. Si la llevan varios, gana el que arrancó más tarde: los restos de un juego ya
/// cerrado (un `wineserver` de Proton, por ejemplo) siguen con su entorno.
pub fn steam_running_app() -> Option<u32> {
    let me = std::process::id();
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut found: Option<(u64, u32)> = None;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        // El propio proceso no es el juego.
        if pid == me {
            continue;
        }
        // De otro usuario no se puede leer: se ignora.
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
            continue;
        };
        let Some(appid) = app_id(&environ) else {
            continue;
        };
        let started = start_ticks(pid).unwrap_or(0);
        if found.is_none_or(|(ticks, _)| started >= ticks) {
            found = Some((started, appid));
        }
    }
    found.map(|(_, appid)| appid)
}

/// `SteamAppId` de un `/proc/<pid>/environ`. `SteamAppIdX` no cuenta y 0 es «ninguno».
fn app_id(environ: &[u8]) -> Option<u32> {
    environ
        .split(|byte| *byte == 0)
        .filter_map(|entry| entry.strip_prefix(b"SteamAppId="))
        .find_map(|value| {
            std::str::from_utf8(value)
                .ok()?
                .parse()
                .ok()
                .filter(|&appid| appid != 0)
        })
}

/// Arranque del proceso en ticks desde el inicio del sistema: campo 22 (`starttime`) de
/// `/proc/<pid>/stat`, contado desde el último `)` (el nombre de la tarea va entre paréntesis y
/// puede llevar más, proc_pid_stat(5)).
fn start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn reads_the_app_id_from_the_environment() {
        assert_eq!(app_id(b"SteamAppId=730\0PATH=/usr/bin\0"), Some(730));
        assert_eq!(app_id(b"PATH=/usr/bin\0SteamAppId=730"), Some(730));
        // 0 es «ninguno» y una clave que empieza igual no es `SteamAppId`.
        assert_eq!(app_id(b"SteamAppId=0\0"), None);
        assert_eq!(app_id(b"SteamAppIdX=730\0"), None);
        assert_eq!(app_id(b"SteamGameId=730\0"), None);
        assert_eq!(app_id(b""), None);
        assert_eq!(app_id(b"SteamAppId=raro\0"), None);
    }

    #[test]
    fn the_first_candidate_with_steamapps_wins() {
        let root =
            std::env::temp_dir().join(format!("framerush-steam-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let data = home.join(".local/share");

        let candidates = root_candidates(&home, &data);
        assert_eq!(
            candidates,
            [
                data.join("Steam"),
                home.join(".steam/root"),
                home.join(".steam/steam"),
            ]
        );
        assert_eq!(installed(candidates.clone()), None, "sin `steamapps`");

        std::fs::create_dir_all(home.join(".steam/steam/steamapps")).unwrap();
        assert_eq!(
            installed(candidates.clone()),
            Some(home.join(".steam/steam")),
            "sin la de XDG se usa la de `~/.steam`"
        );

        std::fs::create_dir_all(data.join("Steam/steamapps")).unwrap();
        assert_eq!(installed(candidates), Some(data.join("Steam")));

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Un hijo con `SteamAppId` en el entorno sale como el juego en marcha, y uno que arranca
    /// después con otro `appid` pasa a ganar.
    #[test]
    fn finds_the_game_in_the_environment_of_a_process() {
        let sleeper = |appid: &str| {
            Command::new("sleep")
                .arg("30")
                .env("SteamAppId", appid)
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut first = sleeper("70");
        // `spawn` vuelve antes del `exec`: hasta entonces el entorno es el del test.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if std::fs::read(format!("/proc/{}/environ", first.id()))
                .is_ok_and(|env| app_id(&env) == Some(70))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(steam_running_app(), Some(70));

        let mut second = sleeper("730");
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if steam_running_app() == Some(730) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            steam_running_app(),
            Some(730),
            "gana el que arrancó después"
        );

        let _ = first.kill();
        let _ = second.kill();
        let _ = first.wait();
        let _ = second.wait();
    }
}
