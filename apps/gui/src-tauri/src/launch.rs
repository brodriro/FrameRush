//! Lanzamiento del juego (It3.2, P4): un hilo mira qué juego de Steam está en marcha y, cuando
//! arranca uno, decide qué hacer ([`LaunchPrefs::decide`]) y emite `game-launched` con la
//! acción. La UI la ejecuta; aquí no se aplica nada.

use crate::snapshot::detected_game;
use framerush_core::Lang;
use framerush_platform::Target;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Cada cuánto se mira si ha arrancado un juego.
const INTERVAL: Duration = Duration::from_secs(3);

/// Evento `game-launched`.
#[derive(Clone, serde::Serialize)]
struct GameLaunched {
    appid: u32,
    pids: Vec<u32>,
    action: LaunchAction,
}

/// Qué hace la UI con un juego recién lanzado (P4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LaunchAction {
    /// Lanzado con «Jugar ahora»: Turbo aplica el perfil una sola vez. Con `retry`, si al
    /// aplicar el juego ya no estaba (CS2 arranca un primer proceso que se cierra), Turbo vuelve
    /// a quedar pendiente para el siguiente arranque.
    Turbo { retry: bool },
    /// Lanzado fuera de la app, con perfil guardado y `profile-auto-apply`: se carga y se aplica.
    Profile,
    /// Solo se avisa; `profile` dice si el juego tiene perfil guardado.
    Notify { profile: bool },
}

/// Lo que la UI le cuenta al hilo para decidir (P4).
#[derive(Debug)]
pub struct LaunchPrefs {
    /// Juego lanzado con «Jugar ahora» a la espera de arrancar.
    turbo_pending: Option<u32>,
    /// Si ya se usó el reintento de Turbo de este lanzamiento.
    turbo_retried: bool,
    /// `profile-auto-apply`.
    auto_apply: bool,
}

impl Default for LaunchPrefs {
    /// `profile-auto-apply` viene marcado en la UI.
    fn default() -> Self {
        Self {
            turbo_pending: None,
            turbo_retried: false,
            auto_apply: true,
        }
    }
}

impl LaunchPrefs {
    /// Turbo pendiente para `appid` (o ninguno). `fresh`: es un lanzamiento nuevo, con su
    /// reintento disponible; si no, es el reintento y no se repone.
    pub fn arm_turbo(&mut self, appid: Option<u32>, fresh: bool) {
        self.turbo_pending = appid;
        if fresh {
            self.turbo_retried = false;
        }
    }

    pub fn set_auto_apply(&mut self, on: bool) {
        self.auto_apply = on;
    }

    /// P4: lanzado desde la app → Turbo, que aplica el perfil una sola vez; fuera de la app →
    /// el perfil solo con `profile-auto-apply`.
    fn decide(&mut self, appid: u32, has_profile: bool) -> LaunchAction {
        if self.turbo_pending == Some(appid) {
            self.turbo_pending = None;
            let retry = !self.turbo_retried;
            self.turbo_retried = true;
            return LaunchAction::Turbo { retry };
        }
        if has_profile && self.auto_apply {
            LaunchAction::Profile
        } else {
            LaunchAction::Notify {
                profile: has_profile,
            }
        }
    }
}

/// Estado de Tauri con las [`LaunchPrefs`].
#[derive(Default)]
pub struct Launch(pub Mutex<LaunchPrefs>);

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

/// Arranca el hilo; vive lo que la app. Necesita el estado [`Launch`].
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
                // El error se descarta: el idioma da igual.
                *seen = detected_game(&processes, framerush_core::Lang::Es).ok()?;
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
                // Un perfiles.json ilegible ya lo avisa la UI al cargar: aquí cuenta como sin perfil.
                let has_profile = crate::profiles::load(Lang::Es)
                    .map(|p| p.contains_key(&appid))
                    .unwrap_or(false);
                let state = app.state::<Launch>();
                let action = match state.0.lock() {
                    Ok(mut prefs) => prefs.decide(appid, has_profile),
                    Err(poisoned) => poisoned.into_inner().decide(appid, has_profile),
                };
                tracing::info!(appid, ?action, "juego lanzado");
                // Sin ventana que escuche (al cerrar) no hay nada que hacer con el error.
                let _ = app.emit(
                    "game-launched",
                    GameLaunched {
                        appid,
                        pids,
                        action,
                    },
                );
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

    /// P4: «Jugar ahora» aplica con Turbo una sola vez (no además el perfil) y con un solo
    /// reintento; fuera de la app, el perfil solo con `profile-auto-apply`.
    #[test]
    fn p4_turbo_from_the_app_and_the_profile_only_outside_it() {
        use LaunchAction::*;
        let mut p = LaunchPrefs::default();
        // Fuera de la app.
        assert_eq!(p.decide(730, true), Profile);
        assert_eq!(p.decide(730, false), Notify { profile: false });
        p.set_auto_apply(false);
        assert_eq!(p.decide(730, true), Notify { profile: true });
        p.set_auto_apply(true);

        // «Jugar ahora»: Turbo aunque haya perfil y auto-aplicar, y solo para ese juego.
        p.arm_turbo(Some(730), true);
        assert_eq!(p.decide(70, true), Profile, "otro juego: fuera de la app");
        assert_eq!(p.decide(730, true), Turbo { retry: true });
        assert_eq!(
            p.decide(730, true),
            Profile,
            "el siguiente ya no es de la app"
        );

        // El reintento (la UI vuelve a armar sin reponerlo) ya no reintenta otra vez.
        p.arm_turbo(Some(730), false);
        assert_eq!(p.decide(730, false), Turbo { retry: false });
        // Un lanzamiento nuevo repone el reintento.
        p.arm_turbo(Some(730), true);
        assert_eq!(p.decide(730, false), Turbo { retry: true });

        // Cancelado (error al lanzar o ya en marcha): fuera de la app.
        p.arm_turbo(Some(730), true);
        p.arm_turbo(None, false);
        assert_eq!(p.decide(730, true), Profile);
    }

    #[test]
    fn the_game_running_when_the_app_opens_is_not_a_launch() {
        let mut l = Launches::new(Some(730));
        assert_eq!(l.tick(Some(730)), None);
    }
}
