//! Valores DWORD y cadena de `HKEY_CURRENT_USER`: lo que necesitan los ajustes de nivel Normal.
//!
//! Solo HKCU (no pide admin). HKLM llegará con el helper elevado.
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteKeyValueW, RegGetValueW, RegSetValueExW,
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

/// Cadena actual (REG_SZ), o `None` si la clave o el valor no existen.
pub(super) fn read_string(path: &str, name: &str) -> Result<Option<String>> {
    let (p, n) = (wide(path), wide(name));
    let mut size = 0u32;
    // SAFETY: las cadenas terminan en nulo; con `pvdata` nulo la función solo escribe en
    // `size` los bytes que hacen falta.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        )
    };
    match code {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        c => return Err(error(c, path, name)),
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` tiene `size` bytes y vive durante la llamada; RRF_RT_REG_SZ garantiza que
    // el resultado termina en nulo.
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    match code {
        ERROR_SUCCESS => {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Ok(Some(String::from_utf16_lossy(&buf[..len])))
        }
        ERROR_FILE_NOT_FOUND => Ok(None),
        c => Err(error(c, path, name)),
    }
}

/// Escribe una cadena (REG_SZ) y crea la clave si no existe.
pub(super) fn write_string(path: &str, name: &str, value: &str) -> Result<()> {
    let data = wide(value);
    let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
    set_value(path, name, REG_SZ, &bytes)
}

/// Escribe el valor y crea la clave si no existe.
pub(super) fn write_dword(path: &str, name: &str, value: u32) -> Result<()> {
    set_value(path, name, REG_DWORD, &value.to_le_bytes())
}

fn set_value(path: &str, name: &str, kind: REG_VALUE_TYPE, bytes: &[u8]) -> Result<()> {
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
    // SAFETY: `key` es la clave recién abierta; `bytes` vive y tiene la longitud declarada.
    let code =
        unsafe { RegSetValueExW(key, n.as_ptr(), 0, kind, bytes.as_ptr(), bytes.len() as u32) };
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

    /// Incluye una ruta con barras como nombre del valor, como en `AppCompatFlags\Layers`.
    #[test]
    fn write_read_and_delete_a_string() {
        let key = test_key("sz");
        let name = r"C:\Games\Some Game\game.exe";
        assert_eq!(read_string(&key.0, name).unwrap(), None);
        write_string(&key.0, name, "~ FLAG").unwrap();
        assert_eq!(
            read_string(&key.0, name).unwrap().as_deref(),
            Some("~ FLAG")
        );
        write_string(&key.0, name, "").unwrap();
        assert_eq!(read_string(&key.0, name).unwrap().as_deref(), Some(""));
        delete_value(&key.0, name).unwrap();
        assert_eq!(read_string(&key.0, name).unwrap(), None);
    }

    #[test]
    fn a_missing_key_reads_as_none() {
        let key = test_key("missing");
        assert_eq!(read_dword(&key.0, "V").unwrap(), None);
        delete_value(&key.0, "V").unwrap();
    }
}
