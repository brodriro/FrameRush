//! Valores DWORD de `HKEY_CURRENT_USER`: lo que necesitan los ajustes de nivel Normal.
//!
//! Solo HKCU (no pide admin). HKLM llegará con el helper elevado.
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, RRF_RT_REG_DWORD,
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
};

/// UTF-16 terminado en nulo.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn error(code: u32, path: &str, name: &str) -> PlatformError {
    let what = format!(r"HKCU\{path}\{name}");
    if code == ERROR_ACCESS_DENIED {
        PlatformError::AccessDenied(what)
    } else {
        let e = std::io::Error::from_raw_os_error(code as i32);
        PlatformError::Os(format!("{what}: {e}"))
    }
}

/// Valor actual, o `None` si la clave o el valor no existen.
pub(super) fn read_dword(path: &str, name: &str) -> Result<Option<u32>> {
    let (p, n) = (wide(path), wide(name));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: las cadenas terminan en nulo y viven durante la llamada; `data` y `size` son
    // locales vivos y `size` indica el tamaño real de `data`.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    match code {
        ERROR_SUCCESS => Ok(Some(data)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        c => Err(error(c, path, name)),
    }
}

/// Escribe el valor y crea la clave si no existe.
pub(super) fn write_dword(path: &str, name: &str, value: u32) -> Result<()> {
    let (p, n) = (wide(path), wide(name));
    let mut key: HKEY = null_mut();
    // SAFETY: `p` termina en nulo; los punteros opcionales son nulos y `key` es un local vivo.
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if code != ERROR_SUCCESS {
        return Err(error(code, path, name));
    }
    let bytes = value.to_le_bytes();
    // SAFETY: `key` es la clave recién abierta; `bytes` tiene los 4 bytes que se declaran.
    let code = unsafe {
        RegSetValueExW(
            key,
            n.as_ptr(),
            0,
            REG_DWORD,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    // SAFETY: `key` se abrió arriba y no se usa después.
    unsafe { RegCloseKey(key) };
    match code {
        ERROR_SUCCESS => Ok(()),
        c => Err(error(c, path, name)),
    }
}

/// Borra el valor. Que no exista no es un error.
pub(super) fn delete_value(path: &str, name: &str) -> Result<()> {
    let (p, n) = (wide(path), wide(name));
    // SAFETY: las dos cadenas terminan en nulo y viven durante la llamada.
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, p.as_ptr(), n.as_ptr()) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        c => Err(error(c, path, name)),
    }
}

#[cfg(test)]
mod tests {
    //! Solo tocan una clave propia del test, que se borra al terminar.

    use super::*;
    use windows_sys::Win32::System::Registry::RegDeleteKeyW;

    struct TestKey(String);

    impl Drop for TestKey {
        fn drop(&mut self) {
            let p = wide(&self.0);
            // SAFETY: `p` termina en nulo. La clave del test no tiene subclaves.
            unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, p.as_ptr()) };
        }
    }

    fn test_key(name: &str) -> TestKey {
        TestKey(format!(
            r"Software\FrameRush-test-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn write_read_and_delete_a_value() {
        let key = test_key("rw");
        assert_eq!(read_dword(&key.0, "V").unwrap(), None);
        write_dword(&key.0, "V", 7).unwrap();
        assert_eq!(read_dword(&key.0, "V").unwrap(), Some(7));
        write_dword(&key.0, "V", 0).unwrap();
        assert_eq!(read_dword(&key.0, "V").unwrap(), Some(0));
        delete_value(&key.0, "V").unwrap();
        assert_eq!(read_dword(&key.0, "V").unwrap(), None);
        delete_value(&key.0, "V").unwrap();
    }

    #[test]
    fn a_missing_key_reads_as_none() {
        let key = test_key("missing");
        assert_eq!(read_dword(&key.0, "V").unwrap(), None);
        delete_value(&key.0, "V").unwrap();
    }
}
