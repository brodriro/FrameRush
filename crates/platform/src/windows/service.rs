//! Servicios con el SCM (advapi32): `OpenSCManagerW`, `OpenServiceW`, `QueryServiceStatus`,
//! `ControlService` y `StartServiceW`.
//!
//! Solo se detienen y se vuelven a arrancar, sin tocar el tipo de inicio (PLAN.md §3,
//! "Alcance"). Leer el estado no pide admin; detener y arrancar sí, y entonces devuelven
//! `AccessDenied` para que `WindowsPlatform` lo pase al helper.
//!
//! La espera tras pedir la parada sigue el ejemplo oficial "Stopping a Service"
//! (learn.microsoft.com/windows/win32/services/stopping-a-service): se espera una décima parte
//! de `dwWaitHint`, entre 1 y 10 s, hasta un máximo total. Un servicio con dependientes en
//! marcha no se detiene (`ERROR_DEPENDENT_SERVICES_RUNNING`): no se paran otros servicios que
//! el usuario no ha marcado.
#![allow(unsafe_code)]

use super::helper::wide;
use crate::{PlatformError, Result, ServiceState};
use std::ptr::null;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST,
    ERROR_SERVICE_NOT_ACTIVE, GetLastError,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_CONTROL_STOP, SERVICE_PAUSED, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED, StartServiceW,
};

/// Cuánto se espera como mucho a que un servicio termine de pararse.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle del SCM o de un servicio; se cierra al soltarlo.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: el handle es válido y propio (no nulo, lo comprueba quien lo crea).
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn last_error(what: &str, name: &str) -> PlatformError {
    // SAFETY: sin requisitos; lee el último error del hilo.
    let code = unsafe { GetLastError() };
    match code {
        ERROR_ACCESS_DENIED => PlatformError::AccessDenied(format!("{what} {name}")),
        ERROR_SERVICE_DOES_NOT_EXIST => PlatformError::NotFound(format!("servicio {name}")),
        _ => PlatformError::Os(format!(
            "{what} {name}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )),
    }
}

/// El servicio `name` con los permisos `access`, o `None` si no existe.
fn open(name: &str, access: u32) -> Result<Option<ScHandle>> {
    // SAFETY: máquina y base de datos nulas = locales (documentado); CONNECT no pide admin.
    let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if scm.is_null() {
        return Err(last_error("abrir el administrador de servicios para", name));
    }
    let scm = ScHandle(scm);
    let wname = wide(name);
    // SAFETY: `scm` es válido y `wname` termina en NUL; los dos viven durante la llamada.
    let service = unsafe { OpenServiceW(scm.0, wname.as_ptr(), access) };
    if service.is_null() {
        return match last_error("abrir el servicio", name) {
            PlatformError::NotFound(_) => Ok(None),
            e => Err(e),
        };
    }
    Ok(Some(ScHandle(service)))
}

fn status(service: &ScHandle, name: &str) -> Result<SERVICE_STATUS> {
    let mut st = SERVICE_STATUS::default();
    // SAFETY: `service` es válido y `st` vive durante la llamada.
    if unsafe { QueryServiceStatus(service.0, &mut st) } == 0 {
        return Err(last_error("leer el estado del servicio", name));
    }
    Ok(st)
}

fn state(st: &SERVICE_STATUS) -> ServiceState {
    match st.dwCurrentState {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED => ServiceState::Stopped,
        SERVICE_PAUSED => ServiceState::Paused,
        _ => ServiceState::Pending,
    }
}

pub(super) fn service_state(name: &str) -> Result<Option<ServiceState>> {
    let Some(service) = open(name, SERVICE_QUERY_STATUS)? else {
        return Ok(None);
    };
    Ok(Some(state(&status(&service, name)?)))
}

pub(super) fn stop_service(name: &str) -> Result<()> {
    let service = open(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?
        .ok_or_else(|| PlatformError::NotFound(format!("servicio {name}")))?;
    let mut st = SERVICE_STATUS::default();
    // SAFETY: `service` es válido y `st` vive durante la llamada.
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut st) } == 0 {
        // SAFETY: sin requisitos; lee el último error del hilo.
        if unsafe { GetLastError() } == ERROR_SERVICE_NOT_ACTIVE {
            return Ok(());
        }
        return Err(last_error("detener el servicio", name));
    }
    let start = Instant::now();
    while st.dwCurrentState != SERVICE_STOPPED {
        if start.elapsed() > STOP_TIMEOUT {
            return Err(PlatformError::Os(format!(
                "el servicio {name} no se detuvo en {} s",
                STOP_TIMEOUT.as_secs()
            )));
        }
        let hint = Duration::from_millis(u64::from(st.dwWaitHint) / 10);
        std::thread::sleep(hint.clamp(Duration::from_secs(1), Duration::from_secs(10)));
        st = status(&service, name)?;
    }
    Ok(())
}

pub(super) fn start_service(name: &str) -> Result<()> {
    let service = open(name, SERVICE_START)?
        .ok_or_else(|| PlatformError::NotFound(format!("servicio {name}")))?;
    // SAFETY: `service` es válido; sin argumentos (0 y puntero nulo, documentado).
    if unsafe { StartServiceW(service.0, 0, null()) } == 0 {
        // SAFETY: sin requisitos; lee el último error del hilo.
        if unsafe { GetLastError() } == ERROR_SERVICE_ALREADY_RUNNING {
            return Ok(());
        }
        return Err(last_error("arrancar el servicio", name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Leer el estado no pide admin; un nombre que no existe es `None`, no un error.
    #[test]
    fn reads_a_service_without_admin() {
        // `RpcSs` (llamada a procedimiento remoto) existe y está en marcha en todo Windows.
        assert_eq!(service_state("RpcSs").unwrap(), Some(ServiceState::Running));
        assert_eq!(service_state("framerush-no-existe").unwrap(), None);
    }
}
