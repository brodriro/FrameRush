//! NVAPI de NVIDIA (SDK público, MIT: <https://github.com/NVIDIA/nvapi>), solo para leer el
//! tacómetro del ventilador de la GPU (`NvAPI_GPU_GetTachReading`). Es el último recurso de los
//! sensores: se usa cuando ninguna otra fuente (NVML incluida) encuentra ventiladores.
//!
//! `nvapi64.dll` la instala el driver de NVIDIA y exporta `nvapi_QueryInterface`, que da cada
//! función por su id de `nvapi_interface.h`. Constantes y firmas de `nvapi.h` y
//! `nvapi_lite_common.h` (comprobadas en el repositorio de NVIDIA el 2026-11-22).
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// Ids de `nvapi_interface.h`.
const ID_INITIALIZE: u32 = 0x0150_e828;
const ID_UNLOAD: u32 = 0xd22b_dd7e;
const ID_ENUM_PHYSICAL_GPUS: u32 = 0xe5ac_921f;
const ID_GET_FULL_NAME: u32 = 0xceee_8e9f;
const ID_GET_TACH_READING: u32 = 0x5f60_8315;

/// `nvapi_lite_common.h`.
const NVAPI_OK: i32 = 0;
const NVAPI_NOT_SUPPORTED: i32 = -104;
const NVAPI_MAX_PHYSICAL_GPUS: usize = 64;
const NVAPI_SHORT_STRING_MAX: usize = 64;

type Handle = *mut c_void;
type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type NoArgs = unsafe extern "C" fn() -> i32;
type EnumPhysicalGpus =
    unsafe extern "C" fn(*mut [Handle; NVAPI_MAX_PHYSICAL_GPUS], *mut u32) -> i32;
type GetFullName = unsafe extern "C" fn(Handle, *mut [u8; NVAPI_SHORT_STRING_MAX]) -> i32;
type GetTachReading = unsafe extern "C" fn(Handle, *mut u32) -> i32;

/// NVAPI cargada e inicializada, con sus GPU físicas.
pub struct Nvapi {
    module: HMODULE,
    unload: Option<NoArgs>,
    tach: GetTachReading,
    gpus: Vec<(Handle, String)>,
}

// SAFETY: los handles de GPU de NVAPI son identificadores opacos válidos en todo el proceso, y
// `Nvapi` solo se usa desde un hilo a la vez (los métodos que llaman a NVAPI piden `&self` y el
// sondeo de sensores lo hace desde un único hilo).
unsafe impl Send for Nvapi {}

impl std::fmt::Debug for Nvapi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nvapi")
            .field("gpus", &self.gpu_names())
            .finish()
    }
}

fn os(what: &str, status: i32) -> PlatformError {
    PlatformError::Os(format!("NVAPI {what}: estado {status}"))
}

impl Nvapi {
    /// Carga `nvapi64.dll`; falla sin driver de NVIDIA.
    pub fn open() -> Result<Self> {
        let name: Vec<u16> = "nvapi64.dll\0".encode_utf16().collect();
        // SAFETY: `name` es una cadena UTF-16 terminada en nulo.
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module.is_null() {
            return Err(PlatformError::Unsupported("NVAPI (sin driver de NVIDIA)"));
        }
        // SAFETY: `module` es válido y el nombre es ASCII terminado en nulo.
        let query = unsafe { GetProcAddress(module, c"nvapi_QueryInterface".as_ptr().cast()) };
        let Some(query) = query else {
            // SAFETY: `module` lo cargó `LoadLibraryW` y no se usa después.
            unsafe { FreeLibrary(module) };
            return Err(PlatformError::Unsupported("NVAPI sin nvapi_QueryInterface"));
        };
        // SAFETY: `nvapi_QueryInterface` tiene la firma `void* (unsigned int)`.
        let query: QueryInterface = unsafe { std::mem::transmute(query) };
        // SAFETY: cada id es de `nvapi_interface.h` y el puntero devuelto, si no es nulo, es
        // esa función con la firma de `nvapi.h`.
        let func = |id: u32| unsafe { query(id) };
        let get = |id| {
            let p = func(id);
            (!p.is_null()).then_some(p)
        };
        let (Some(init), Some(enumerate), Some(tach)) = (
            get(ID_INITIALIZE),
            get(ID_ENUM_PHYSICAL_GPUS),
            get(ID_GET_TACH_READING),
        ) else {
            // SAFETY: como arriba.
            unsafe { FreeLibrary(module) };
            return Err(PlatformError::Unsupported(
                "NVAPI sin las funciones del tacómetro",
            ));
        };
        // SAFETY: punteros a funciones de NVAPI con las firmas de `nvapi.h`.
        let (init, enumerate, tach, unload, name_of) = unsafe {
            (
                std::mem::transmute::<*mut c_void, NoArgs>(init),
                std::mem::transmute::<*mut c_void, EnumPhysicalGpus>(enumerate),
                std::mem::transmute::<*mut c_void, GetTachReading>(tach),
                get(ID_UNLOAD).map(|p| std::mem::transmute::<*mut c_void, NoArgs>(p)),
                get(ID_GET_FULL_NAME).map(|p| std::mem::transmute::<*mut c_void, GetFullName>(p)),
            )
        };
        let mut nvapi = Nvapi {
            module,
            unload: None,
            tach,
            gpus: vec![],
        };
        // SAFETY: `NvAPI_Initialize()` no tiene argumentos.
        let status = unsafe { init() };
        if status != NVAPI_OK {
            return Err(os("NvAPI_Initialize", status));
        }
        nvapi.unload = unload;
        let mut handles = [std::ptr::null_mut(); NVAPI_MAX_PHYSICAL_GPUS];
        let mut count = 0u32;
        // SAFETY: el array tiene `NVAPI_MAX_PHYSICAL_GPUS` entradas, como pide la firma.
        let status = unsafe { enumerate(&mut handles, &mut count) };
        if status != NVAPI_OK {
            return Err(os("NvAPI_EnumPhysicalGPUs", status));
        }
        let count = (count as usize).min(NVAPI_MAX_PHYSICAL_GPUS);
        nvapi.gpus = handles[..count]
            .iter()
            .enumerate()
            .map(|(i, &h)| {
                let mut buf = [0u8; NVAPI_SHORT_STRING_MAX];
                let name = name_of
                    // SAFETY: `buf` es un `NvAPI_ShortString` y `h` un handle de la enumeración.
                    .filter(|f| unsafe { f(h, &mut buf) } == NVAPI_OK)
                    .map(|_| {
                        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
                        String::from_utf8_lossy(&buf[..end]).into_owned()
                    })
                    .unwrap_or_else(|| format!("GPU NVIDIA {i}"));
                (h, name)
            })
            .collect();
        Ok(nvapi)
    }

    /// Nombre de cada GPU física, en el orden de NVAPI.
    pub fn gpu_names(&self) -> Vec<&str> {
        self.gpus.iter().map(|(_, n)| n.as_str()).collect()
    }

    /// RPM del tacómetro de la GPU `index`. `Ok(None)` = la GPU no tiene tacómetro que leer
    /// (`NVAPI_NOT_SUPPORTED`, p. ej. en portátiles).
    pub fn tach_rpm(&self, index: usize) -> Result<Option<u32>> {
        let Some((handle, _)) = self.gpus.get(index) else {
            return Err(PlatformError::NotFound(format!("GPU NVAPI {index}")));
        };
        let mut value = 0u32;
        // SAFETY: `handle` es de `NvAPI_EnumPhysicalGPUs` y `value` es un `NvU32` válido.
        match unsafe { (self.tach)(*handle, &mut value) } {
            NVAPI_OK => Ok(Some(value)),
            NVAPI_NOT_SUPPORTED => Ok(None),
            status => Err(os("NvAPI_GPU_GetTachReading", status)),
        }
    }
}

impl Drop for Nvapi {
    fn drop(&mut self) {
        if let Some(unload) = self.unload {
            // SAFETY: NVAPI se inicializó (si no, `unload` es `None`) y no se usa después.
            unsafe { unload() };
        }
        // SAFETY: `module` lo cargó `LoadLibraryW` en `open`.
        unsafe { FreeLibrary(self.module) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sin driver de NVIDIA (la CI) no abre; con uno, cada GPU responde con RPM o "no tiene".
    #[test]
    fn reads_the_tach_or_says_it_has_none() {
        let Ok(nvapi) = Nvapi::open() else { return };
        for (i, name) in nvapi.gpu_names().iter().enumerate() {
            assert!(!name.is_empty());
            nvapi.tach_rpm(i).unwrap();
        }
        assert!(nvapi.tach_rpm(NVAPI_MAX_PHYSICAL_GPUS).is_err());
    }
}
