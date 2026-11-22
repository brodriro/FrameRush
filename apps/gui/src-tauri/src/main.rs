//! GUI de FrameRush: una ventana de 420 px sin bordes y transparente con el frontend de
//! `apps/gui/ui` (diseño final v3). Los datos aún son de muestra (`ui/api.js`); los comandos
//! y eventos del contrato de `design/README.md` llegan en las siguientes tareas de M3.

// Sin consola en las compilaciones de release de Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    tauri::Builder::default().run(tauri::generate_context!())?;
    Ok(())
}
