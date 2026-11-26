//! Avisos de cambio de hardware (PLAN.md Iteración 6.1): pantalla, alimentación y dispositivos.
//!
//! Una ventana de nivel superior oculta en su propio hilo recibe los mensajes que Windows difunde
//! a todas las ventanas de nivel superior. No sirve una ventana «solo mensajes» (`HWND_MESSAGE`):
//! según Microsoft Learn («Window Features»), no recibe mensajes difundidos.
//!
//! - `WM_DISPLAYCHANGE`: cambió la resolución (Microsoft Learn: «WM_DISPLAYCHANGE message»).
//! - Cambios de solo Hz: Windows no los avisa (ni `WM_DISPLAYCHANGE`, comprobado en real, ni un
//!   evento documentado), así que un temporizador de la ventana compara el modo actual de cada
//!   monitor cada [`DISPLAY_POLL_MS`] y avisa `Display` si cambió.
//! - `WM_POWERBROADCAST` + `PBT_APMPOWERSTATUSCHANGE`: CA ↔ batería o cambio de batería
//!   (Microsoft Learn: «PBT_APMPOWERSTATUSCHANGE event»).
//! - `WM_DEVICECHANGE` + `DBT_DEVNODES_CHANGED`: se añadió o quitó un dispositivo, sin decir cuál
//!   (Microsoft Learn: «DBT_DEVNODES_CHANGED event»).

use super::display::current_modes;
use super::helper::wide;
use crate::{HardwareChange, Mode, PlatformError, Result};
use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DBT_DEVNODES_CHANGED, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, KillTimer, MSG, PBT_APMPOWERSTATUSCHANGE, PostMessageW, PostQuitMessage,
    RegisterClassExW, SetTimer, UnregisterClassW, WM_CLOSE, WM_DESTROY, WM_DEVICECHANGE,
    WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_TIMER, WNDCLASSEXW,
};

type Callback = Box<dyn Fn(HardwareChange)>;

thread_local! {
    /// El aviso del hilo de la ventana: el procedimiento de ventana corre en ese mismo hilo.
    static ON_CHANGE: RefCell<Option<Callback>> = const { RefCell::new(None) };
    /// Último modo visto de cada monitor, para el temporizador.
    static MODES: RefCell<Vec<(String, Mode)>> = const { RefCell::new(Vec::new()) };
}

/// Cada cuánto se compara el modo de los monitores.
const DISPLAY_POLL_MS: u32 = 2000;
const DISPLAY_TIMER: usize = 1;

/// Guarda el modo actual y dice si es distinto del anterior.
fn modes_changed() -> bool {
    let now = current_modes();
    MODES.with(|m| {
        let changed = *m.borrow() != now;
        *m.borrow_mut() = now;
        changed
    })
}

/// Ventana que vigila. Soltarla cierra la ventana y espera a su hilo.
#[derive(Debug)]
pub struct HardwareWatch {
    /// `HWND` como entero: el puntero no es `Send`.
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl Drop for HardwareWatch {
    fn drop(&mut self) {
        // SAFETY: la ventana es de este módulo; si ya no existe, PostMessage solo falla.
        unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const CLASS: &str = "FrameRushHardwareWatch";

/// Empieza a vigilar; `on_change` se llama desde el hilo de la ventana con cada aviso.
pub fn watch_hardware(
    on_change: impl Fn(HardwareChange) + Send + 'static,
) -> Result<HardwareWatch> {
    let (ready, created) = mpsc::channel::<std::result::Result<isize, String>>();
    let thread = std::thread::spawn(move || {
        ON_CHANGE.with(|c| *c.borrow_mut() = Some(Box::new(on_change)));
        modes_changed();
        let class = wide(CLASS);
        // SAFETY: NULL = el módulo del proceso.
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            // SAFETY: WNDCLASSEXW es una estructura C; el resto de campos a cero es válido.
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: `wc` y `class` viven durante la llamada.
        if unsafe { RegisterClassExW(&wc) } == 0 {
            let _ = ready.send(Err(std::io::Error::last_os_error().to_string()));
            return;
        }
        // SAFETY: clase registrada arriba; estilo 0 (superpuesta), sin padre ni menú y nunca se
        // muestra: es de nivel superior para recibir los mensajes difundidos.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            let _ = ready.send(Err(std::io::Error::last_os_error().to_string()));
            // SAFETY: la clase es de este hilo y no tiene ventanas.
            unsafe { UnregisterClassW(class.as_ptr(), instance) };
            return;
        }
        // SAFETY: la ventana es de este hilo; sin TIMERPROC llega WM_TIMER a su procedimiento.
        // Si falla, solo se pierden los cambios de solo Hz.
        unsafe { SetTimer(hwnd, DISPLAY_TIMER, DISPLAY_POLL_MS, None) };
        let _ = ready.send(Ok(hwnd as isize));
        // SAFETY: MSG es una estructura C; cero es un valor inicial válido.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: `msg` es válido; GetMessage devuelve 0 con WM_QUIT y -1 si falla.
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
            // SAFETY: `msg` lo acaba de rellenar GetMessage.
            unsafe { DispatchMessageW(&msg) };
        }
        // SAFETY: la ventana ya se destruyó (WM_QUIT llega tras WM_DESTROY).
        unsafe { UnregisterClassW(class.as_ptr(), instance) };
    });
    match created.recv() {
        Ok(Ok(hwnd)) => Ok(HardwareWatch {
            hwnd,
            thread: Some(thread),
        }),
        Ok(Err(e)) => Err(PlatformError::Os(format!(
            "ventana de avisos de hardware: {e}"
        ))),
        Err(_) => Err(PlatformError::Os(
            "ventana de avisos de hardware: el hilo terminó".into(),
        )),
    }
}

fn notify(change: HardwareChange) {
    ON_CHANGE.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f(change);
        }
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DISPLAYCHANGE => {
            // Se guarda el modo nuevo para que el temporizador no avise otra vez.
            modes_changed();
            notify(HardwareChange::Display);
        }
        WM_TIMER if wparam == DISPLAY_TIMER => {
            if modes_changed() {
                notify(HardwareChange::Display);
            }
            return 0;
        }
        WM_POWERBROADCAST if wparam == PBT_APMPOWERSTATUSCHANGE as usize => {
            notify(HardwareChange::Power)
        }
        WM_DEVICECHANGE if wparam == DBT_DEVNODES_CHANGED as usize => {
            notify(HardwareChange::Devices);
            // Documentado: DBT_DEVNODES_CHANGED devuelve TRUE.
            return 1;
        }
        WM_CLOSE => {
            // SAFETY: la ventana y el temporizador son de este hilo.
            unsafe {
                KillTimer(hwnd, DISPLAY_TIMER);
                DestroyWindow(hwnd);
            }
            return 0;
        }
        WM_DESTROY => {
            // SAFETY: termina el bucle de mensajes de este hilo.
            unsafe { PostQuitMessage(0) };
            return 0;
        }
        _ => {}
    }
    // SAFETY: los parámetros son los que recibió el procedimiento.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_modes_are_not_a_change() {
        modes_changed();
        assert!(!modes_changed());
        MODES.with(|m| m.borrow_mut().clear());
        // En la CI puede no haber monitores: entonces no hay cambio.
        assert_eq!(modes_changed(), !current_modes().is_empty());
    }

    #[test]
    fn watch_starts_and_stops() {
        // Dos seguidas: soltar la primera desregistra la clase y la segunda puede volver a crearla.
        for _ in 0..2 {
            let watch = watch_hardware(|_| {}).expect("ventana de avisos");
            drop(watch);
        }
    }
}
