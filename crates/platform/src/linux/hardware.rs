//! Avisos de cambio de hardware por los uevent del kernel (PLAN.md Iteración 6.1).
//!
//! Se abre un socket `NETLINK_KOBJECT_UEVENT` y se escucha el grupo del kernel (grupo 1), que es
//! lo que imprime `udevadm monitor --kernel`; no hace falta ser root (netlink(7): el grupo 1 de
//! ese protocolo es el que emite el kernel con cada `kobject_uevent`). Cada mensaje lleva el
//! evento (`change@/devices/…`) y líneas `CLAVE=VALOR` separadas por NUL:
//! - `SUBSYSTEM=drm` → [`HardwareChange::Display`]: un monitor conectado o un modo puesto.
//! - `SUBSYSTEM=power_supply` → [`HardwareChange::Power`]: CA, batería o su carga.
//! - `ACTION=add` o `remove` de cualquier otro subsistema → [`HardwareChange::Devices`], como el
//!   `DBT_DEVNODES_CHANGED` de Windows. Un `change` de otros subsistemas no cuenta: el kernel
//!   manda muchos y no significan que se haya tocado el hardware.
//!
//! Un cambio de solo Hz no siempre deja un uevent visible (depende del compositor), así que el
//! hilo compara además el modo actual de cada monitor cada segundo, como el temporizador de
//! Windows.

#![allow(unsafe_code)]

use super::drm;
use crate::{HardwareChange, Mode, PlatformError, Result};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Grupo del kernel de `NETLINK_KOBJECT_UEVENT`.
const KERNEL_GROUP: u32 = 1;

/// Cada cuánto se comparan los modos si no llega ningún aviso.
const POLL: Duration = Duration::from_secs(1);

/// Vigilancia en marcha. Soltarla para el hilo y espera a que termine.
#[derive(Debug)]
pub struct HardwareWatch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for HardwareWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Empieza a vigilar; `on_change` se llama desde el hilo de la vigilancia con cada aviso.
pub fn watch_hardware(
    on_change: impl Fn(HardwareChange) + Send + 'static,
) -> Result<HardwareWatch> {
    let socket = socket().map_err(|e| PlatformError::Os(format!("netlink de udev: {e}")))?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        let fd = socket.as_raw_fd();
        let mut buffer = vec![0u8; 4096];
        let mut last = modes();
        while !flag.load(Ordering::Relaxed) {
            // SAFETY: `buffer` es válido y `recv` no escribe más allá de su longitud.
            let read = unsafe {
                libc::recv(
                    fd,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if read > 0 {
                let text = String::from_utf8_lossy(&buffer[..read as usize]);
                if let Some(change) = change_of(&text) {
                    on_change(change);
                }
                continue;
            }
            std::thread::sleep(POLL);
            let now = modes();
            if now != last {
                last = now;
                on_change(HardwareChange::Display);
            }
        }
    });
    Ok(HardwareWatch {
        stop,
        thread: Some(thread),
    })
}

/// Socket netlink de uevent del kernel, sin bloquear en `recv`.
fn socket() -> std::io::Result<OwnedFd> {
    // SAFETY: socket(2) no toma punteros.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` es un descriptor nuevo que a partir de aquí es del `OwnedFd`.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `sockaddr_nl` es un struct C de solo datos y todo a cero es válido; `nl_pid` a 0
    // deja que el kernel elija el puerto.
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    address.nl_groups = KERNEL_GROUP;
    // SAFETY: `address` es un `sockaddr_nl` y se pasa su tamaño real.
    let bound = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            &address as *const libc::sockaddr_nl as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(socket)
}

/// Qué avisa un uevent del kernel (`change@/devices/…\0ACTION=change\0SUBSYSTEM=drm\0…`).
fn change_of(message: &str) -> Option<HardwareChange> {
    let mut action = None;
    let mut subsystem = None;
    for field in message.split('\0') {
        if let Some(value) = field.strip_prefix("ACTION=") {
            action = Some(value);
        } else if let Some(value) = field.strip_prefix("SUBSYSTEM=") {
            subsystem = Some(value);
        }
    }
    match subsystem {
        Some("drm") => Some(HardwareChange::Display),
        Some("power_supply") => Some(HardwareChange::Power),
        // `bind`/`unbind`/`change` de otros subsistemas no dicen que se tocara el hardware.
        _ => matches!(action, Some("add" | "remove")).then_some(HardwareChange::Devices),
    }
}

/// Modo actual de cada monitor, para detectar los cambios de solo Hz.
fn modes() -> Vec<(String, Mode)> {
    drm::displays()
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.id, d.current))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_kernel_uevents() {
        let drm = "change@/devices/pci0000:00/0000:00:02.0/drm/card1\0ACTION=change\0\
                   DEVPATH=/devices/pci0000:00/0000:00:02.0/drm/card1\0SUBSYSTEM=drm\0\
                   HOTPLUG=1\0";
        assert_eq!(change_of(drm), Some(HardwareChange::Display));
        let power = "change@/devices/LNXSYSTM:00/ACPI0003:00/power_supply/ADP1\0ACTION=change\0\
                     SUBSYSTEM=power_supply\0POWER_SUPPLY_ONLINE=1\0";
        assert_eq!(change_of(power), Some(HardwareChange::Power));
        let usb = "add@/devices/pci0000:00/0000:00:14.0/usb1/1-1\0ACTION=add\0SUBSYSTEM=usb\0";
        assert_eq!(change_of(usb), Some(HardwareChange::Devices));
        let removed = "remove@/devices/pci0000:00/0000:00:14.0/usb1/1-1\0ACTION=remove\0\
                       SUBSYSTEM=usb\0";
        assert_eq!(change_of(removed), Some(HardwareChange::Devices));
        // Un `change` de otro subsistema no es un cambio de hardware, y sin datos tampoco.
        assert_eq!(
            change_of("change@/devices/x\0ACTION=change\0SUBSYSTEM=block\0"),
            None
        );
        assert_eq!(change_of(""), None);
    }

    #[test]
    fn watch_starts_and_stops() {
        for _ in 0..2 {
            let watch = watch_hardware(|_| {}).expect("netlink de udev");
            drop(watch);
        }
    }
}
