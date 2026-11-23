//! GUI de FrameRush: una ventana de 420 px sin bordes y transparente con el frontend de
//! `apps/gui/ui` (diseño final v3). Comandos y eventos del contrato de `design/README.md`:
//!
//! - `snapshot`: el equipo, el plan y la sesión pendiente, leídos de este equipo (R8).
//! - evento `sensors` a 1 Hz con las lecturas de los sensores (`null` = "No disponible").
//! - `elevate`: abre el helper elevado (UAC) y vuelve a abrir los sensores a través de él, para
//!   las fuentes que solo se leen con admin (el EC de Acer).
//! - `apply`, `undo` y `revert`: lo marcado en la UI, con `core::Engine` y el journal de la CLI.
//!   Con el helper abierto, lo que pide admin pasa por él.
//! - Watchdog (It1.4, P5): tras `apply`, con `auto-revert-on-exit`, vigila el juego y al cerrarse
//!   revierte lo de sesión y emite `game-exited` con el informe.
//! - `profiles` y `save_profile`: perfiles por juego (It3.2) en `profiles.json`.
//! - evento `game-launched` cuando arranca un juego de Steam: la UI carga y aplica su perfil si
//!   `profile-auto-apply` está activo (P4).
//! - Turbo (It5.2): `launch_game` pide a Steam que lance el juego; al arrancar, la UI aplica con
//!   `turbo` y el watchdog congela cada `turbo-rescan` las instancias nuevas (evento
//!   `turbo-rescan`) y al salir reanuda siempre lo congelado (P5).
//!
//! El diagnóstico aún no está conectado: la UI lo dice.

// Sin consola en las compilaciones de release de Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod launch;
mod profiles;
mod snapshot;

use actions::Elevated;

use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Fuentes de sensores abiertas y el hilo que las lee.
struct Sensors(Mutex<SensorState>);

struct SensorState {
    info: snapshot::SensorInfo,
    /// Se sustituye al elevar; al soltarlo se para el hilo.
    _poller: framerush_sensors::Poller,
    /// Helper elevado: vive lo que la app aunque ninguna fuente lo use.
    helper: Option<Elevated>,
}

/// Empieza a leer `sources` y a emitir el evento `sensors`.
fn start(
    app: &AppHandle,
    sources: framerush_sensors::Sources,
    helper: Option<Elevated>,
) -> SensorState {
    let handle = app.clone();
    let poller = framerush_sensors::Poller::spawn(
        sources.providers,
        framerush_sensors::POLL_INTERVAL,
        move |sensors| {
            // Sin ventana que escuche (al cerrar) no hay nada que hacer con el error.
            let _ = handle.emit("sensors", snapshot::sensor_views(&sensors));
        },
    );
    SensorState {
        info: snapshot::SensorInfo {
            checked: sources.checked,
            needs_admin: sources.needs_admin,
            elevated: helper.is_some(),
        },
        _poller: poller,
        helper,
    }
}

/// Cada cuánto mira el watchdog si el juego sigue en marcha.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Watchdog en marcha, si lo hay. Soltarlo lo para.
struct Watchdog(Mutex<Option<GameWatch>>);

struct GameWatch {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for GameWatch {
    fn drop(&mut self) {
        // Soltar el emisor despierta al hilo (Disconnected).
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Informe del evento `game-exited`.
#[derive(Clone, serde::Serialize)]
struct GameExited {
    outcomes: Option<actions::Outcomes>,
    error: Option<String>,
}

/// Vigila el juego y, cuando ya no queda ningún proceso suyo, revierte lo de sesión (o, en Turbo
/// sin `auto-revert-on-exit`, solo reanuda lo congelado, P5). En Turbo congela además las
/// instancias nuevas cada `turbo-rescan`. El hilo no toca el estado de la app: termina solo y
/// el `GameWatch` se suelta en el siguiente `apply` o `revert`.
fn watch(app: AppHandle, plan: actions::WatchPlan, helper: Option<Elevated>) -> GameWatch {
    let (stop, stopped) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let mut last_rescan = Instant::now();
        loop {
            match stopped.recv_timeout(WATCH_INTERVAL) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
            // Un escaneo fallido no es un juego cerrado: se vuelve a mirar.
            let Ok(processes) = framerush_platform::scan_processes() else {
                continue;
            };
            if framerush_core::game_running(&plan.game, &processes) {
                if let Some(turbo) = plan
                    .turbo
                    .as_ref()
                    .filter(|t| last_rescan.elapsed() >= t.every)
                {
                    last_rescan = Instant::now();
                    let payload = match actions::turbo_rescan(turbo, helper.clone()) {
                        Ok(outcomes) => GameExited {
                            outcomes: Some(outcomes),
                            error: None,
                        },
                        Err(e) => GameExited {
                            outcomes: None,
                            error: Some(format!("{e:#}")),
                        },
                    };
                    let _ = app.emit("turbo-rescan", payload);
                }
                continue;
            }
            let result = if plan.revert_session {
                actions::revert_session(helper)
            } else {
                actions::resume_frozen(helper)
            };
            let payload = match result {
                Ok(outcomes) => GameExited {
                    outcomes: Some(outcomes),
                    error: None,
                },
                Err(e) => GameExited {
                    outcomes: None,
                    error: Some(format!("{e:#}")),
                },
            };
            // Sin ventana que escuche (al cerrar) no hay nada que hacer con el error.
            let _ = app.emit("game-exited", payload);
            return;
        }
    });
    GameWatch {
        stop: Some(stop),
        thread: Some(thread),
    }
}

/// Para el watchdog, si lo hay.
fn stop_watch(watchdog: &Watchdog) -> Result<(), String> {
    let old = watchdog
        .0
        .lock()
        .map_err(|_| "estado del watchdog roto".to_owned())?
        .take();
    // Se suelta fuera del cerrojo: esperar al hilo con él tomado no hace falta.
    drop(old);
    Ok(())
}

/// Lee el equipo fuera del hilo de la UI: WMI, NVML y el escaneo de procesos tardan.
#[tauri::command]
async fn snapshot(sensors: tauri::State<'_, Sensors>) -> Result<snapshot::View, String> {
    let info = sensors
        .0
        .lock()
        .map_err(|_| "estado de los sensores roto".to_owned())?
        .info
        .clone();
    tauri::async_runtime::spawn_blocking(move || snapshot::build(info))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Helper elevado abierto, si lo hay (se comparte: un solo UAC).
fn helper(sensors: &Sensors) -> Result<Option<Elevated>, String> {
    Ok(sensors
        .0
        .lock()
        .map_err(|_| "estado de los sensores roto".to_owned())?
        .helper
        .clone())
}

/// Corre `op` fuera del hilo de la UI (escanea procesos y toca el sistema).
async fn blocking<T: Send + 'static>(
    op: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn apply(
    app: AppHandle,
    selection: actions::Selection,
    sensors: tauri::State<'_, Sensors>,
    watchdog: tauri::State<'_, Watchdog>,
) -> Result<actions::Outcomes, String> {
    let helper = helper(&sensors)?;
    let for_apply = helper.clone();
    let mut out = blocking(move || actions::apply(&selection, for_apply)).await?;
    let plan = out.take_watch();
    stop_watch(&watchdog)?;
    if let Some(plan) = plan {
        let w = watch(app, plan, helper);
        *watchdog
            .0
            .lock()
            .map_err(|_| "estado del watchdog roto".to_owned())? = Some(w);
        out.set_watching(true);
    }
    Ok(out)
}

/// Perfiles guardados por `appid`.
#[tauri::command]
fn profiles() -> Result<profiles::Profiles, String> {
    profiles::load().map_err(|e| format!("{e:#}"))
}

/// Guarda (o sustituye) el perfil del juego `appid`.
#[tauri::command]
fn save_profile(appid: u32, profile: profiles::Profile) -> Result<(), String> {
    profiles::save(appid, profile).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn undo(sensors: tauri::State<'_, Sensors>) -> Result<actions::Outcomes, String> {
    let helper = helper(&sensors)?;
    blocking(move || actions::undo(helper)).await
}

/// «Jugar ahora» (Turbo): pide a Steam que lance el juego `appid`.
#[tauri::command]
async fn launch_game(appid: u32) -> Result<(), String> {
    blocking(move || actions::launch_game(appid)).await
}

/// Filas con algún proceso limitado por la sesión.
#[tauri::command]
async fn limited_rows() -> Result<Vec<String>, String> {
    blocking(actions::limited_rows).await
}

/// Reanuda una fila sin tocar el resto de la sesión (el watchdog sigue).
#[tauri::command]
async fn release_row(
    id: String,
    sensors: tauri::State<'_, Sensors>,
) -> Result<actions::Outcomes, String> {
    let helper = helper(&sensors)?;
    blocking(move || actions::release_row(&id, helper)).await
}

/// Vuelve a limitar una fila reanudada con la acción del nivel (sin cerrar procesos).
#[tauri::command]
async fn limit_row(
    id: String,
    level: u8,
    sensors: tauri::State<'_, Sensors>,
) -> Result<actions::Outcomes, String> {
    let helper = helper(&sensors)?;
    blocking(move || actions::limit_row(&id, level, helper)).await
}

/// Restaurar: deshace toda la sesión y para el watchdog (ya no queda nada que revertir).
#[tauri::command]
async fn revert(
    sensors: tauri::State<'_, Sensors>,
    watchdog: tauri::State<'_, Watchdog>,
) -> Result<actions::Outcomes, String> {
    stop_watch(&watchdog)?;
    let helper = helper(&sensors)?;
    blocking(move || actions::revert(helper)).await
}

/// Abre el helper elevado (UAC) y vuelve a abrir los sensores a través de él.
#[tauri::command]
async fn elevate(app: AppHandle) -> Result<(), String> {
    let (sources, helper) = tauri::async_runtime::spawn_blocking(elevated_sources)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    let state = start(&app, sources, Some(helper));
    let sensors = app.state::<Sensors>();
    let mut current = sensors
        .0
        .lock()
        .map_err(|_| "estado de los sensores roto".to_owned())?;
    // El hilo anterior se para al soltar su `Poller`.
    *current = state;
    Ok(())
}

/// El helper va junto al ejecutable de la app (`cargo build --workspace` lo compila).
#[cfg(windows)]
fn elevated_sources() -> anyhow::Result<(framerush_sensors::Sources, Elevated)> {
    use anyhow::Context;
    let exe = std::env::current_exe()
        .context("no se encontró el ejecutable de la app")?
        .with_file_name("framerush-helper.exe");
    anyhow::ensure!(
        exe.is_file(),
        "falta {} (compílalo con `cargo build --workspace`)",
        exe.display()
    );
    let helper = framerush_platform::Helper::launch(&exe).context("no se pudo abrir el helper")?;
    Ok((framerush_sensors::detect_elevated(helper.clone()), helper))
}

#[cfg(not(windows))]
fn elevated_sources() -> anyhow::Result<(framerush_sensors::Sources, Elevated)> {
    anyhow::bail!("en este sistema todavía no hay helper elevado")
}

fn main() -> anyhow::Result<()> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            snapshot,
            elevate,
            apply,
            undo,
            revert,
            release_row,
            launch_game,
            limited_rows,
            limit_row,
            profiles,
            save_profile
        ])
        .setup(|app| {
            let state = start(app.handle(), framerush_sensors::detect(), None);
            app.manage(Sensors(Mutex::new(state)));
            app.manage(Watchdog(Mutex::new(None)));
            launch::spawn(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
