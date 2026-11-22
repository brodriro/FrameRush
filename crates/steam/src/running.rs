//! Qué juego de Steam está en marcha (V12).
//!
//! Primero el `RunningAppID` que publica Steam (solo Windows); si no hay, el proceso en primer
//! plano cuando su exe está bajo la carpeta de un juego instalado (todos los SO).

use crate::{App, Result};
use framerush_platform::ProcessInfo;
use std::path::{Component, Path};

/// De dónde salió el juego detectado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `RunningAppID` de Steam.
    RunningAppId,
    /// El proceso en primer plano está bajo la carpeta del juego.
    Foreground,
}

/// `appid` que Steam dice tener en marcha, o `None` (ninguno o SO sin esa fuente).
pub fn running_app_id() -> Result<Option<u32>> {
    #[cfg(windows)]
    return Ok(framerush_platform::steam_running_app()?);
    #[cfg(not(windows))]
    Ok(None)
}

/// El juego en marcha entre `apps`. `running` es el [`running_app_id`] y `foreground`, el exe
/// en primer plano.
pub fn detect<'a>(
    apps: &'a [App],
    running: Option<u32>,
    foreground: Option<&Path>,
) -> Option<(&'a App, Source)> {
    if let Some(app) = running.and_then(|id| apps.iter().find(|a| a.appid == id)) {
        return Some((app, Source::RunningAppId));
    }
    let exe = foreground?;
    apps.iter()
        .find(|a| is_under(&a.dir, exe))
        .map(|app| (app, Source::Foreground))
}

/// Procesos cuyo exe está bajo la carpeta de `app`.
pub fn game_processes<'a>(app: &App, processes: &'a [ProcessInfo]) -> Vec<&'a ProcessInfo> {
    processes
        .iter()
        .filter(|p| is_under(&app.dir, &p.target.exe_path))
        .collect()
}

/// `path` está dentro de `dir`. En Windows sin distinguir mayúsculas ni `/` de `\`: Steam guarda
/// `SteamPath` en minúsculas y con `/` (`c:/program files (x86)/steam`).
fn is_under(dir: &Path, path: &Path) -> bool {
    let key = |c: Component<'_>| {
        let s = c.as_os_str().to_string_lossy();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s.into_owned()
        }
    };
    let mut path = path.components();
    let mut dir = dir.components().peekable();
    // Una carpeta vacía no contiene nada.
    dir.peek().is_some() && dir.all(|d| path.next().is_some_and(|p| key(p) == key(d)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_platform::{StartTime, Target};
    use std::path::PathBuf;

    fn app(appid: u32, dir: &str) -> App {
        App {
            appid,
            name: format!("app {appid}"),
            dir: PathBuf::from(dir),
        }
    }

    fn process(pid: u32, exe: &str) -> ProcessInfo {
        ProcessInfo {
            target: Target {
                pid,
                start_time: StartTime(0),
                exe_path: PathBuf::from(exe),
            },
            name: exe.rsplit(['/', '\\']).next().unwrap_or(exe).to_owned(),
            memory_bytes: 0,
            cpu_percent: None,
            own: Some(true),
            framerush: false,
        }
    }

    #[cfg(windows)]
    const CS2: &str =
        "c:/program files (x86)/steam/steamapps/common/Counter-Strike Global Offensive";
    #[cfg(windows)]
    const CS2_EXE: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe";
    #[cfg(not(windows))]
    const CS2: &str = "/home/u/.steam/steam/steamapps/common/Counter-Strike Global Offensive";
    #[cfg(not(windows))]
    const CS2_EXE: &str = "/home/u/.steam/steam/steamapps/common/Counter-Strike Global Offensive/game/bin/linuxsteamrt64/cs2";

    #[test]
    fn running_app_id_wins_over_the_foreground() {
        let apps = [app(70, "/games/hl"), app(730, CS2)];
        assert_eq!(
            detect(&apps, Some(70), Some(Path::new(CS2_EXE))),
            Some((&apps[0], Source::RunningAppId))
        );
    }

    #[test]
    fn falls_back_to_the_foreground_process() {
        let apps = [app(70, "/games/hl"), app(730, CS2)];
        // Un appid que no está instalado no cuenta.
        assert_eq!(
            detect(&apps, Some(999), Some(Path::new(CS2_EXE))),
            Some((&apps[1], Source::Foreground))
        );
        assert_eq!(detect(&apps, None, Some(Path::new("/usr/bin/bash"))), None);
        assert_eq!(detect(&apps, None, None), None);
    }

    #[test]
    fn a_sibling_folder_with_the_same_prefix_is_not_inside() {
        let apps = [app(1, "/games/Portal")];
        assert_eq!(
            detect(&apps, None, Some(Path::new("/games/Portal 2/portal2"))),
            None
        );
        assert!(!is_under(Path::new(""), Path::new("/games/x")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_ignore_case_and_slashes() {
        assert!(is_under(Path::new(CS2), Path::new(CS2_EXE)));
    }

    #[test]
    fn lists_the_game_processes() {
        let cs2 = app(730, CS2);
        let processes = [process(1, CS2_EXE), process(2, "/usr/bin/discord")];
        let found: Vec<u32> = game_processes(&cs2, &processes)
            .iter()
            .map(|p| p.target.pid)
            .collect();
        assert_eq!(found, [1]);
    }
}
