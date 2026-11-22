//! GUI de FrameRush: una ventana de 420 px sin bordes y transparente con el frontend de
//! `apps/gui/ui` (diseño final v3). Comandos y eventos del contrato de `design/README.md`:
//!
//! - `snapshot`: el equipo, el plan y la sesión pendiente, leídos de este equipo (R8).
//! - evento `sensors` a 1 Hz con las lecturas de los sensores (`null` = "No disponible").
//!
//! Aplicar, deshacer y restaurar aún no están conectados: la UI lo dice en vez de simularlo.

// Sin consola en las compilaciones de release de Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod snapshot;

use tauri::{Emitter, Manager};

/// Fuentes de sensores que se consultaron al arrancar.
struct SensorSources(Vec<&'static str>);

/// Lee el equipo fuera del hilo de la UI: WMI, NVML y el escaneo de procesos tardan.
#[tauri::command]
async fn snapshot(sources: tauri::State<'_, SensorSources>) -> Result<snapshot::View, String> {
    let checked = sources.0.clone();
    tauri::async_runtime::spawn_blocking(move || snapshot::build(checked))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

fn main() -> anyhow::Result<()> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![snapshot])
        .setup(|app| {
            let handle = app.handle().clone();
            let sources = framerush_sensors::detect();
            app.manage(SensorSources(sources.checked));
            let poller = framerush_sensors::Poller::spawn(
                sources.providers,
                framerush_sensors::POLL_INTERVAL,
                move |sensors| {
                    // Sin ventana que escuche (al cerrar) no hay nada que hacer con el error.
                    let _ = handle.emit("sensors", snapshot::sensor_views(&sensors));
                },
            );
            // El hilo vive lo que la app.
            app.manage(poller);
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
