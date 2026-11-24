//! Overlay (It3.5): atajo global que muestra u oculta la ventana compacta e icono en la bandeja.
//!
//! El atajo (`overlay-hotkey`) lo fija la UI al arrancar y al cambiarlo; vacío = sin atajo. La
//! bandeja tiene «Mostrar u ocultar» y «Salir»; un clic izquierdo en el icono hace lo primero.
//! Los textos del menú los pone la UI en su idioma (`tray_labels`).

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const TOGGLE: &str = "toggle";
const QUIT: &str = "quit";

/// Entradas del menú de la bandeja, para cambiarles el texto.
pub struct TrayMenu {
    toggle: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// Muestra la ventana o la oculta. Con `focused`, una ventana visible pero detrás (el juego en
/// primer plano) pasa al frente en vez de ocultarse.
fn toggle(app: &AppHandle, focused: bool) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let visible = w.is_visible().unwrap_or(false);
    let front = !focused || w.is_focused().unwrap_or(false);
    if visible && front {
        let _ = w.hide();
    } else {
        crate::show_main(app);
    }
}

/// Cambia el atajo global por `accel` (p. ej. `Ctrl+Shift+F`). Error si no es válido o si otro
/// programa ya lo tiene.
pub fn set_hotkey(app: &AppHandle, accel: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    shortcuts.unregister_all().map_err(|e| e.to_string())?;
    let accel = accel.trim();
    if accel.is_empty() {
        return Ok(());
    }
    shortcuts
        .on_shortcut(accel, |app, _, event| {
            if event.state == ShortcutState::Pressed {
                toggle(app, true);
            }
        })
        .map_err(|e| e.to_string())?;
    tracing::info!(accel, "atajo global fijado");
    Ok(())
}

/// Crea el icono de la bandeja con el icono de la app.
pub fn tray(app: &tauri::App) -> tauri::Result<()> {
    let toggle_item = MenuItem::with_id(app, TOGGLE, "Mostrar u ocultar", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, QUIT, "Salir", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle_item, &quit_item])?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("FrameRush")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            tracing::info!(id = event.id.as_ref(), "menú de la bandeja");
            match event.id.as_ref() {
                TOGGLE => toggle(app, false),
                QUIT => crate::quit(app),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle(tray.app_handle(), false);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayMenu {
        toggle: toggle_item,
        quit: quit_item,
    });
    Ok(())
}

/// Textos del menú de la bandeja en el idioma de la UI.
pub fn set_labels(app: &AppHandle, toggle: &str, quit: &str) -> Result<(), String> {
    let menu = app.state::<TrayMenu>();
    menu.toggle.set_text(toggle).map_err(|e| e.to_string())?;
    menu.quit.set_text(quit).map_err(|e| e.to_string())
}
