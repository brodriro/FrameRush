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
//! - Segundo plano: cerrar la ventana con el watchdog en marcha solo la oculta (evento
//!   `background`) y la app termina al cerrarse el juego. Una sola instancia: abrir la app otra
//!   vez muestra la ventana que vigila.
//! - Overlay (It3.5): atajo global (`set_overlay_hotkey`) que muestra u oculta la ventana e
//!   icono en la bandeja con «Mostrar u ocultar» y «Salir» (deshace antes lo que vigila).
//! - `export_diagnostics` (It4.5): informe JSON en Descargas con los logs locales de `tracing`.

// Sin consola en las compilaciones de release de Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod diagnostics;
mod launch;
mod overlay;
mod profiles;
mod snapshot;

use actions::Elevated;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Lo que se hará al cerrarse el juego (`auto-revert-on-exit`), también al salir de la app.
    revert_session: bool,
}

impl GameWatch {
    /// El hilo sigue vigilando (termina solo al cerrarse el juego).
    fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
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
    let revert_session = plan.revert_session;
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
                    let payload = match logged(
                        "turbo-rescan",
                        actions::turbo_rescan(turbo, helper.clone()).map_err(|e| format!("{e:#}")),
                    ) {
                        Ok(outcomes) => GameExited {
                            outcomes: Some(outcomes),
                            error: None,
                        },
                        Err(e) => GameExited {
                            outcomes: None,
                            error: Some(e),
                        },
                    };
                    let _ = app.emit("turbo-rescan", payload);
                }
                continue;
            }
            tracing::info!(revert_session = plan.revert_session, "el juego se cerró");
            let result = if plan.revert_session {
                actions::revert_session(helper)
            } else {
                actions::resume_frozen(helper)
            };
            let payload = match logged("game-exited", result.map_err(|e| format!("{e:#}"))) {
                Ok(outcomes) => GameExited {
                    outcomes: Some(outcomes),
                    error: None,
                },
                Err(e) => GameExited {
                    outcomes: None,
                    error: Some(e),
                },
            };
            // Sin ventana que escuche (al cerrar) no hay nada que hacer con el error.
            let _ = app.emit("game-exited", payload);
            // La ventana se cerró mientras vigilaba: ya no queda nada que hacer.
            if app.state::<Closed>().0.load(Ordering::Relaxed) {
                tracing::info!("vigilancia en segundo plano terminada: la app se cierra");
                app.exit(0);
            }
            return;
        }
    });
    GameWatch {
        stop: Some(stop),
        thread: Some(thread),
        revert_session,
    }
}

/// El usuario cerró la ventana con el juego vigilado: al cerrarse el juego, la app termina.
/// Ocultarla con el atajo o la bandeja no cuenta (la app sigue en la bandeja).
struct Closed(AtomicBool);

/// «Salir» de la bandeja. Si el watchdog vigila, antes hace lo que haría al cerrarse el juego
/// (deshacer lo de sesión o reanudar lo congelado), para no dejar procesos congelados.
fn quit(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let watch = app
            .state::<Watchdog>()
            .0
            .lock()
            .ok()
            .and_then(|mut w| w.take());
        // Soltar el `GameWatch` para su hilo antes de deshacer.
        let pending = watch.filter(GameWatch::running).map(|w| w.revert_session);
        if let Some(revert_session) = pending {
            let helper = helper(&app.state::<Sensors>()).ok().flatten();
            let result = if revert_session {
                actions::revert_session(helper)
            } else {
                actions::resume_frozen(helper)
            };
            let _ = logged("quit", result.map_err(|e| format!("{e:#}")));
        }
        tracing::info!("salir desde la bandeja");
        app.exit(0);
    });
}

/// Trae al frente la ventana principal (otra instancia de la app pidió abrirse).
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    app.state::<Closed>().0.store(false, Ordering::Relaxed);
}

/// Al cerrar la ventana con el watchdog vigilando, se oculta en vez de cerrarse: el juego
/// sigue y lo de sesión se deshace cuando termine.
fn on_close(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let watching = window
        .state::<Watchdog>()
        .0
        .lock()
        .map(|w| w.as_ref().is_some_and(GameWatch::running))
        .unwrap_or(false);
    if watching {
        api.prevent_close();
        window.state::<Closed>().0.store(true, Ordering::Relaxed);
        let _ = window.hide();
        tracing::info!("ventana cerrada con el juego vigilado: sigue en segundo plano");
        let _ = window.emit("background", ());
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

/// Vista Estado: journal y equipo ahora, fuera del hilo de la UI (WMI tarda).
#[tauri::command]
async fn status() -> Result<snapshot::StatusView, String> {
    blocking(snapshot::status).await
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

/// Deja `result` en el log de diagnóstico y lo devuelve.
fn logged(
    op: &str,
    result: Result<actions::Outcomes, String>,
) -> Result<actions::Outcomes, String> {
    match &result {
        Ok(out) => out.log(op),
        Err(e) => tracing::error!(op, error = %e, "falló"),
    }
    result
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
    let mut out = logged(
        "apply",
        blocking(move || actions::apply(&selection, for_apply)).await,
    )?;
    let plan = out.take_watch();
    stop_watch(&watchdog)?;
    if let Some(plan) = plan {
        let w = watch(app, plan, helper);
        *watchdog
            .0
            .lock()
            .map_err(|_| "estado del watchdog roto".to_owned())? = Some(w);
        out.set_watching(true);
        tracing::info!("watchdog vigilando el juego");
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
    logged("undo", blocking(move || actions::undo(helper)).await)
}

/// «Jugar ahora» (Turbo): pide a Steam que lance el juego `appid`, salvo que ya esté en marcha
/// (`true`).
#[tauri::command]
async fn launch_game(appid: u32) -> Result<bool, String> {
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
    logged(
        "release_row",
        blocking(move || actions::release_row(&id, helper)).await,
    )
}

/// Vuelve a limitar una fila reanudada con la acción del nivel (sin cerrar procesos).
#[tauri::command]
async fn limit_row(
    id: String,
    level: u8,
    keep_alive: Option<Vec<String>>,
    sensors: tauri::State<'_, Sensors>,
) -> Result<actions::Outcomes, String> {
    let helper = helper(&sensors)?;
    logged(
        "limit_row",
        blocking(move || actions::limit_row(&id, level, keep_alive.as_deref(), helper)).await,
    )
}

/// Restaurar: deshace toda la sesión y para el watchdog (ya no queda nada que revertir).
#[tauri::command]
async fn revert(
    sensors: tauri::State<'_, Sensors>,
    watchdog: tauri::State<'_, Watchdog>,
) -> Result<actions::Outcomes, String> {
    stop_watch(&watchdog)?;
    let helper = helper(&sensors)?;
    logged("revert", blocking(move || actions::revert(helper)).await)
}

/// Abre el helper elevado (UAC) y vuelve a abrir los sensores a través de él.
#[tauri::command]
async fn elevate(app: AppHandle) -> Result<(), String> {
    let (sources, helper) = tauri::async_runtime::spawn_blocking(elevated_sources)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
        .inspect_err(|e| tracing::error!(error = %e, "no se pudo abrir el helper"))?;
    tracing::info!("helper elevado abierto");
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

/// Alerta térmica o de ventilador parado (It3.6): notificación del sistema, que se ve también
/// con el juego a pantalla completa. Se llama desde Rust para no dar permisos del plugin a la UI.
#[tauri::command]
fn notify(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

/// Atajo global del overlay (`overlay-hotkey`); vacío lo quita.
#[tauri::command]
fn set_overlay_hotkey(app: AppHandle, accel: String) -> Result<(), String> {
    overlay::set_hotkey(&app, &accel)
}

/// Textos del menú de la bandeja en el idioma de la UI.
#[tauri::command]
fn tray_labels(app: AppHandle, toggle: String, quit: String) -> Result<(), String> {
    overlay::set_labels(&app, &toggle, &quit)
}

/// Informe de diagnóstico (It4.5): devuelve la ruta del archivo guardado.
#[tauri::command]
async fn export_diagnostics() -> Result<String, String> {
    blocking(|| diagnostics::export().map(|p| p.display().to_string())).await
}

fn main() -> anyhow::Result<()> {
    tauri::Builder::default()
        // Debe ser el primer plugin (documentación de Tauri).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                on_close(window, api);
            }
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            status,
            elevate,
            apply,
            undo,
            revert,
            release_row,
            launch_game,
            limited_rows,
            limit_row,
            profiles,
            save_profile,
            notify,
            export_diagnostics,
            set_overlay_hotkey,
            tray_labels
        ])
        .setup(|app| {
            // Aquí y no antes: una segunda instancia ya ha salido (single-instance) y no pisa el
            // log de la que está abierta.
            if let Some(log) = diagnostics::init_logs() {
                tracing::info!(version = env!("CARGO_PKG_VERSION"), log = %log.display(), "FrameRush arranca");
            }
            let state = start(app.handle(), framerush_sensors::detect(), None);
            app.manage(Sensors(Mutex::new(state)));
            app.manage(Watchdog(Mutex::new(None)));
            app.manage(Closed(AtomicBool::new(false)));
            launch::spawn(app.handle().clone());
            overlay::tray(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
