//! Procesos congelados: todos sus hilos suspendidos (PLAN.md M3, «congelados por otro programa»).
//!
//! `NtQuerySystemInformation(SystemProcessInformation)` devuelve, por proceso, un
//! `SYSTEM_PROCESS_INFORMATION` seguido de `NumberOfThreads` registros
//! `SYSTEM_THREAD_INFORMATION` con `ThreadState` y `WaitReason` (Microsoft Learn,
//! «NtQuerySystemInformation function»; el orden de los registros, como lo lee .NET en
//! `ProcessManager.Windows.cs` de dotnet/runtime). Un hilo está suspendido si está en espera
//! (`ThreadState` 5, `System.Diagnostics.ThreadState.Wait`) por suspensión (`WaitReason` 5 o 12,
//! que .NET traduce a `ThreadWaitReason.Suspended`).
//!
//! Windows también suspende las apps empaquetadas (Store) en segundo plano. Esas no cuentan:
//! un proceso solo se devuelve si `GetPackageFullName` dice `APPMODEL_ERROR_NO_PACKAGE` (sin
//! identidad de paquete). Si no se puede abrir el proceso para preguntarlo, tampoco cuenta.

use crate::{PlatformError, Result};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
use windows_sys::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, STATUS_INFO_LENGTH_MISMATCH};
use windows_sys::Win32::Storage::Packaging::Appx::GetPackageFullName;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::System::WindowsProgramming::{
    SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION,
};

/// `ThreadState` de un hilo en espera.
const WAITING: u32 = 5;
/// `WaitReason` de un hilo suspendido (`Suspended` y `WrSuspended`).
const SUSPENDED: [u32; 2] = [5, 12];
/// Tamaño inicial del búfer; si no basta, el SO dice cuánto hace falta.
const INITIAL_BUFFER: usize = 512 * 1024;
/// Intentos si los procesos crecen entre la llamada que da el tamaño y la siguiente.
const ATTEMPTS: usize = 4;

/// PID de los procesos sin paquete con todos sus hilos suspendidos.
pub(super) fn suspended_pids() -> Result<Vec<u32>> {
    let data = system_processes()?;
    let mut pids = vec![];
    for (pid, threads) in parse(&data)? {
        if all_suspended(&threads) && unpackaged(pid) {
            pids.push(pid);
        }
    }
    Ok(pids)
}

/// `true` si hay hilos y todos están en espera por suspensión.
fn all_suspended(threads: &[(u32, u32)]) -> bool {
    !threads.is_empty()
        && threads
            .iter()
            .all(|&(state, reason)| state == WAITING && SUSPENDED.contains(&reason))
}

/// Búfer de `SystemProcessInformation`. Va en `u64` para que las estructuras queden alineadas.
fn system_processes() -> Result<Vec<u64>> {
    let mut bytes = INITIAL_BUFFER;
    for _ in 0..ATTEMPTS {
        let mut buf = vec![0u64; bytes.div_ceil(8)];
        let len = u32::try_from(buf.len() * 8)
            .map_err(|_| PlatformError::Os("lista de procesos demasiado grande".into()))?;
        let mut needed = 0u32;
        // SAFETY: `buf` mide `len` bytes y vive durante la llamada; `needed` es local.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buf.as_mut_ptr().cast(),
                len,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // Margen por si aparecen procesos antes del siguiente intento.
            bytes = needed as usize + 64 * 1024;
            continue;
        }
        if status < 0 {
            return Err(PlatformError::Os(format!(
                "NtQuerySystemInformation: estado {status:#x}"
            )));
        }
        return Ok(buf);
    }
    Err(PlatformError::Os(
        "NtQuerySystemInformation: la lista de procesos no deja de crecer".into(),
    ))
}

/// PID y `(ThreadState, WaitReason)` de cada hilo.
type ProcessThreads = (u32, Vec<(u32, u32)>);

/// Procesos del búfer con sus hilos.
fn parse(data: &[u64]) -> Result<Vec<ProcessThreads>> {
    // SAFETY: ver `read`; el búfer de `u64` se lee como bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 8) };
    let mut out = vec![];
    let mut offset = 0usize;
    loop {
        let process: SYSTEM_PROCESS_INFORMATION = read(bytes, offset)?;
        let first = offset + size_of::<SYSTEM_PROCESS_INFORMATION>();
        let threads = (0..process.NumberOfThreads as usize)
            .map(|i| {
                read::<SYSTEM_THREAD_INFORMATION>(
                    bytes,
                    first + i * size_of::<SYSTEM_THREAD_INFORMATION>(),
                )
                .map(|t| (t.ThreadState, t.WaitReason))
            })
            .collect::<Result<Vec<_>>>()?;
        // El PID viene como HANDLE; el 0 es el proceso inactivo del sistema.
        let pid = process.UniqueProcessId as usize as u32;
        if pid != 0 {
            out.push((pid, threads));
        }
        if process.NextEntryOffset == 0 {
            return Ok(out);
        }
        offset += process.NextEntryOffset as usize;
    }
}

/// Copia un `T` del búfer en `offset`, comprobando que cabe.
fn read<T: Copy>(bytes: &[u8], offset: usize) -> Result<T> {
    let end = offset.checked_add(size_of::<T>());
    if end.is_none_or(|end| end > bytes.len()) {
        return Err(PlatformError::Os(
            "NtQuerySystemInformation: registro fuera del búfer".into(),
        ));
    }
    // SAFETY: `offset..offset + size_of::<T>()` está dentro de `bytes` (comprobado arriba), `T`
    // es una estructura C sin invariantes (`Copy`) y `read_unaligned` no exige alineación.
    Ok(unsafe { bytes.as_ptr().add(offset).cast::<T>().read_unaligned() })
}

/// `true` si el proceso no tiene identidad de paquete. `false` si la tiene o no se sabe.
fn unpackaged(pid: u32) -> bool {
    // SAFETY: OpenProcess no toma punteros; devuelve null si falla.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return false;
    }
    // SAFETY: `raw` es un handle recién abierto y solo lo cierra este `OwnedHandle`.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut len = 0u32;
    // SAFETY: con longitud 0 y búfer null solo pregunta; `len` es local.
    let code =
        unsafe { GetPackageFullName(process.as_raw_handle(), &mut len, std::ptr::null_mut()) };
    // Con paquete devuelve ERROR_INSUFFICIENT_BUFFER (pide el nombre); cualquier otro código
    // tampoco confirma que no lo tenga.
    code == APPMODEL_ERROR_NO_PACKAGE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProcessControl;
    use crate::scan::target_of;
    use crate::windows::WindowsPlatform;

    #[test]
    fn all_suspended_needs_every_thread_suspended() {
        assert!(all_suspended(&[(5, 5), (5, 12)]));
        assert!(!all_suspended(&[(5, 5), (5, 6)]));
        assert!(!all_suspended(&[(2, 0)]));
        assert!(!all_suspended(&[]));
    }

    #[test]
    fn lists_this_process_with_its_threads() {
        let data = system_processes().expect("lista de procesos");
        let me = std::process::id();
        let procs = parse(&data).expect("búfer válido");
        let (_, threads) = procs
            .iter()
            .find(|(pid, _)| *pid == me)
            .expect("este proceso");
        assert!(!threads.is_empty());
        assert!(!all_suspended(threads), "el test está en marcha");
    }

    #[test]
    fn finds_a_suspended_child_until_resumed() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("ping");
        let pid = child.id();
        let platform = WindowsPlatform::new();
        let target = target_of(pid).expect("ping vivo");
        // Suspender y reanudar llegan a cada hilo de forma asíncrona: con el equipo cargado,
        // la lista puede tardar en reflejarlo.
        let listed = |want: bool| {
            (0..30).any(|_| {
                let found = suspended_pids().expect("congelados").contains(&pid);
                if found != want {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                found == want
            })
        };
        platform.suspend(&target).expect("suspender");
        let frozen = listed(true);
        platform.resume(&target).expect("reanudar");
        let after = !listed(false);
        let _ = child.kill();
        let _ = child.wait();
        assert!(frozen, "{pid} no salió como congelado en 3 s");
        assert!(
            !after,
            "{pid} sigue como congelado 3 s después de reanudarlo"
        );
    }
}
