//! Helper elevado en Linux (PLAN.md It2.1): un proceso aparte, lanzado con `pkexec` cuando hace
//! falta, que hace solo lo que pide privilegios.
//!
//! - El cliente (CLI o GUI, sin privilegios) escucha en un socket Unix nuevo dentro de
//!   `$XDG_RUNTIME_DIR`, que es solo suyo (modo 0700, XDG Base Directory Specification), y
//!   lanza `pkexec <helper> --socket <ruta> --parent <su PID>`. polkit pide la contraseña de
//!   administrador (acción por defecto `org.freedesktop.policykit.exec`, pkexec(1)).
//! - El helper solo arranca si lo lanzó `pkexec` como root: se fía de `PKEXEC_UID`, que pkexec
//!   pone con el usuario que lo invocó. Comprueba que la carpeta del socket es de ese usuario y
//!   0700 y que el socket es suyo, se conecta y, con `SO_PEERCRED` (unix(7): las credenciales
//!   del otro extremo al hacer `listen`), que quien escucha es el PID `--parent` de ese usuario.
//! - El cliente solo acepta una conexión de root (`SO_PEERCRED`) y borra el socket en cuanto se
//!   conecta.
//! - Protocolo: una petición JSON por línea y una respuesta por línea, con comandos tipados
//!   ([`Request`]): acciones sobre procesos que el cliente no puede hacer (subir la prioridad,
//!   procesos de otro usuario, P12) y los servicios de las reglas. Los procesos pasan por la
//!   guardia de quien lanza el helper (la lista de protegidos de `core`) y nunca son el cliente.
//! - El journal lo sigue escribiendo el cliente. El helper termina cuando se cierra el socket,
//!   también si el cliente muere.
#![allow(unsafe_code)]

use super::LinuxPlatform;
use crate::{PlatformError, Priority, ProcessControl, Result, ServiceControl, Target};
use serde::{Deserialize, Serialize};
use std::fs::Metadata;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Lo único que el helper sabe hacer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Request {
    SetPriority(Target, Priority),
    Suspend(Target),
    Resume(Target),
    Kill(Target),
    /// Servicios de las reglas: detener durante la sesión y volver a arrancar.
    StopService(String),
    StartService(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Response {
    Done,
    Failed(String),
}

/// Qué puede tocar el helper.
#[derive(Debug, Clone, Copy)]
struct Policy {
    /// El cliente: nunca se congela, cierra ni cambia de prioridad desde aquí.
    parent: u32,
    /// Guardia de quien lanza el helper: `false` para un proceso protegido.
    may_touch: fn(&Target) -> bool,
    /// Lista blanca de servicios (los de las reglas).
    may_stop_service: fn(&str) -> bool,
}

/// Lo que se espera a que el usuario escriba la contraseña en el diálogo de polkit.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

/// `pkexec` sale con 126 si el usuario cerró el diálogo y con 127 si no hubo autorización o
/// falló (pkexec(1), «RETURN VALUE»).
const PKEXEC_DISMISSED: i32 = 126;
const PKEXEC_NOT_AUTHORIZED: i32 = 127;

fn io_error(what: &str, e: std::io::Error) -> PlatformError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => PlatformError::AccessDenied(format!("{what}: {e}")),
        _ => PlatformError::Os(format!("{what}: {e}")),
    }
}

fn uid() -> u32 {
    // SAFETY: getuid no toma argumentos y siempre tiene éxito (getuid(2)).
    unsafe { libc::getuid() }
}

fn euid() -> u32 {
    // SAFETY: geteuid no toma argumentos y siempre tiene éxito (getuid(2)).
    unsafe { libc::geteuid() }
}

/// PID y usuario del otro extremo de `stream` (`SO_PEERCRED`, unix(7)).
fn peer(stream: &UnixStream) -> Result<(u32, u32)> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: el descriptor es del socket conectado `stream`, `cred` es un `ucred` vivo durante
    // la llamada y `len` es su tamaño, como pide getsockopt(2) para SO_PEERCRED.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    if r != 0 {
        return Err(io_error("SO_PEERCRED", std::io::Error::last_os_error()));
    }
    let pid = u32::try_from(cred.pid)
        .map_err(|_| PlatformError::Os(format!("SO_PEERCRED: PID {}", cred.pid)))?;
    Ok((pid, cred.uid))
}

/// Carpeta solo de `owner`: es una carpeta (no un enlace), suya y con modo 0700.
fn private_dir(meta: &Metadata, owner: u32) -> bool {
    meta.file_type().is_dir() && meta.uid() == owner && meta.mode() & 0o777 == 0o700
}

/// `$XDG_RUNTIME_DIR` si es una carpeta privada del usuario actual.
fn runtime_dir() -> Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .ok_or_else(|| PlatformError::Os("helper: no hay XDG_RUNTIME_DIR".into()))?;
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| io_error("XDG_RUNTIME_DIR", e))?;
    if !private_dir(&meta, uid()) {
        return Err(PlatformError::Os(format!(
            "helper: {} no es una carpeta privada (0700) del usuario",
            dir.display()
        )));
    }
    Ok(dir)
}

/// Nombre que no se repite, para no chocar con un socket que ya exista.
fn socket_path(dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    dir.join(format!(
        "framerush-helper-{}-{nanos}.sock",
        std::process::id()
    ))
}

/// Conexión con un helper elevado. Las copias comparten la conexión (una sola contraseña); al
/// soltar la última se cierra el socket y el helper termina.
#[derive(Debug, Clone)]
pub struct Helper(Arc<Connection>);

#[derive(Debug)]
struct Connection {
    conn: Mutex<BufReader<UnixStream>>,
    pid: u32,
    /// `pkexec`: se recoge al soltar la conexión para no dejar un zombi.
    child: Mutex<Child>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // El helper sale al ver el socket cerrado; `conn` se cierra antes de esperar.
        let _ = self
            .conn
            .get_mut()
            .map(|c| c.get_ref().shutdown(std::net::Shutdown::Both));
        if let Ok(child) = self.child.get_mut() {
            let _ = child.wait();
        }
    }
}

impl Helper {
    /// Lanza `exe` con `pkexec` (aparece el diálogo de polkit) y espera a que se conecte. Si el
    /// usuario cancela o no está autorizado, devuelve `AccessDenied`.
    pub fn launch(exe: &Path) -> Result<Helper> {
        let path = socket_path(&runtime_dir()?);
        let listener = UnixListener::bind(&path).map_err(|e| io_error("socket del helper", e))?;
        let result = Self::wait_for(&listener, exe, &path);
        // Conectado o no, el socket ya no hace falta en el disco.
        let _ = std::fs::remove_file(&path);
        result
    }

    fn wait_for(listener: &UnixListener, exe: &Path, path: &Path) -> Result<Helper> {
        listener
            .set_nonblocking(true)
            .map_err(|e| io_error("socket del helper", e))?;
        let mut child = Command::new("pkexec")
            .arg(exe)
            .arg("--socket")
            .arg(path)
            .arg("--parent")
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => PlatformError::Unsupported("pkexec (polkit)"),
                _ => io_error("pkexec", e),
            })?;
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            match listener.accept() {
                // Solo root: en una carpeta 0700 del usuario solo pueden entrar él y root.
                Ok((stream, _)) => match peer(&stream) {
                    Ok((pid, 0)) => {
                        stream
                            .set_nonblocking(false)
                            .map_err(|e| io_error("socket del helper", e))?;
                        return Ok(Helper(Arc::new(Connection {
                            conn: Mutex::new(BufReader::new(stream)),
                            pid,
                            child: Mutex::new(child),
                        })));
                    }
                    _ => continue,
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(io_error("socket del helper", e)),
            }
            if let Some(status) = child.try_wait().map_err(|e| io_error("pkexec", e))? {
                return Err(match status.code() {
                    Some(PKEXEC_DISMISSED) => {
                        PlatformError::AccessDenied("se canceló la autenticación de polkit".into())
                    }
                    Some(PKEXEC_NOT_AUTHORIZED) => {
                        PlatformError::AccessDenied("polkit no autorizó el helper".into())
                    }
                    _ => PlatformError::Os(format!("el helper terminó: {status}")),
                });
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PlatformError::Os(format!(
                    "el helper no se conectó en {} s",
                    CONNECT_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// PID del proceso del helper.
    pub(super) fn pid(&self) -> u32 {
        self.0.pid
    }

    pub(super) fn set_priority(&self, target: &Target, priority: Priority) -> Result<()> {
        self.call(&Request::SetPriority(target.clone(), priority))
    }

    pub(super) fn suspend(&self, target: &Target) -> Result<()> {
        self.call(&Request::Suspend(target.clone()))
    }

    pub(super) fn resume(&self, target: &Target) -> Result<()> {
        self.call(&Request::Resume(target.clone()))
    }

    pub(super) fn kill(&self, target: &Target) -> Result<()> {
        self.call(&Request::Kill(target.clone()))
    }

    pub(super) fn stop_service(&self, name: &str) -> Result<()> {
        self.call(&Request::StopService(name.to_owned()))
    }

    pub(super) fn start_service(&self, name: &str) -> Result<()> {
        self.call(&Request::StartService(name.to_owned()))
    }

    /// Envía `request` y lee la respuesta; `Failed` pasa a error.
    fn call(&self, request: &Request) -> Result<()> {
        let mut conn = self
            .0
            .conn
            .lock()
            .map_err(|_| PlatformError::Os("conexión con el helper rota".into()))?;
        exchange(&mut conn, request)
    }
}

fn exchange(conn: &mut BufReader<UnixStream>, request: &Request) -> Result<()> {
    let mut line = serde_json::to_string(request)
        .map_err(|e| PlatformError::Os(format!("petición al helper: {e}")))?;
    line.push('\n');
    conn.get_mut()
        .write_all(line.as_bytes())
        .map_err(|e| io_error("escribir al helper", e))?;
    line.clear();
    if conn
        .read_line(&mut line)
        .map_err(|e| io_error("leer del helper", e))?
        == 0
    {
        return Err(PlatformError::Os("el helper se cerró".into()));
    }
    match serde_json::from_str(&line) {
        Ok(Response::Done) => Ok(()),
        Ok(Response::Failed(e)) => Err(PlatformError::Os(format!("helper: {e}"))),
        Err(e) => Err(PlatformError::Os(format!("respuesta del helper: {e}"))),
    }
}

/// Lado del helper: se conecta al socket del cliente `parent` y atiende sus peticiones con la
/// plataforma de este proceso (root) hasta que lo cierre. `may_touch` decide qué procesos se
/// pueden congelar, cerrar o cambiar de prioridad (la lista de protegidos vive en `core`, por
/// encima de este crate) y `may_stop_service`, qué servicios.
pub fn serve_helper(
    socket: &Path,
    parent: u32,
    may_touch: fn(&Target) -> bool,
    may_stop_service: fn(&str) -> bool,
) -> Result<()> {
    let user = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|u| u.parse::<u32>().ok())
        .ok_or_else(|| PlatformError::AccessDenied("el helper solo se lanza con pkexec".into()))?;
    if euid() != 0 {
        return Err(PlatformError::AccessDenied(
            "el helper tiene que correr como root".into(),
        ));
    }
    check_socket(socket, user)?;
    let stream = UnixStream::connect(socket).map_err(|e| io_error("socket del cliente", e))?;
    let (pid, peer_uid) = peer(&stream)?;
    if pid != parent || peer_uid != user {
        return Err(PlatformError::AccessDenied(format!(
            "el socket no es del cliente (PID {pid}, usuario {peer_uid})"
        )));
    }
    let policy = Policy {
        parent,
        may_touch,
        may_stop_service,
    };
    serve(stream, &LinuxPlatform::new(), policy)
}

/// El socket es de `user` y está en una carpeta privada suya (sin seguir enlaces).
fn check_socket(socket: &Path, user: u32) -> Result<()> {
    let dir = socket
        .parent()
        .filter(|_| socket.is_absolute())
        .ok_or_else(|| PlatformError::AccessDenied("ruta del socket no válida".into()))?;
    let dir_meta = std::fs::symlink_metadata(dir).map_err(|e| io_error("carpeta del socket", e))?;
    let meta = std::fs::symlink_metadata(socket).map_err(|e| io_error("socket del cliente", e))?;
    if !private_dir(&dir_meta, user) || !meta.file_type().is_socket() || meta.uid() != user {
        return Err(PlatformError::AccessDenied(format!(
            "{} no es un socket privado del usuario {user}",
            socket.display()
        )));
    }
    Ok(())
}

fn serve<T: ProcessControl + ServiceControl>(
    stream: UnixStream,
    platform: &T,
    policy: Policy,
) -> Result<()> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| io_error("socket del cliente", e))?,
    );
    let mut writer = stream;
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .map_err(|e| io_error("leer del cliente", e))?
            == 0
        {
            return Ok(());
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(request, platform, policy),
            Err(e) => Response::Failed(format!("petición no válida: {e}")),
        };
        let mut out = serde_json::to_string(&response)
            .map_err(|e| PlatformError::Os(format!("respuesta: {e}")))?;
        out.push('\n');
        writer
            .write_all(out.as_bytes())
            .map_err(|e| io_error("escribir al cliente", e))?;
    }
}

fn handle<T: ProcessControl + ServiceControl>(
    request: Request,
    platform: &T,
    policy: Policy,
) -> Response {
    if let Request::SetPriority(t, _) | Request::Suspend(t) | Request::Kill(t) = &request {
        if t.pid == policy.parent {
            return Response::Failed("el helper no actúa sobre su cliente".into());
        }
        if !(policy.may_touch)(t) {
            return Response::Failed(format!("{} está protegido", t.exe_path.display()));
        }
    }
    if let Request::StopService(name) | Request::StartService(name) = &request
        && !(policy.may_stop_service)(name)
    {
        return Response::Failed(format!("el servicio {name} no está en las reglas"));
    }
    let result = match request {
        Request::SetPriority(t, p) => platform.set_priority(&t, p),
        Request::Suspend(t) => platform.suspend(&t),
        Request::Resume(t) => platform.resume(&t),
        Request::Kill(t) => platform.kill(&t),
        Request::StopService(name) => platform.stop_service(&name),
        Request::StartService(name) => platform.start_service(&name),
    };
    match result {
        Ok(()) => Response::Done,
        Err(e) => Response::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockPlatform;
    use crate::{ServiceState, StartTime};

    fn target(pid: u32, exe: &str) -> Target {
        Target {
            pid,
            start_time: StartTime(1),
            exe_path: exe.into(),
        }
    }

    fn policy() -> Policy {
        Policy {
            parent: 10,
            may_touch: |t| !t.exe_path.ends_with("systemd"),
            may_stop_service: |s| s == "cups",
        }
    }

    #[test]
    fn refuses_the_client_protected_processes_and_services_outside_the_rules() {
        let m = MockPlatform::new(4);
        let p = policy();
        let client = target(10, "/usr/bin/framerush");
        let init = target(1, "/usr/lib/systemd/systemd");
        for r in [
            Request::Suspend(client.clone()),
            Request::Kill(client.clone()),
            Request::SetPriority(client, Priority::High),
            Request::Suspend(init),
            Request::StopService("sshd".into()),
            Request::StartService("sshd".into()),
        ] {
            assert!(
                matches!(handle(r.clone(), &m, p), Response::Failed(_)),
                "{r:?}"
            );
        }
    }

    #[test]
    fn serves_requests_over_the_socket_until_it_closes() {
        let m = Arc::new(MockPlatform::new(4));
        m.add_service("cups", ServiceState::Running);
        let (client, server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn({
            let m = Arc::clone(&m);
            move || serve(server, &*m, policy())
        });
        let mut conn = BufReader::new(client);
        exchange(&mut conn, &Request::StopService("cups".into())).unwrap();
        assert!(exchange(&mut conn, &Request::StopService("sshd".into())).is_err());
        drop(conn);
        worker.join().unwrap().unwrap();
        assert_eq!(
            m.service_state("cups").unwrap(),
            Some(ServiceState::Stopped)
        );
    }

    #[test]
    fn a_private_dir_is_a_real_dir_of_the_user_with_mode_0700() {
        let dir = std::env::temp_dir().join(format!("framerush-helper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        assert!(private_dir(&meta, uid()));
        assert!(!private_dir(&meta, uid() + 1));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!private_dir(
            &std::fs::symlink_metadata(&dir).unwrap(),
            uid()
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
