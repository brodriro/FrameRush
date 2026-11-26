//! Control de procesos en Linux: [`ProcessControl`] y la afinidad de [`GameTuning`].
//!
//! Fuentes (Linux man-pages, man7.org):
//! - <https://man7.org/linux/man-pages/man2/setpriority.2.html>: el nice va de -20 a 19 y, con
//!   NPTL, es un atributo **por hilo** (sección BUGS). Por eso se fija en cada hilo de
//!   `/proc/<pid>/task` y se lee el del hilo principal (TID = PID). `getpriority` puede devolver
//!   -1 sin error: hay que poner `errno` a 0 antes y mirarlo después. Bajar el nice sin
//!   `CAP_SYS_NICE` da `EACCES`.
//! - <https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html>: la máscara también es por
//!   hilo (`pid` es un TID); `EINVAL` si no contiene ninguna CPU permitida, `EPERM` sin permiso.
//! - <https://man7.org/linux/man-pages/man5/proc_pid_task.5.html>: `/proc/<pid>/task` tiene un
//!   subdirectorio por hilo con su TID.
//! - <https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html>: en `/proc/<pid>/stat`, el
//!   campo 2 es `comm` entre paréntesis (puede llevar espacios y paréntesis) y el 3, `state`;
//!   `T` es «parado por una señal» (`t` es la parada de un depurador, que no cuenta).
//! - <https://man7.org/linux/man-pages/man2/pidfd_open.2.html> (Linux 5.3, sin envoltorio de
//!   glibc: va por `syscall`) y
//!   <https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html> (Linux 5.1): un pidfd es
//!   una referencia estable al proceso; si termina, la señal falla con `ESRCH` en vez de llegar
//!   a otro que reutilice el PID.
//! - <https://man7.org/linux/man-pages/man7/signal.7.html>: `SIGSTOP` para todo el proceso y no
//!   se puede capturar ni ignorar; `SIGCONT` lo reanuda.
//!
//! Revalidación (It2.4): se abre un pidfd y después se comprueba `(pid, start_time, exe_path)`;
//! si coincide, el pidfd es del proceso que se quería. Las señales van por ese pidfd. `nice` y
//! la afinidad se aplican por TID y, al terminar, se comprueba con el pidfd que el proceso sigue
//! vivo (si vive, su PID no se reutilizó por el camino).

#![allow(unsafe_code)]

use super::Helper;
use crate::scan::target_of;
use crate::{
    GameTuning, PlatformError, Priority, ProcessControl, ProcessInfo, Result, SavedValue, Target,
    scan_processes,
};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

/// Plataforma Linux.
#[derive(Debug, Default, Clone)]
pub struct LinuxPlatform {
    /// Helper elevado (pkexec) para lo que el SO deniega a este proceso: subir la prioridad,
    /// procesos de otro usuario y servicios. Sin él, eso falla con `AccessDenied`.
    helper: Option<Helper>,
}

impl LinuxPlatform {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plataforma que delega en `helper` lo que el SO deniega.
    pub fn with_helper(helper: Helper) -> Self {
        Self {
            helper: Some(helper),
        }
    }

    /// Nunca se congela ni se cierra el propio FrameRush, que es quien reanuda al resto, ni su
    /// helper.
    fn refuse_own(&self, target: &Target) -> Result<()> {
        let helper = self.helper.as_ref().map(Helper::pid);
        if target.pid == std::process::id() || Some(target.pid) == helper {
            Err(PlatformError::AccessDenied(
                "FrameRush no actúa sobre sus propios procesos".into(),
            ))
        } else {
            Ok(())
        }
    }

    /// `here` o, si el SO lo deniega y hay helper, la misma acción en el helper.
    pub(super) fn or_helper(
        &self,
        here: impl FnOnce() -> Result<()>,
        elevated: impl FnOnce(&Helper) -> Result<()>,
    ) -> Result<()> {
        match (here(), &self.helper) {
            (Err(PlatformError::AccessDenied(_)), Some(helper)) => elevated(helper),
            (result, _) => result,
        }
    }
}

/// Referencia estable a un proceso (pidfd), ya revalidada.
struct Pidfd {
    fd: OwnedFd,
    pid: u32,
}

impl Pidfd {
    /// Abre el pidfd y comprueba después que el PID sigue siendo `target`.
    fn open(target: &Target) -> Result<Self> {
        let pid = libc::pid_t::try_from(target.pid)
            .map_err(|_| PlatformError::NotFound(format!("PID {}", target.pid)))?;
        let flags: libc::c_uint = 0;
        // SAFETY: pidfd_open no toma punteros; devuelve un descriptor nuevo o -1 con errno.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, flags) };
        if raw < 0 {
            // Como en Windows: un proceso que ya no está es un objetivo que cambió.
            return Err(match os_error(io::Error::last_os_error(), target.pid) {
                PlatformError::NotFound(_) => PlatformError::TargetChanged { pid: target.pid },
                other => other,
            });
        }
        let raw = RawFd::try_from(raw)
            .map_err(|_| PlatformError::Os(format!("pidfd fuera de rango: {raw}")))?;
        // SAFETY: `raw` es un descriptor abierto que nos pertenece; OwnedFd lo cierra al soltarlo.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if target_of(target.pid).as_ref() != Some(target) {
            return Err(PlatformError::TargetChanged { pid: target.pid });
        }
        Ok(Self {
            fd,
            pid: target.pid,
        })
    }

    /// Envía `sig` al proceso del pidfd (0 solo comprueba que sigue vivo).
    fn signal(&self, sig: libc::c_int) -> Result<()> {
        let flags: libc::c_uint = 0;
        // SAFETY: el descriptor está abierto mientras viva `self`; `info` nulo equivale a kill(2)
        // y `flags` 0 es el valor por defecto (pidfd_send_signal(2)).
        let r = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                sig,
                std::ptr::null::<libc::siginfo_t>(),
                flags,
            )
        };
        if r < 0 {
            Err(os_error(io::Error::last_os_error(), self.pid))
        } else {
            Ok(())
        }
    }

    /// Tras actuar por PID o TID: si el proceso terminó, el PID pudo reutilizarse.
    fn still_alive(&self) -> Result<()> {
        self.signal(0).map_err(|e| match e {
            PlatformError::NotFound(_) => PlatformError::TargetChanged { pid: self.pid },
            other => other,
        })
    }

    /// TID de cada hilo del proceso.
    fn threads(&self) -> Result<Vec<libc::pid_t>> {
        let dir = std::fs::read_dir(format!("/proc/{}/task", self.pid))
            .map_err(|e| os_error(e, self.pid))?;
        Ok(dir
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect())
    }

    /// Aplica `op` a cada hilo. Un hilo que ya terminó (`ESRCH`) no cuenta como fallo.
    fn each_thread(&self, op: impl Fn(libc::pid_t) -> io::Result<()>) -> Result<()> {
        for tid in self.threads()? {
            match op(tid) {
                Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
                Err(e) => return Err(os_error(e, self.pid)),
                Ok(()) => {}
            }
        }
        self.still_alive()
    }
}

/// Traduce un errno al error de plataforma.
fn os_error(e: io::Error, pid: u32) -> PlatformError {
    match e.raw_os_error() {
        Some(libc::EPERM | libc::EACCES) => PlatformError::AccessDenied(format!("PID {pid}: {e}")),
        Some(libc::ESRCH) => PlatformError::NotFound(format!("PID {pid}")),
        // `/proc/<pid>` ya no existe.
        Some(libc::ENOENT) => PlatformError::NotFound(format!("PID {pid}")),
        Some(libc::ENOSYS) => PlatformError::Unsupported("pidfd (pide Linux 5.3 o posterior)"),
        _ => PlatformError::Os(format!("PID {pid}: {e}")),
    }
}

/// Nice de cada prioridad (decisión del usuario, 2026-11-24): solo `nice`, sin planificador de
/// tiempo real; Tiempo real es el nice más bajo.
fn nice_of(priority: Priority) -> libc::c_int {
    match priority {
        Priority::Idle => 19,
        Priority::BelowNormal => 10,
        Priority::Normal => 0,
        Priority::AboveNormal => -5,
        Priority::High => -10,
        Priority::Realtime => -20,
    }
}

/// Prioridad de un nice: la de [`nice_of`] más cercana (un empate va a la más baja).
fn priority_of(nice: libc::c_int) -> Priority {
    use Priority::*;
    [Idle, BelowNormal, Normal, AboveNormal, High, Realtime]
        .into_iter()
        .min_by_key(|&p| ((nice_of(p) - nice).abs(), -nice_of(p)))
        .unwrap_or(Normal)
}

/// Nice de un hilo.
fn get_nice(tid: libc::pid_t) -> io::Result<libc::c_int> {
    let who = libc::id_t::try_from(tid).map_err(|_| io::Error::from_raw_os_error(libc::ESRCH))?;
    // SAFETY: __errno_location devuelve el errno de este hilo, siempre válido.
    unsafe { *libc::__errno_location() = 0 };
    // SAFETY: getpriority no toma punteros.
    let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, who) };
    let e = io::Error::last_os_error();
    if nice == -1 && e.raw_os_error() != Some(0) {
        Err(e)
    } else {
        Ok(nice)
    }
}

fn set_nice(tid: libc::pid_t, nice: libc::c_int) -> io::Result<()> {
    let who = libc::id_t::try_from(tid).map_err(|_| io::Error::from_raw_os_error(libc::ESRCH))?;
    // SAFETY: setpriority no toma punteros.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, who, nice) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Número de CPU que caben en un `cpu_set_t`.
const CPU_SETSIZE: usize = libc::CPU_SETSIZE as usize;

fn get_affinity(tid: libc::pid_t) -> io::Result<Vec<usize>> {
    // SAFETY: cpu_set_t es una máscara de bits; todo ceros es un valor válido (ninguna CPU).
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: `set` vive durante la llamada y su tamaño es el que se pasa.
    let r = unsafe { libc::sched_getaffinity(tid, std::mem::size_of_val(&set), &mut set) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((0..CPU_SETSIZE)
        // SAFETY: `cpu` < CPU_SETSIZE, dentro de la máscara.
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &set) })
        .collect())
}

fn set_affinity(tid: libc::pid_t, set: &libc::cpu_set_t) -> io::Result<()> {
    // SAFETY: `set` vive durante la llamada y su tamaño es el que se pasa.
    if unsafe { libc::sched_setaffinity(tid, std::mem::size_of_val(set), set) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Máscara con `cpus`. Falla si está vacía o alguna no cabe en un `cpu_set_t`.
fn cpu_set(cpus: &[usize]) -> Result<libc::cpu_set_t> {
    if cpus.is_empty() {
        return Err(PlatformError::Os("afinidad vacía".into()));
    }
    // SAFETY: cpu_set_t es una máscara de bits; todo ceros es un valor válido.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for &cpu in cpus {
        if cpu >= CPU_SETSIZE {
            return Err(PlatformError::Os(format!(
                "CPU {cpu} fuera de la máscara ({CPU_SETSIZE} como máximo)"
            )));
        }
        // SAFETY: `cpu` < CPU_SETSIZE, dentro de la máscara.
        unsafe { libc::CPU_SET(cpu, &mut set) };
    }
    Ok(set)
}

/// Estado (campo 3) de una línea de `/proc/<pid>/stat`. `comm` va entre paréntesis y puede
/// contenerlos, así que el estado es lo que sigue al último `)`.
fn stat_state(stat: &str) -> Option<char> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.trim_start().chars().next()
}

/// PID de los procesos parados por una señal (estado `T`), sin el propio FrameRush.
fn stopped_pids() -> Result<Vec<u32>> {
    let me = std::process::id();
    let proc = std::fs::read_dir("/proc").map_err(|e| PlatformError::Os(format!("/proc: {e}")))?;
    let mut pids: Vec<u32> = proc
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| pid != me)
        // Un proceso que termina entre listar y leer simplemente no está.
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|s| stat_state(&s) == Some('T'))
        })
        .collect();
    pids.sort_unstable();
    Ok(pids)
}

fn main_thread(target: &Target) -> Result<libc::pid_t> {
    libc::pid_t::try_from(target.pid)
        .map_err(|_| PlatformError::NotFound(format!("PID {}", target.pid)))
}

impl ProcessControl for LinuxPlatform {
    fn processes(&self) -> Result<Vec<ProcessInfo>> {
        scan_processes()
    }

    fn is_same_process(&self, target: &Target) -> Result<bool> {
        Ok(target_of(target.pid).is_some_and(|t| t == *target))
    }

    /// La del hilo principal.
    fn priority(&self, target: &Target) -> Result<Priority> {
        let pidfd = Pidfd::open(target)?;
        let nice = get_nice(main_thread(target)?).map_err(|e| os_error(e, target.pid))?;
        pidfd.still_alive()?;
        Ok(priority_of(nice))
    }

    /// En todos los hilos. Subir la prioridad (bajar el nice) pide `CAP_SYS_NICE`.
    fn set_priority(&self, target: &Target, priority: Priority) -> Result<()> {
        let nice = nice_of(priority);
        self.or_helper(
            || Pidfd::open(target)?.each_thread(|tid| set_nice(tid, nice)),
            |h| h.set_priority(target, priority),
        )
    }

    fn suspend(&self, target: &Target) -> Result<()> {
        self.refuse_own(target)?;
        self.or_helper(
            || Pidfd::open(target)?.signal(libc::SIGSTOP),
            |h| h.suspend(target),
        )
    }

    fn resume(&self, target: &Target) -> Result<()> {
        self.or_helper(
            || Pidfd::open(target)?.signal(libc::SIGCONT),
            |h| h.resume(target),
        )
    }

    fn kill(&self, target: &Target) -> Result<()> {
        self.refuse_own(target)?;
        self.or_helper(
            || Pidfd::open(target)?.signal(libc::SIGKILL),
            |h| h.kill(target),
        )
    }

    /// Linux no tiene una ventana en primer plano común a X11 y Wayland.
    fn foreground(&self) -> Result<Option<Target>> {
        Ok(None)
    }

    fn suspended(&self) -> Result<Vec<u32>> {
        stopped_pids()
    }
}

/// Solo la afinidad; lo demás es de Windows.
impl GameTuning for LinuxPlatform {
    /// La del hilo principal.
    fn affinity(&self, target: &Target) -> Result<Vec<usize>> {
        let pidfd = Pidfd::open(target)?;
        let cpus = get_affinity(main_thread(target)?).map_err(|e| os_error(e, target.pid))?;
        pidfd.still_alive()?;
        Ok(cpus)
    }

    /// En todos los hilos.
    fn set_affinity(&self, target: &Target, cpus: &[usize]) -> Result<()> {
        let set = cpu_set(cpus)?;
        Pidfd::open(target)?.each_thread(|tid| set_affinity(tid, &set))
    }

    fn compat_flags(&self, _exe: &Path) -> Result<Option<String>> {
        Err(PlatformError::Unsupported("flags de compatibilidad"))
    }

    fn disable_fullscreen_optimizations(&self, _exe: &Path) -> Result<()> {
        Err(PlatformError::Unsupported(
            "optimizaciones de pantalla completa",
        ))
    }

    fn clear_compat_flags(&self, _exe: &Path) -> Result<()> {
        Err(PlatformError::Unsupported("flags de compatibilidad"))
    }

    fn gpu_preference(&self, _exe: &Path) -> Result<Option<String>> {
        Err(PlatformError::Unsupported("preferencia de GPU"))
    }

    fn prefer_high_performance_gpu(&self, _exe: &Path) -> Result<()> {
        Err(PlatformError::Unsupported("preferencia de GPU"))
    }

    fn clear_gpu_preference(&self, _exe: &Path) -> Result<()> {
        Err(PlatformError::Unsupported("preferencia de GPU"))
    }

    fn read_persistent_priority(&self, _exe: &Path) -> Result<Vec<SavedValue>> {
        Err(PlatformError::Unsupported("prioridad persistente"))
    }

    fn set_persistent_priority(&self, _exe: &Path) -> Result<()> {
        Err(PlatformError::Unsupported("prioridad persistente"))
    }

    fn restore_persistent_priority(&self, _exe: &Path, _saved: &[SavedValue]) -> Result<()> {
        Err(PlatformError::Unsupported("prioridad persistente"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// Un `sleep` propio que se cierra al soltarlo.
    struct Sleeper(Child);

    impl Sleeper {
        fn spawn() -> Self {
            Self(Command::new("sleep").arg("60").spawn().unwrap())
        }

        fn target(&self) -> Target {
            target_of(self.0.id()).unwrap()
        }

        fn state(&self) -> Option<char> {
            let stat = std::fs::read_to_string(format!("/proc/{}/stat", self.0.id())).ok()?;
            stat_state(&stat)
        }

        /// Espera (como mucho 5 s) a que el estado sea `want`: la señal llega sin esperar.
        fn wait_state(&self, want: char) -> bool {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if self.state() == Some(want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }
    }

    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn state_follows_the_last_parenthesis() {
        assert_eq!(stat_state("42 (sleep) S 1 42 42"), Some('S'));
        assert_eq!(stat_state("42 (a) T (b)) T 1 42"), Some('T'));
        assert_eq!(stat_state("42 (sin cierre"), None);
    }

    #[test]
    fn nice_round_trips_and_rounds_to_the_nearest() {
        use Priority::*;
        for p in [Idle, BelowNormal, Normal, AboveNormal, High, Realtime] {
            assert_eq!(priority_of(nice_of(p)), p);
        }
        assert_eq!(priority_of(3), Normal);
        assert_eq!(priority_of(5), BelowNormal);
        assert_eq!(priority_of(15), Idle);
        assert_eq!(priority_of(-15), High);
        assert_eq!(priority_of(-16), Realtime);
    }

    #[test]
    fn cpu_set_rejects_empty_and_out_of_range() {
        assert!(cpu_set(&[]).is_err());
        assert!(cpu_set(&[CPU_SETSIZE]).is_err());
        assert!(cpu_set(&[0, 1]).is_ok());
    }

    #[test]
    fn suspend_and_resume_a_child() {
        let child = Sleeper::spawn();
        let target = child.target();
        let linux = LinuxPlatform::new();
        linux.suspend(&target).unwrap();
        assert!(child.wait_state('T'));
        assert!(linux.suspended().unwrap().contains(&target.pid));
        linux.resume(&target).unwrap();
        assert!(child.wait_state('S'));
        assert!(!linux.suspended().unwrap().contains(&target.pid));
    }

    /// Bajar la prioridad no pide privilegios; volver a Normal (bajar el nice) puede pedirlos.
    #[test]
    fn lower_priority_of_a_child() {
        let child = Sleeper::spawn();
        let target = child.target();
        let linux = LinuxPlatform::new();
        assert_eq!(
            linux.priority(&target).unwrap(),
            priority_of(get_nice(0).unwrap())
        );
        linux.set_priority(&target, Priority::Idle).unwrap();
        assert_eq!(linux.priority(&target).unwrap(), Priority::Idle);
        match linux.set_priority(&target, Priority::Normal) {
            Ok(()) => assert_eq!(linux.priority(&target).unwrap(), Priority::Normal),
            Err(e) => assert!(matches!(e, PlatformError::AccessDenied(_)), "{e}"),
        }
    }

    #[test]
    fn affinity_of_a_child() {
        let child = Sleeper::spawn();
        let target = child.target();
        let linux = LinuxPlatform::new();
        let all = linux.affinity(&target).unwrap();
        assert!(!all.is_empty());
        linux.set_affinity(&target, &all[..1]).unwrap();
        assert_eq!(linux.affinity(&target).unwrap(), all[..1]);
        linux.set_affinity(&target, &all).unwrap();
        assert_eq!(linux.affinity(&target).unwrap(), all);
    }

    #[test]
    fn kill_a_child() {
        let mut child = Sleeper::spawn();
        LinuxPlatform::new().kill(&child.target()).unwrap();
        let status = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn a_changed_target_is_not_touched() {
        let child = Sleeper::spawn();
        let mut target = child.target();
        target.start_time.0 += 1;
        let linux = LinuxPlatform::new();
        assert_eq!(
            linux.suspend(&target),
            Err(PlatformError::TargetChanged { pid: target.pid })
        );
        assert!(!linux.is_same_process(&target).unwrap());
        assert_ne!(child.state(), Some('T'));
    }

    #[test]
    fn refuses_to_stop_itself() {
        let me = target_of(std::process::id()).unwrap();
        let linux = LinuxPlatform::new();
        assert!(matches!(
            linux.suspend(&me),
            Err(PlatformError::AccessDenied(_))
        ));
        assert!(!linux.suspended().unwrap().contains(&me.pid));
    }
}
