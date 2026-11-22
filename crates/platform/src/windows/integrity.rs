//! Nivel de integridad del token de un proceso (Mandatory Integrity Control). Un proceso
//! elevado del mismo usuario tiene integridad alta y uno sin elevar no puede tocarlo (P12).
#![allow(unsafe_code)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// RID de integridad del proceso `pid` (`SECURITY_MANDATORY_*_RID`), o `None` si el SO no
/// deja leerlo.
pub(crate) fn integrity_level(pid: u32) -> Option<u32> {
    // SAFETY: OpenProcess no toma punteros; devuelve un handle nuevo o null.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` es un handle nuevo que nos pertenece.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `process` está abierto y `token` es un local vivo.
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    // SAFETY: `token` es un handle nuevo que nos pertenece.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut len = 0u32;
    // SAFETY: con buffer nulo y tamaño 0 solo escribe en `len` el tamaño necesario.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut len,
        )
    };
    if len == 0 {
        return None;
    }
    // Buffer de u64 para que la estructura y el SID que lleva detrás queden alineados.
    let mut buf = vec![0u64; (len as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` tiene al menos `len` bytes y vive durante la llamada.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenIntegrityLevel,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: el SO escribió un TOKEN_MANDATORY_LABEL al principio de `buf`, alineado.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
    // SAFETY: `sid` apunta dentro de `buf`, que sigue vivo; el SO garantiza que es válido.
    let count = unsafe { *GetSidSubAuthorityCount(sid) };
    if count == 0 {
        return None;
    }
    // SAFETY: `count - 1` es una subautoridad que existe en `sid`.
    Some(unsafe { *GetSidSubAuthority(sid, u32::from(count) - 1) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SECURITY_MANDATORY_LOW_RID` y `SECURITY_MANDATORY_SYSTEM_RID` (winnt.h).
    const SECURITY_MANDATORY_LOW_RID: u32 = 0x1000;
    const SECURITY_MANDATORY_SYSTEM_RID: u32 = 0x4000;

    #[test]
    fn reads_the_own_integrity_level() {
        let level = integrity_level(std::process::id()).unwrap();
        assert!(
            (SECURITY_MANDATORY_LOW_RID..=SECURITY_MANDATORY_SYSTEM_RID).contains(&level),
            "{level:#x}"
        );
    }
}
