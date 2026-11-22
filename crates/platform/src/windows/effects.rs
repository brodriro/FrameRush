//! `visual-effects-performance` con `SystemParametersInfo` (PLAN.md V21).
//!
//! El valor `VisualFXSetting` del registro solo recuerda la opción del diálogo: el diálogo
//! aplica los efectos con `SystemParametersInfo`, y escribir el registro no los cambia. Aquí se
//! apagan, con la API documentada, cuatro efectos que elige FrameRush (Microsoft no publica qué
//! apaga "mejor rendimiento"): efectos de la interfaz, animaciones del área cliente, animación
//! al minimizar y restaurar, y arrastrar ventanas con contenido. `SPIF_UPDATEINIFILE` los
//! guarda en el perfil y `SPIF_SENDCHANGE` avisa a las ventanas, así que el efecto es inmediato.
#![allow(unsafe_code)]

use crate::{PlatformError, Result, SavedData, SavedValue};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ANIMATIONINFO, SPI_GETANIMATION, SPI_GETCLIENTAREAANIMATION, SPI_GETDRAGFULLWINDOWS,
    SPI_GETUIEFFECTS, SPI_SETANIMATION, SPI_SETCLIENTAREAANIMATION, SPI_SETDRAGFULLWINDOWS,
    SPI_SETUIEFFECTS, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SYSTEM_PARAMETERS_INFO_ACTION,
    SystemParametersInfoW,
};

/// Cómo pasa cada parámetro su valor al fijarlo (cada `SPI_SET*` lo documenta a su manera).
#[derive(Debug, Clone, Copy)]
enum Pass {
    /// BOOL en `pvParam`.
    BoolInPv,
    /// BOOL en `uiParam`.
    BoolInUi,
    /// `ANIMATIONINFO` con `iMinAnimate`.
    Animation,
}

#[derive(Debug, Clone, Copy)]
struct Effect {
    name: &'static str,
    get: SYSTEM_PARAMETERS_INFO_ACTION,
    set: SYSTEM_PARAMETERS_INFO_ACTION,
    pass: Pass,
}

const EFFECTS: [Effect; 4] = [
    Effect {
        name: "UIEFFECTS",
        get: SPI_GETUIEFFECTS,
        set: SPI_SETUIEFFECTS,
        pass: Pass::BoolInPv,
    },
    Effect {
        name: "CLIENTAREAANIMATION",
        get: SPI_GETCLIENTAREAANIMATION,
        set: SPI_SETCLIENTAREAANIMATION,
        pass: Pass::BoolInPv,
    },
    Effect {
        name: "ANIMATION",
        get: SPI_GETANIMATION,
        set: SPI_SETANIMATION,
        pass: Pass::Animation,
    },
    Effect {
        name: "DRAGFULLWINDOWS",
        get: SPI_GETDRAGFULLWINDOWS,
        set: SPI_SETDRAGFULLWINDOWS,
        pass: Pass::BoolInUi,
    },
];

impl Effect {
    fn location(&self) -> String {
        format!(r"SystemParametersInfo\{}", self.name)
    }

    /// 1 encendido, 0 apagado.
    fn read(&self) -> Result<u32> {
        let (ok, on) = match self.pass {
            Pass::Animation => {
                let mut info = animation_info(0);
                // SAFETY: `info` es un ANIMATIONINFO vivo con `cbSize` puesto, como pide la API.
                let ok = unsafe {
                    SystemParametersInfoW(self.get, info.cbSize, (&raw mut info).cast(), 0)
                };
                (ok, info.iMinAnimate)
            }
            Pass::BoolInPv | Pass::BoolInUi => {
                let mut on = 0i32;
                // SAFETY: `on` es un BOOL vivo que recibe el valor.
                let ok = unsafe { SystemParametersInfoW(self.get, 0, (&raw mut on).cast(), 0) };
                (ok, on)
            }
        };
        if ok == 0 {
            return Err(os_error(self.name));
        }
        Ok(u32::from(on != 0))
    }

    fn write(&self, on: u32) -> Result<()> {
        let flags = SPIF_UPDATEINIFILE | SPIF_SENDCHANGE;
        let on = u32::from(on != 0);
        // SAFETY: cada rama pasa el valor como lo documenta su `SPI_SET*`: un BOOL en `pvParam`
        // (convertido a puntero, no se desreferencia), un BOOL en `uiParam` o un ANIMATIONINFO
        // vivo con su tamaño.
        let ok = unsafe {
            match self.pass {
                Pass::BoolInPv => SystemParametersInfoW(self.set, 0, on as usize as *mut _, flags),
                Pass::BoolInUi => SystemParametersInfoW(self.set, on, std::ptr::null_mut(), flags),
                Pass::Animation => {
                    let mut info = animation_info(on as i32);
                    SystemParametersInfoW(self.set, info.cbSize, (&raw mut info).cast(), flags)
                }
            }
        };
        if ok == 0 {
            return Err(os_error(self.name));
        }
        Ok(())
    }
}

fn animation_info(min_animate: i32) -> ANIMATIONINFO {
    ANIMATIONINFO {
        cbSize: size_of::<ANIMATIONINFO>() as u32,
        iMinAnimate: min_animate,
    }
}

fn os_error(what: &str) -> PlatformError {
    PlatformError::Os(format!(
        "SystemParametersInfo {what}: {}",
        std::io::Error::last_os_error()
    ))
}

/// Estado actual de los cuatro efectos.
pub(super) fn read() -> Result<Vec<SavedValue>> {
    EFFECTS
        .iter()
        .map(|e| {
            Ok(SavedValue {
                location: e.location(),
                value: Some(SavedData::Number(e.read()?)),
            })
        })
        .collect()
}

/// Los apaga.
pub(super) fn apply() -> Result<()> {
    EFFECTS.iter().try_for_each(|e| e.write(0))
}

/// Devuelve cada efecto a lo guardado.
pub(super) fn restore(saved: &[SavedValue]) -> Result<()> {
    for e in &EFFECTS {
        let location = e.location();
        let Some(SavedValue {
            value: Some(SavedData::Number(on)),
            ..
        }) = saved.iter().find(|s| s.location == location)
        else {
            return Err(PlatformError::Os(format!(
                "el journal no tiene el valor previo de {location}"
            )));
        };
        e.write(*on)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Solo lee: los tests no cambian la configuración del usuario.
    #[test]
    fn reads_every_effect_as_on_or_off() {
        let values = read().unwrap();
        assert_eq!(values.len(), EFFECTS.len());
        for v in values {
            assert!(matches!(v.value, Some(SavedData::Number(0 | 1))), "{v:?}");
        }
    }
}
