//! Consultas WMI locales con COM, siguiendo el ejemplo de Microsoft "Getting WMI Data from the
//! Local Computer": `CoInitializeEx`, `CoInitializeSecurity`, `CoCreateInstance(WbemLocator)`,
//! `ConnectServer`, `CoSetProxyBlanket`, `ExecQuery` y `Next` + `Get`.
//!
//! Solo se lee la primera fila de cada consulta. Las clases que se usan se pueden leer sin
//! elevación (comprobado en Windows 11 el 2026-11-22).
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use windows::Win32::Foundation::RPC_E_TOO_LATE;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoInitializeSecurity, CoSetProxyBlanket, CoTaskMemFree, CoUninitialize, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{
    VARIANT, VariantClear, VariantToStringAlloc, VariantToUInt32,
};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_INFINITE, WbemLocator,
};
use windows::core::{BSTR, HSTRING};

fn os(e: windows::core::Error) -> PlatformError {
    PlatformError::Os(format!("WMI: {e}"))
}

/// COM inicializado en este hilo mientras vive; `CoUninitialize` al soltarlo.
pub(super) struct Com;

impl Com {
    pub(super) fn init() -> Result<Self> {
        // SAFETY: inicializa COM en el hilo actual; S_FALSE (ya inicializado) también cuenta y
        // también pide `CoUninitialize`.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(os)?;
        // La seguridad de COM es de todo el proceso y solo se puede poner una vez: si ya está
        // puesta (RPC_E_TOO_LATE) se usa la que hay.
        // SAFETY: parámetros del ejemplo de Microsoft; los punteros opcionales van a None.
        let security = unsafe {
            CoInitializeSecurity(
                None,
                -1,
                None,
                None,
                RPC_C_AUTHN_LEVEL_DEFAULT,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
                None,
            )
        };
        match security {
            Err(e) if e.code() != RPC_E_TOO_LATE => {
                // SAFETY: equilibra el CoInitializeEx de arriba.
                unsafe { CoUninitialize() };
                Err(os(e))
            }
            _ => Ok(Com),
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: equilibra el CoInitializeEx de `init` en el mismo hilo.
        unsafe { CoUninitialize() };
    }
}

/// Propiedad de una fila; se libera al soltarla.
struct Value(VARIANT);

impl Drop for Value {
    fn drop(&mut self) {
        // SAFETY: el VARIANT lo rellenó `Get` y no se usa después.
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

fn get(obj: &IWbemClassObject, name: &str) -> Result<Value> {
    let mut v = Value(VARIANT::default());
    // SAFETY: `obj` es válido y `v.0` vive durante la llamada.
    unsafe { obj.Get(&HSTRING::from(name), 0, &mut v.0, None, None) }.map_err(os)?;
    Ok(v)
}

/// Cadena de la propiedad, o `None` si es nula.
pub(super) fn string(obj: &IWbemClassObject, name: &str) -> Result<Option<String>> {
    let v = get(obj, name)?;
    // SAFETY: `v.0` es un VARIANT válido; la cadena la reserva el sistema con CoTaskMemAlloc y
    // se libera después de copiarla.
    let Ok(raw) = (unsafe { VariantToStringAlloc(&v.0) }) else {
        return Ok(None);
    };
    // SAFETY: `raw` es una cadena terminada en NUL que devolvió la llamada anterior.
    let text = unsafe { raw.to_string() }.ok();
    // SAFETY: `raw` se reservó con CoTaskMemAlloc (documentado) y ya se copió.
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    Ok(text.filter(|t| !t.trim().is_empty()))
}

pub(super) fn u32(obj: &IWbemClassObject, name: &str) -> Result<Option<u32>> {
    let v = get(obj, name)?;
    // SAFETY: `v.0` es un VARIANT válido.
    Ok(unsafe { VariantToUInt32(&v.0) }.ok())
}

/// Primera fila de `query` en `namespace`, leída con `read`; `None` si no hay filas. Para
/// clases de una sola instancia (`Win32_OperatingSystem`, `Win32_ComputerSystem`…).
pub(super) fn first<T>(
    namespace: &str,
    query: &str,
    read: impl FnMut(&IWbemClassObject) -> Result<T>,
) -> Result<Option<T>> {
    rows(namespace, query, read).map(|rows| rows.into_iter().next())
}

/// Todas las filas de `query` en `namespace`, leídas con `read`.
pub(super) fn rows<T>(
    namespace: &str,
    query: &str,
    mut read: impl FnMut(&IWbemClassObject) -> Result<T>,
) -> Result<Vec<T>> {
    let _com = Com::init()?;
    // SAFETY: COM está inicializado en este hilo (`_com`); el crate `windows` gestiona las
    // referencias de las interfaces.
    let locator: IWbemLocator =
        unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }.map_err(os)?;
    let empty = BSTR::new();
    // SAFETY: `locator` es válido; usuario y contraseña vacíos = usuario actual (documentado).
    let services = unsafe {
        locator.ConnectServer(
            &BSTR::from(namespace),
            &empty,
            &empty,
            &empty,
            0,
            &empty,
            None,
        )
    }
    .map_err(os)?;
    // SAFETY: parámetros del ejemplo de Microsoft sobre un proxy válido.
    unsafe {
        CoSetProxyBlanket(
            &services,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
    }
    .map_err(os)?;
    // SAFETY: `services` es válido.
    let rows = unsafe {
        services.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(query),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )
    }
    .map_err(os)?;
    let mut values = vec![];
    loop {
        let mut row = [None];
        let mut returned = 0u32;
        // SAFETY: `rows` es válido y `row` tiene sitio para un objeto. Al acabar devuelve
        // WBEM_S_FALSE (no es un error) con `returned` a 0.
        unsafe { rows.Next(WBEM_INFINITE, &mut row, &mut returned) }
            .ok()
            .map_err(os)?;
        match row[0].take().filter(|_| returned == 1) {
            Some(obj) => values.push(read(&obj)?),
            None => return Ok(values),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_build_number() {
        let build = first(
            r"ROOT\CIMV2",
            "SELECT BuildNumber FROM Win32_OperatingSystem",
            |o| string(o, "BuildNumber"),
        )
        .unwrap()
        .flatten()
        .unwrap();
        assert!(build.parse::<u32>().is_ok(), "{build}");
    }
}
