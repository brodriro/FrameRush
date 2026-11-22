//! Lanzamiento del juego (It3.2, P4): un hilo mira qué juego de Steam está en marcha y, cuando
//! arranca uno, emite `game-launched`. La UI decide con `profile-auto-apply` si carga y aplica su
//! perfil; aquí no se aplica nada.

use crate::snapshot::detected_game;
use framerush_platform::Target;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Cada cuánto se mira si ha arrancado un juego.
const INTERVAL: Duration = Duration::from_secs(3);

/// Evento `game-launched`.
#[derive(Clone, serde::Serialize)]
struct GameLaunched {
    appid: u32,
    pids: Vec<u32>,
}

/// Cuándo un juego cuenta como lanzado: pasa de no estar (o estar otro) a estar en marcha. El
/// juego que ya corría al abrir la app no cuenta: no lo lanzó el usuario ahora.
struct Launches {
    last: Option<u32>,
}

impl Launches {
    fn new(now: Option<u32>) -> Self {
        Self { last: now }
    }

    /// El `appid` recién lanzado, si lo hay.
    fn tick(&mut self, now: Option<u32>) -> Option<u32> {
        let launched = now.filter(|id| self.last != Some(*id));
        self.last = now;
        launched
    }
}

/// Arranca el hilo; vive lo que la app.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        // El último juego visto sigue siendo «el juego» mientras viva alguno de sus procesos:
        // detectado por primer plano, un Alt+Tab no lo cierra ni lo vuelve a lanzar.
        let mut seen: Option<(u32, Vec<Target>)> = None;
        // Un fallo al leer Steam o los procesos no es un juego cerrado ni lanzado: se salta.
        let read = |seen: &mut Option<(u32, Vec<Target>)>| -> Option<Option<u32>> {
            let processes = framerush_platform::scan_processes().ok()?;
            let alive = seen
                .as_ref()
                .is_some_and(|(_, t)| framerush_core::game_running(t, &processes));
            if !alive {
                *seen = detected_game(&processes).ok()?;
            }
            Some(seen.as_ref().map(|(id, _)| *id))
        };
        let mut launches = Launches::new(read(&mut seen).flatten());
        loop {
            std::thread::sleep(INTERVAL);
            let Some(now) = read(&mut seen) else { continue };
            if let Some(appid) = launches.tick(now) {
                let pids = seen
                    .as_ref()
                    .map(|(_, t)| t.iter().map(|t| t.pid).collect())
                    .unwrap_or_default();
                // Sin ventana que escuche (al cerrar) no hay nada que hacer con el error.
                let _ = app.emit("game-launched", GameLaunched { appid, pids });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_counts_once_when_it_starts() {
        let mut l = Launches::new(None);
        assert_eq!(l.tick(None), None);
        assert_eq!(l.tick(Some(730)), Some(730));
        assert_eq!(
            l.tick(Some(730)),
            None,
            "sigue en marcha: no se vuelve a lanzar"
        );
        assert_eq!(l.tick(None), None);
        assert_eq!(l.tick(Some(730)), Some(730), "cerrado y vuelto a abrir");
        assert_eq!(l.tick(Some(70)), Some(70), "otro juego");
    }

    #[test]
    fn the_game_running_when_the_app_opens_is_not_a_launch() {
        let mut l = Launches::new(Some(730));
        assert_eq!(l.tick(Some(730)), None);
    }
}
