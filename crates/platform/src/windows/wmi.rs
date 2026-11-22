//! Consultas WMI locales con COM, siguiendo el ejemplo de Microsoft "Getting WMI Data from the
//! Local Computer": `CoInitializeEx`, `CoInitializeSecurity`, `CoCreateInstance(WbemLocator)`,
//! `ConnectServer`, `CoSetProxyBlanket`, `ExecQuery` y `Next` + `Get`.
//!
//! Las clases que se consultan se pueden leer sin elevación (comprobado en Windows 11 el
//! 2026-11-22). [`call_u32`] ejecuta un método de una instancia (`GetObject`, `GetMethod`,
//! `SpawnInstance`, `Put` y `ExecMethod`); los de `root\WMI` suelen pedir admin.
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, RPC_E_TOO_LATE};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoInitializeSecurity, CoSetProxyBlanket, CoTaskMemFree, CoUninitialize, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{
    VARIANT, VT_I4, VariantClear, VariantToStringAlloc, VariantToUInt32,
};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WBEM_E_ACCESS_DENIED, WBEM_FLAG_FORWARD_ONLY,
    WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE, WBEM_INFINITE, WbemLocator,
};
use windows::core::{BSTR, HSTRING};

/// `WBEM_E_ACCESS_DENIED` pasa a `AccessDenied` para que la plataforma delegue en el helper.
fn os(e: windows::core::Error) -> PlatformError {
    if e.code().0 == WBEM_E_ACCESS_DENIED.0 {
        return PlatformError::AccessDenied(format!("WMI: {e}"));
    }
    PlatformError::Os(format!("WMI: {e}"))
}

/// COM inicializado en este hilo mientras vive; `CoUninitialize` al soltarlo.
pub(super) struct Com {
    /// `false` si el hilo ya tenía COM en otro modo: entonces no se desinicializa.
    uninit: bool,
}

impl Com {
    pub(super) fn init() -> Result<Self> {
        // SAFETY: inicializa COM en el hilo actual; S_FALSE (ya inicializado) también cuenta y
        // también pide `CoUninitialize`.
        let uninit = match unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            Ok(()) => true,
            // El hilo ya está en un apartamento de un solo hilo (p. ej. tras lanzar el helper
            // con ShellExecuteEx): WMI funciona igual y no hay nada que equilibrar.
            Err(e) if e.code() == RPC_E_CHANGED_MODE => false,
            Err(e) => return Err(os(e)),
        };
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
                drop(Com { uninit });
                Err(os(e))
            }
            _ => Ok(Com { uninit }),
        }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: equilibra el CoInitializeEx de `init` en el mismo hilo.
            unsafe { CoUninitialize() };
        }
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

/// Conexión a `namespace`. Necesita COM inicializado en el hilo ([`Com::init`]).
fn services(namespace: &str) -> Result<IWbemServices> {
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
    Ok(services)
}

/// Todas las filas de `query` en `namespace`, leídas con `read`.
pub(super) fn rows<T>(
    namespace: &str,
    query: &str,
    mut read: impl FnMut(&IWbemClassObject) -> Result<T>,
) -> Result<Vec<T>> {
    let _com = Com::init()?;
    let services = services(namespace)?;
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

/// Llama a `method` de la primera instancia de `class` con un único argumento `uint32`
/// (`input`) y devuelve el argumento de salida `output` como texto (WMI da los `uint64` como
/// cadena). `AccessDenied` si el SO no deja llamarlo sin elevación.
pub(super) fn call_u32(
    namespace: &str,
    class: &str,
    method: &str,
    (input, value): (&str, u32),
    output: &str,
) -> Result<Option<String>> {
    let _com = Com::init()?;
    let services = services(namespace)?;
    let path = rows(namespace, &format!("SELECT __PATH FROM {class}"), |o| {
        string(o, "__PATH")
    })?
    .into_iter()
    .flatten()
    .next()
    .ok_or_else(|| PlatformError::NotFound(format!("instancia de {class}")))?;
    let mut class_obj = None;
    // SAFETY: `services` es válido y `class_obj` recibe la definición de la clase.
    unsafe {
        services.GetObject(
            &BSTR::from(class),
            WBEM_GENERIC_FLAG_TYPE(0),
            None,
            Some(&mut class_obj),
            None,
        )
    }
    .map_err(os)?;
    let class_obj = class_obj.ok_or_else(|| PlatformError::NotFound(class.to_owned()))?;
    let mut signature = None;
    // SAFETY: `class_obj` es válido; solo se pide la firma de entrada.
    unsafe {
        class_obj.GetMethod(
            &HSTRING::from(method),
            0,
            &mut signature,
            std::ptr::null_mut(),
        )
    }
    .map_err(os)?;
    let signature =
        signature.ok_or_else(|| PlatformError::NotFound(format!("{class}.{method}")))?;
    // SAFETY: `signature` es la clase de los argumentos de entrada.
    let args = unsafe { signature.SpawnInstance(0) }.map_err(os)?;
    let mut arg = Value(VARIANT::default());
    // SAFETY: un `uint32` de WMI va en un VARIANT VT_I4 (mismos 32 bits); `arg.0` es un
    // VARIANT vacío y propio.
    unsafe {
        let v = &mut *arg.0.Anonymous.Anonymous;
        v.vt = VT_I4;
        v.Anonymous.lVal = value as i32;
    }
    // SAFETY: `args` es válido y `arg.0` vive durante la llamada (Put lo copia).
    unsafe { args.Put(&HSTRING::from(input), 0, &arg.0, 0) }.map_err(os)?;
    let mut out = None;
    // SAFETY: `services`, la ruta, el método y `args` son válidos; `out` recibe la salida.
    unsafe {
        services.ExecMethod(
            &BSTR::from(path),
            &BSTR::from(method),
            WBEM_GENERIC_FLAG_TYPE(0),
            None,
            &args,
            Some(&mut out),
            None,
        )
    }
    .map_err(os)?;
    match out {
        Some(out) => string(&out, output),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tras lanzar el helper el hilo queda en STA (ShellExecuteEx): WMI tiene que seguir
    /// funcionando para que la lectura llegue a pedir admin y pase al helper.
    #[test]
    fn works_on_a_single_threaded_apartment() {
        std::thread::spawn(|| {
            // SAFETY: inicializa COM en este hilo nuevo; se equilibra abajo.
            unsafe { CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED) }
                .ok()
                .unwrap();
            let build = first(
                r"ROOT\CIMV2",
                "SELECT BuildNumber FROM Win32_OperatingSystem",
                |o| string(o, "BuildNumber"),
            );
            // SAFETY: equilibra el CoInitializeEx de arriba.
            unsafe { CoUninitialize() };
            assert!(build.unwrap().flatten().is_some());
        })
        .join()
        .unwrap();
    }

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
