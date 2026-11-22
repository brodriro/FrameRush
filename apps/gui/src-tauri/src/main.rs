//! GUI de FrameRush: una ventana de 420 px sin bordes y transparente con el frontend de
//! `apps/gui/ui` (diseño final v3). Comandos y eventos del contrato de `design/README.md`:
//!
//! - `snapshot`: el equipo, el plan y la sesión pendiente, leídos de este equipo (R8).
//! - evento `sensors` a 1 Hz con las lecturas de los sensores (`null` = "No disponible").
//! - `elevate`: abre el helper elevado (UAC) y vuelve a abrir los sensores a través de él, para
//!   las fuentes que solo se leen con admin (el EC de Acer).
//!
//! Aplicar, deshacer y restaurar aún no están conectados: la UI lo dice en vez de simularlo.

// Sin consola en las compilaciones de release de Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod snapshot;

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// Fuentes de sensores abiertas y el hilo que las lee.
struct Sensors(Mutex<SensorState>);

struct SensorState {
    info: snapshot::SensorInfo,
    /// Se sustituye al elevar; al soltarlo se para el hilo.
    _poller: framerush_sensors::Poller,
    /// Helper elevado: vive lo que la app aunque ninguna fuente lo use.
    _helper: Option<Elevated>,
}

/// Lo que mantiene abierto el helper elevado.
#[cfg(windows)]
type Elevated = framerush_platform::Helper;
#[cfg(not(windows))]
type Elevated = ();

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
        _helper: helper,
    }
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
        .invoke_handler(tauri::generate_handler![snapshot, elevate])
        .setup(|app| {
            let state = start(app.handle(), framerush_sensors::detect(), None);
            app.manage(Sensors(Mutex::new(state)));
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
