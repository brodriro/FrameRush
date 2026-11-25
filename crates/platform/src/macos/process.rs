//! Control de procesos en macOS ([`ProcessControl`]).
//!
//! Fuentes (páginas man de macOS y cabeceras de XNU que publica Apple):
//! - Prioridad: getpriority(2)/setpriority(2) con `PRIO_PROCESS`. El valor va de -20 a 20 y
//!   «Only the super-user may lower priorities»: sin root, bajar el nice (subir la prioridad),
//!   aunque sea para volver al valor anterior, falla con `EACCES`. `getpriority` «can
//!   legitimately return the value -1», así que se limpia `errno` antes y se mira después.
//! - Congelar, reanudar y cerrar: kill(2) con `SIGSTOP`, `SIGCONT` y `SIGKILL`. signal(3):
//!   `SIGSTOP` y `SIGKILL` no se pueden capturar ni ignorar.
//! - Congelados: sysctl(3), `CTL_KERN` / `KERN_PROC` / `KERN_PROC_ALL` devuelve la tabla de
//!   procesos como un array de `struct kinfo_proc` (el ejemplo de la página usa
//!   `kern.proc.pid` con `struct kinfo_proc`). Su primer campo es `struct extern_proc`, con
//!   `p_stat` y `p_pid` (xnu `bsd/sys/sysctl.h` y `bsd/sys/proc.h`). `SSTOP` (4) es «Process
//!   debugging or suspension» (`bsd/sys/proc.h`), y el `ps` de macOS (adv_cmds, `ps/print.c`)
//!   lo muestra como `T`, «stopped process» en ps(1). Un proceso parado en un depurador también
//!   cuenta como congelado.
//! - Afinidad: macOS no tiene afinidad estricta (Apple, «Thread Affinity API Release Notes»: no
//!   se puede fijar un hilo a una CPU, solo dar pistas al planificador). No se simula:
//!   `CapabilitySet::affinity` es `false`.
//!
//! Pendiente: `taskpolicy`/QoS, el primer plano (`NSWorkspace.frontmostApplication`) y el
//! helper con `SMAppService` para lo que pide root.

#![allow(unsafe_code)]

use crate::scan::target_of;
use crate::{
    Capabilities, CapabilitySet, PlatformError, Priority, ProcessControl, ProcessInfo, Result,
    Target, scan_processes,
};
use std::ffi::{c_char, c_int, c_void};
use std::io;
use std::mem::size_of;
use std::ptr;

/// Plataforma macOS. Por ahora solo controla procesos (M5).
#[derive(Debug, Default)]
pub struct MacosPlatform;

impl MacosPlatform {
    pub fn new() -> Self {
        Self
    }

    /// Comprueba que el PID sigue siendo el proceso del `Target` (It2.4).
    fn revalidate(&self, target: &Target) -> Result<libc::pid_t> {
        let pid = pid_t(target.pid)?;
        if self.is_same_process(target)? {
            Ok(pid)
        } else {
            Err(PlatformError::TargetChanged { pid: target.pid })
        }
    }

    fn refuse_own(&self, target: &Target) -> Result<()> {
        if target.pid == std::process::id() {
            Err(PlatformError::AccessDenied(
                "FrameRush no actúa sobre sus propios procesos".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn signal(&self, target: &Target, sig: c_int) -> Result<()> {
        let pid = self.revalidate(target)?;
        // SAFETY: kill(2) solo recibe enteros; `pid` es > 0, así que no apunta a un grupo.
        if unsafe { libc::kill(pid, sig) } == 0 {
            Ok(())
        } else {
            Err(os_error(target.pid, io::Error::last_os_error()))
        }
    }
}

/// `nice` de cada prioridad. El orden decide los empates al leer: gana la más cercana a
/// `Normal`.
const NICE: [(Priority, c_int); 6] = [
    (Priority::Normal, 0),
    (Priority::BelowNormal, 10),
    (Priority::AboveNormal, -5),
    (Priority::High, -10),
    (Priority::Idle, 19),
    (Priority::Realtime, -20),
];

fn to_nice(priority: Priority) -> c_int {
    NICE.iter()
        .find(|(p, _)| *p == priority)
        .map_or(0, |(_, n)| *n)
}

/// El tramo más cercano: el nice puede tener cualquier valor que haya puesto otro programa.
fn from_nice(nice: c_int) -> Priority {
    NICE.iter()
        .min_by_key(|(_, n)| (n - nice).abs())
        .map_or(Priority::Normal, |(p, _)| *p)
}

/// PID válido para kill(2) y setpriority(2). 0 no lo es: kill(2) lo trataría como «mi grupo».
fn pid_t(pid: u32) -> Result<libc::pid_t> {
    libc::pid_t::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| PlatformError::NotFound(format!("PID {pid}")))
}

fn os_error(pid: u32, err: io::Error) -> PlatformError {
    match err.raw_os_error() {
        Some(libc::ESRCH) => PlatformError::NotFound(format!("PID {pid}")),
        Some(libc::EPERM | libc::EACCES) => {
            PlatformError::AccessDenied(format!("PID {pid}: {err}"))
        }
        _ => PlatformError::Os(format!("PID {pid}: {err}")),
    }
}

impl ProcessControl for MacosPlatform {
    fn processes(&self) -> Result<Vec<ProcessInfo>> {
        scan_processes()
    }

    fn is_same_process(&self, target: &Target) -> Result<bool> {
        Ok(target_of(target.pid).is_some_and(|t| t == *target))
    }

    fn priority(&self, target: &Target) -> Result<Priority> {
        let pid = self.revalidate(target)?;
        // SAFETY: `__error` devuelve el `errno` del hilo actual, siempre válido.
        unsafe { *libc::__error() = 0 };
        // SAFETY: getpriority(2) solo recibe enteros.
        let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, pid as libc::id_t) };
        let err = io::Error::last_os_error();
        if nice == -1 && err.raw_os_error().is_some_and(|e| e != 0) {
            return Err(os_error(target.pid, err));
        }
        Ok(from_nice(nice))
    }

    /// Sin root solo se puede bajar la prioridad (subir el nice); lo demás da `AccessDenied`.
    fn set_priority(&self, target: &Target, priority: Priority) -> Result<()> {
        let pid = self.revalidate(target)?;
        // SAFETY: setpriority(2) solo recibe enteros.
        let r =
            unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, to_nice(priority)) };
        if r == 0 {
            Ok(())
        } else {
            Err(os_error(target.pid, io::Error::last_os_error()))
        }
    }

    fn suspend(&self, target: &Target) -> Result<()> {
        self.refuse_own(target)?;
        self.signal(target, libc::SIGSTOP)
    }

    fn resume(&self, target: &Target) -> Result<()> {
        self.signal(target, libc::SIGCONT)
    }

    fn kill(&self, target: &Target) -> Result<()> {
        self.refuse_own(target)?;
        self.signal(target, libc::SIGKILL)
    }

    fn foreground(&self) -> Result<Option<Target>> {
        Err(PlatformError::Unsupported("primer plano en macOS"))
    }

    fn suspended(&self) -> Result<Vec<u32>> {
        stopped_pids()
    }
}

impl Capabilities for MacosPlatform {
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            set_priority: true,
            suspend: true,
            kill: true,
            ..CapabilitySet::default()
        }
    }
}

/// Principio de `struct extern_proc` (xnu `bsd/sys/proc.h`) hasta `p_pid`, en 64 bits. El
/// tamaño real de `struct kinfo_proc` no se fija aquí: se mide con el propio sysctl.
#[repr(C)]
struct ExternProcHead {
    /// Unión de dos punteros (`p_st1`) y `struct timeval` (`p_starttime`): 16 bytes.
    _p_un: [u64; 2],
    _p_vmspace: usize,
    _p_sigacts: usize,
    _p_flag: c_int,
    p_stat: c_char,
    p_pid: libc::pid_t,
}

// macOS solo existe ya en 64 bits (x86_64 y arm64); la cabecera de arriba lo da por hecho.
const _: () = assert!(size_of::<usize>() == 8);

/// `p_stat` de un proceso parado (xnu `bsd/sys/proc.h`).
const SSTOP: c_char = 4;

/// Lee `kern.proc.<cmd>.<arg>` en un búfer de `len` bytes. Devuelve los bytes escritos.
fn kern_proc(cmd: c_int, arg: c_int, len: usize) -> io::Result<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, cmd, arg];
    let mut buf = vec![0u8; len];
    let mut size = buf.len();
    // SAFETY: `mib` tiene 4 elementos, `buf` tiene `size` bytes escribibles y `size` es una
    // variable local viva; no se escribe ningún valor nuevo (newp nulo, newlen 0).
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(size);
    Ok(buf)
}

/// Tamaño de `struct kinfo_proc`: lo que ocupa la entrada de este mismo proceso.
fn kinfo_proc_size() -> Result<usize> {
    let me = pid_t(std::process::id())?;
    let size = kern_proc(libc::KERN_PROC_PID, me, 64 * 1024)
        .map_err(|e| PlatformError::Os(format!("sysctl kern.proc.pid: {e}")))?
        .len();
    if size < size_of::<ExternProcHead>() {
        return Err(PlatformError::Os(format!(
            "kinfo_proc de {size} bytes, más corto de lo esperado"
        )));
    }
    Ok(size)
}

/// PID de los procesos en `SSTOP`, según `kern.proc.all`.
fn stopped_pids() -> Result<Vec<u32>> {
    let stride = kinfo_proc_size()?;
    let table = all_procs(stride)?;
    if table.len() % stride != 0 {
        return Err(PlatformError::Os(format!(
            "kern.proc.all: {} bytes no son entradas de {stride}",
            table.len()
        )));
    }
    let mut pids: Vec<u32> = table
        .chunks_exact(stride)
        .filter_map(|entry| {
            // SAFETY: cada entrada tiene `stride` ≥ size_of::<ExternProcHead>() bytes (lo
            // comprueba `kinfo_proc_size`) y `read_unaligned` no pide alineación.
            let head = unsafe { ptr::read_unaligned(entry.as_ptr().cast::<ExternProcHead>()) };
            (head.p_stat == SSTOP)
                .then(|| u32::try_from(head.p_pid).ok())
                .flatten()
        })
        .collect();
    pids.sort_unstable();
    Ok(pids)
}

/// Tabla de procesos completa. Entre la consulta del tamaño y la lectura pueden nacer
/// procesos: se deja margen y, si aun así falta sitio (`ENOMEM`, sysctl(3)), se reintenta.
fn all_procs(stride: usize) -> Result<Vec<u8>> {
    let mut len = 0;
    for _ in 0..4 {
        len = (estimate(libc::KERN_PROC_ALL)?.max(len) + 64 * stride).max(len * 2);
        match kern_proc(libc::KERN_PROC_ALL, 0, len) {
            Ok(table) => return Ok(table),
            Err(e) if e.raw_os_error() == Some(libc::ENOMEM) => continue,
            Err(e) => return Err(PlatformError::Os(format!("sysctl kern.proc.all: {e}"))),
        }
    }
    Err(PlatformError::Os(
        "sysctl kern.proc.all: la tabla no deja de crecer".into(),
    ))
}

/// Bytes que pide `kern.proc.<cmd>` (sysctl(3): con `oldp` nulo devuelve el tamaño en
/// `oldlenp`).
fn estimate(cmd: c_int) -> Result<usize> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, cmd, 0];
    let mut size = 0usize;
    // SAFETY: `mib` tiene 4 elementos, `oldp` es nulo y `size` es una variable local viva.
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return Err(PlatformError::Os(format!(
            "sysctl kern.proc: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    //! Solo actúan sobre un proceso hijo lanzado por el propio test.

    use super::*;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    struct Victim {
        child: Child,
        target: Target,
    }

    impl Drop for Victim {
        fn drop(&mut self) {
            // Por si el test falla con el hijo parado: SIGKILL también termina a un parado.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn victim() -> Victim {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let target = target_of(child.id()).unwrap();
        Victim { child, target }
    }

    /// SIGSTOP y SIGCONT se entregan de forma asíncrona: se espera hasta 5 s al estado.
    fn wait_stopped(m: &MacosPlatform, pid: u32, stopped: bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if m.suspended().unwrap().contains(&pid) == stopped {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn is_root() -> bool {
        // SAFETY: geteuid(2) no recibe argumentos y no falla.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn nice_maps_to_the_closest_priority() {
        for (p, n) in NICE {
            assert_eq!(from_nice(n), p);
            assert_eq!(to_nice(p), n);
        }
        assert_eq!(from_nice(5), Priority::Normal);
        assert_eq!(from_nice(14), Priority::BelowNormal);
        assert_eq!(from_nice(20), Priority::Idle);
        assert_eq!(from_nice(-15), Priority::High);
        assert_eq!(from_nice(-18), Priority::Realtime);
    }

    /// Bajar la prioridad siempre se puede; volver a subirla solo con root.
    #[test]
    fn priority_goes_down_and_needs_root_to_go_up() {
        let m = MacosPlatform::new();
        let v = victim();
        m.set_priority(&v.target, Priority::BelowNormal).unwrap();
        assert_eq!(m.priority(&v.target).unwrap(), Priority::BelowNormal);
        m.set_priority(&v.target, Priority::Idle).unwrap();
        assert_eq!(m.priority(&v.target).unwrap(), Priority::Idle);
        let back = m.set_priority(&v.target, Priority::Normal);
        if is_root() {
            back.unwrap();
            assert_eq!(m.priority(&v.target).unwrap(), Priority::Normal);
        } else {
            assert!(matches!(back, Err(PlatformError::AccessDenied(_))));
        }
    }

    #[test]
    fn suspend_and_resume_a_child() {
        let m = MacosPlatform::new();
        let v = victim();
        assert!(!m.suspended().unwrap().contains(&v.target.pid));
        m.suspend(&v.target).unwrap();
        assert!(wait_stopped(&m, v.target.pid, true));
        m.resume(&v.target).unwrap();
        assert!(wait_stopped(&m, v.target.pid, false));
        assert!(m.is_same_process(&v.target).unwrap());
    }

    #[test]
    fn kill_ends_the_child() {
        let m = MacosPlatform::new();
        let mut v = victim();
        m.kill(&v.target).unwrap();
        v.child.wait().unwrap();
        assert!(!m.is_same_process(&v.target).unwrap());
    }

    #[test]
    fn a_changed_target_is_rejected() {
        let m = MacosPlatform::new();
        let v = victim();
        let mut stale = v.target.clone();
        stale.start_time.0 += 1;
        let changed = Err(PlatformError::TargetChanged { pid: stale.pid });
        assert_eq!(m.set_priority(&stale, Priority::Idle), changed);
        assert_eq!(m.suspend(&stale), changed);
        assert_eq!(m.kill(&stale), changed);
        assert!(m.is_same_process(&v.target).unwrap());
    }

    #[test]
    fn refuses_to_freeze_or_kill_itself() {
        let m = MacosPlatform::new();
        let me = target_of(std::process::id()).unwrap();
        assert!(matches!(
            m.suspend(&me),
            Err(PlatformError::AccessDenied(_))
        ));
        assert!(matches!(m.kill(&me), Err(PlatformError::AccessDenied(_))));
    }

    #[test]
    fn pid_zero_is_never_signalled() {
        assert!(matches!(pid_t(0), Err(PlatformError::NotFound(_))));
        assert!(matches!(pid_t(u32::MAX), Err(PlatformError::NotFound(_))));
    }

    #[test]
    fn the_process_table_includes_this_process() {
        let stride = kinfo_proc_size().unwrap();
        let table = all_procs(stride).unwrap();
        assert_eq!(table.len() % stride, 0);
        let me = pid_t(std::process::id()).unwrap();
        assert!(table.chunks_exact(stride).any(|e| {
            // SAFETY: igual que en `stopped_pids`.
            let head = unsafe { ptr::read_unaligned(e.as_ptr().cast::<ExternProcHead>()) };
            head.p_pid == me
        }));
    }

    #[test]
    fn no_affinity_on_macos() {
        let caps = MacosPlatform::new().capabilities();
        assert!(caps.set_priority && caps.suspend && caps.kill);
        assert!(!caps.affinity);
        assert_eq!(
            MacosPlatform::new().foreground(),
            Err(PlatformError::Unsupported("primer plano en macOS"))
        );
    }
}
