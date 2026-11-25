//! Monitores y frecuencia de refresco (`refresh-rate-max`) con `EnumDisplayDevicesW`,
//! `EnumDisplaySettingsW` y `ChangeDisplaySettingsExW` (user32).
//!
//! Un monitor es un adaptador con `DISPLAY_DEVICE_ATTACHED_TO_DESKTOP`; su id es el
//! `DeviceName` (`\\.\DISPLAY1`), que es lo que aceptan las otras dos funciones. Los modos que se
//! ofrecen tienen la misma profundidad de color que el actual y no son entrelazados, como en la
//! referencia CS2-FPS-Boost. El cambio usa `CDS_UPDATEREGISTRY` (perfil del usuario, sin
//! elevación), también como la referencia; el journal lo devuelve al revertir.
#![allow(unsafe_code)]

use crate::{Display, Mode, PlatformError, Result};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Graphics::Gdi::{
    CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_RESTART,
    DISP_CHANGE_SUCCESSFUL, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE,
    DISPLAY_DEVICEW, DM_DISPLAYFREQUENCY, DM_INTERLACED, ENUM_CURRENT_SETTINGS,
    EnumDisplayDevicesW, EnumDisplaySettingsW,
};

/// `DeviceName` de los adaptadores del escritorio, terminados en NUL, y si es el principal.
fn desktop_devices() -> Vec<(Vec<u16>, bool)> {
    let mut names = vec![];
    for index in 0.. {
        let mut dd = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `dd` vive durante la llamada y tiene `cb` inicializado (documentado); un
        // `lpDevice` nulo enumera adaptadores.
        if unsafe { EnumDisplayDevicesW(null(), index, &mut dd, 0) } == 0 {
            break;
        }
        if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0 {
            let len = dd.DeviceName.iter().position(|&c| c == 0).unwrap_or(32);
            let primary = dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0;
            names.push((
                dd.DeviceName[..len].iter().copied().chain([0]).collect(),
                primary,
            ));
        }
    }
    names
}

fn devmode() -> DEVMODEW {
    DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        dmDriverExtra: 0,
        ..Default::default()
    }
}

/// Modo `index` (o `ENUM_CURRENT_SETTINGS`) del dispositivo.
fn mode(device: &[u16], index: u32) -> Option<DEVMODEW> {
    let mut dm = devmode();
    // SAFETY: `device` termina en NUL y `dm` tiene `dmSize` y `dmDriverExtra` puestos
    // (documentado); los dos viven durante la llamada.
    (unsafe { EnumDisplaySettingsW(device.as_ptr(), index, &mut dm) } != 0).then_some(dm)
}

fn interlaced(dm: &DEVMODEW) -> bool {
    // SAFETY: para pantallas, la unión guarda `dmDisplayFlags` (la rellena EnumDisplaySettings).
    let flags = unsafe { dm.Anonymous2.dmDisplayFlags };
    flags & DM_INTERLACED != 0
}

fn to_mode(dm: &DEVMODEW) -> Mode {
    Mode {
        width: dm.dmPelsWidth,
        height: dm.dmPelsHeight,
        hz: dm.dmDisplayFrequency,
    }
}

fn name(device: &[u16]) -> String {
    String::from_utf16_lossy(&device[..device.len().saturating_sub(1)])
}

/// Modos con la profundidad de color del actual, sin entrelazados ni repetidos.
fn modes(device: &[u16], current: &DEVMODEW) -> Vec<Mode> {
    let mut modes: Vec<Mode> = (0..)
        .map_while(|i| mode(device, i))
        .filter(|dm| dm.dmBitsPerPel == current.dmBitsPerPel && !interlaced(dm))
        .map(|dm| to_mode(&dm))
        .collect();
    modes.sort_by_key(|m| (m.width, m.height, m.hz));
    modes.dedup();
    modes
}

/// Modo actual de cada monitor del escritorio, sin listar sus modos: barato para comparar a
/// menudo (los cambios de solo Hz no llegan por `WM_DISPLAYCHANGE`).
pub(super) fn current_modes() -> Vec<(String, Mode)> {
    desktop_devices()
        .into_iter()
        .filter_map(|(device, _)| {
            let current = mode(&device, ENUM_CURRENT_SETTINGS)?;
            Some((name(&device), to_mode(&current)))
        })
        .collect()
}

pub(super) fn displays() -> Result<Vec<Display>> {
    Ok(desktop_devices()
        .into_iter()
        .filter_map(|(device, primary)| {
            let current = mode(&device, ENUM_CURRENT_SETTINGS)?;
            Some(Display {
                id: name(&device),
                current: to_mode(&current),
                modes: modes(&device, &current),
                primary,
            })
        })
        .collect())
}

pub(super) fn set_refresh_rate(id: &str, hz: u32) -> Result<()> {
    let device = desktop_devices()
        .into_iter()
        .map(|(d, _)| d)
        .find(|d| name(d) == id)
        .ok_or_else(|| PlatformError::NotFound(format!("pantalla {id}")))?;
    let mut current = mode(&device, ENUM_CURRENT_SETTINGS)
        .ok_or_else(|| PlatformError::Os(format!("no se pudo leer el modo actual de {id}")))?;
    let (w, h) = (current.dmPelsWidth, current.dmPelsHeight);
    if !modes(&device, &current)
        .iter()
        .any(|m| m.width == w && m.height == h && m.hz == hz)
    {
        return Err(PlatformError::Os(format!(
            "{id} no admite {w}x{h} a {hz} Hz"
        )));
    }
    current.dmDisplayFrequency = hz;
    current.dmFields = DM_DISPLAYFREQUENCY;
    // SAFETY: `device` termina en NUL y `current` es un DEVMODEW completo; los dos viven durante
    // la llamada. `hwnd` y `lParam` son nulos (documentado).
    let code = unsafe {
        ChangeDisplaySettingsExW(
            device.as_ptr(),
            &current,
            null_mut(),
            CDS_UPDATEREGISTRY,
            null(),
        )
    };
    match code {
        DISP_CHANGE_SUCCESSFUL => Ok(()),
        DISP_CHANGE_RESTART => Err(PlatformError::Os(format!(
            "{id}: el cambio a {hz} Hz necesita reiniciar"
        ))),
        other => Err(PlatformError::Os(format!(
            "{id}: ChangeDisplaySettingsExW devolvió {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solo lee. En la CI puede no haber monitores; si los hay, el modo actual está en la lista.
    #[test]
    fn the_current_mode_is_listed() {
        for d in displays().unwrap() {
            assert!(d.id.starts_with(r"\\.\"), "{}", d.id);
            assert!(d.modes.contains(&d.current), "{d:?}");
            assert!(d.max_hz_at_current_resolution() >= Some(d.current.hz));
        }
    }

    /// Volver a poner la frecuencia actual no cambia nada y no pide elevación.
    #[test]
    fn setting_the_current_rate_works_and_unknown_rates_are_refused() {
        for d in displays().unwrap() {
            set_refresh_rate(&d.id, d.current.hz).unwrap();
            assert!(set_refresh_rate(&d.id, 1).is_err());
        }
        assert!(set_refresh_rate(r"\\.\NO-EXISTE", 60).is_err());
    }
}
