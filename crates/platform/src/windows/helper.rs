//! Helper elevado (PLAN.md It2.1): un proceso aparte, lanzado con UAC cuando hace falta, que
//! aplica y restaura solo los ajustes que piden admin.
//!
//! - El cliente (CLI o GUI, sin elevar) lanza `framerush-helper` con el verbo `runas` y le pasa
//!   un nombre de pipe aleatorio, su PID y el SID de su usuario.
//! - El helper crea el pipe con una DACL que solo deja entrar a ese usuario (y a SYSTEM), sin
//!   clientes remotos, y solo acepta como cliente a ese PID. El cliente comprueba a su vez que
//!   el servidor es el proceso que lanzó.
//! - Protocolo: una petición JSON por línea y una respuesta por línea. Solo hay comandos
//!   tipados ([`Request`]): ajustes que piden admin y acciones sobre procesos que el cliente
//!   no puede tocar (de otro usuario o de SYSTEM, P12). Los procesos pasan por la guardia de
//!   quien lanza el helper (la lista de protegidos de `core`) y nunca son el propio cliente.
//! - El journal lo sigue escribiendo el cliente antes de cada petición. El helper termina al
//!   cerrarse el pipe o al terminar el proceso que lo lanzó.
#![allow(unsafe_code)]

use super::AcerQuery;
use crate::{
    DefenderExclusion, GameTuning, MemoryControl, PlatformError, Priority, ProcessControl, Result,
    SavedValue, SecurityTweaks, ServiceControl, SystemTweaks, Target, Tweak,
};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessId, INFINITE, OpenProcess, OpenProcessToken, PROCESS_SYNCHRONIZE,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

/// Lo único que el helper sabe hacer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Request {
    /// Lectura de un ajuste que solo se lee elevado (el arranque de `vbs-hvci-off`).
    ReadTweak(Tweak),
    ApplyTweak(Tweak),
    RestoreTweak(Tweak, Vec<SavedValue>),
    /// `game-priority-persistent` del exe.
    SetPersistentPriority(PathBuf),
    RestorePersistentPriority(PathBuf, Vec<SavedValue>),
    /// `memory-trim` de un proceso.
    TrimWorkingSet(Target),
    /// `standby-purge`.
    PurgeStandbyList,
    /// Acciones sobre un proceso que el cliente no puede tocar sin elevar.
    SetPriority(Target, Priority),
    Suspend(Target),
    Resume(Target),
    Kill(Target),
    /// Lectura de los sensores del EC de Acer (solo lectura: `AcerQuery` no admite otras
    /// órdenes).
    Acer(AcerQuery),
    /// Servicios de las reglas: detener durante la sesión y volver a arrancar.
    StopService(String),
    StartService(String),
    /// Exclusiones de Defender del juego (lista blanca: `may_exclude`).
    HasDefenderExclusion(DefenderExclusion),
    AddDefenderExclusion(DefenderExclusion),
    RemoveDefenderExclusion(DefenderExclusion),
}

/// Qué procesos puede tocar el helper.
#[derive(Debug, Clone, Copy)]
struct Policy {
    /// El cliente: nunca se congela, cierra ni cambia de prioridad desde aquí.
    parent: u32,
    /// Guardia de quien lanza el helper: `false` para un proceso protegido.
    may_touch: fn(&Target) -> bool,
    /// Lista blanca de servicios que se pueden detener y arrancar (los de las reglas).
    may_stop_service: fn(&str) -> bool,
    /// Lista blanca de exclusiones de Defender: solo carpetas y exes de juegos instalados.
    may_exclude: fn(&DefenderExclusion) -> bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Response {
    Done,
    /// Resultado de una lectura.
    Value(u64),
    /// Estado de un ajuste ([`Request::ReadTweak`]).
    Saved(Vec<SavedValue>),
    Failed(String),
}

/// Cuánto se espera a que el helper cree el pipe una vez aceptado el UAC.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn os_error(what: &str) -> PlatformError {
    PlatformError::Os(format!("{what}: {}", std::io::Error::last_os_error()))
}

fn io_error(what: &str, e: std::io::Error) -> PlatformError {
    PlatformError::Os(format!("{what}: {e}"))
}

fn owned(raw: HANDLE) -> OwnedHandle {
    // SAFETY: quien llama pasa un handle válido y propio que no cierra nadie más.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

/// Conexión con un helper elevado. Las copias comparten la conexión (un solo UAC); al soltar
/// la última se cierra el pipe y el helper termina.
#[derive(Debug, Clone)]
pub struct Helper(Arc<Connection>);

#[derive(Debug)]
struct Connection {
    conn: Mutex<BufReader<File>>,
    /// Handle del proceso del helper (mantiene válido su PID mientras dure la conexión).
    _process: Option<OwnedHandle>,
    pid: Option<u32>,
}

impl Helper {
    /// Lanza `exe` elevado (aparece el UAC) y se conecta a él. Si el usuario cancela el UAC,
    /// devuelve `AccessDenied`.
    pub fn launch(exe: &Path) -> Result<Helper> {
        let pipe = pipe_name();
        let args = format!(
            "--pipe {pipe} --parent {} --sid {}",
            std::process::id(),
            current_user_sid()?
        );
        let process = run_elevated(exe, &args)?;
        // SAFETY: `process` es un handle de proceso abierto.
        let pid = unsafe { GetProcessId(process.as_raw_handle()) };
        let file = connect(&pipe, pid, Some(&process))?;
        Ok(Helper(Arc::new(Connection {
            conn: Mutex::new(BufReader::new(file)),
            _process: Some(process),
            pid: Some(pid),
        })))
    }

    /// PID del proceso del helper.
    pub(super) fn pid(&self) -> Option<u32> {
        self.0.pid
    }

    pub(super) fn read_tweak(&self, tweak: Tweak) -> Result<Vec<SavedValue>> {
        match self.exchange(&Request::ReadTweak(tweak))? {
            Response::Saved(saved) => Ok(saved),
            other => Err(PlatformError::Os(format!(
                "respuesta del helper: {other:?}"
            ))),
        }
    }

    pub(super) fn apply_tweak(&self, tweak: Tweak) -> Result<()> {
        self.call(&Request::ApplyTweak(tweak))
    }

    pub(super) fn restore_tweak(&self, tweak: Tweak, saved: &[SavedValue]) -> Result<()> {
        self.call(&Request::RestoreTweak(tweak, saved.to_vec()))
    }

    pub(super) fn set_persistent_priority(&self, exe: &Path) -> Result<()> {
        self.call(&Request::SetPersistentPriority(exe.to_path_buf()))
    }

    pub(super) fn restore_persistent_priority(
        &self,
        exe: &Path,
        saved: &[SavedValue],
    ) -> Result<()> {
        self.call(&Request::RestorePersistentPriority(
            exe.to_path_buf(),
            saved.to_vec(),
        ))
    }

    pub(super) fn trim_working_set(&self, target: &Target) -> Result<()> {
        self.call(&Request::TrimWorkingSet(target.clone()))
    }

    pub(super) fn purge_standby_list(&self) -> Result<()> {
        self.call(&Request::PurgeStandbyList)
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

    pub(super) fn has_defender_exclusion(&self, exclusion: &DefenderExclusion) -> Result<bool> {
        match self.exchange(&Request::HasDefenderExclusion(exclusion.clone()))? {
            Response::Value(v) => Ok(v != 0),
            other => Err(PlatformError::Os(format!(
                "respuesta del helper: {other:?}"
            ))),
        }
    }

    pub(super) fn add_defender_exclusion(&self, exclusion: &DefenderExclusion) -> Result<()> {
        self.call(&Request::AddDefenderExclusion(exclusion.clone()))
    }

    pub(super) fn remove_defender_exclusion(&self, exclusion: &DefenderExclusion) -> Result<()> {
        self.call(&Request::RemoveDefenderExclusion(exclusion.clone()))
    }

    pub(super) fn acer(&self, query: AcerQuery) -> Result<u64> {
        match self.exchange(&Request::Acer(query))? {
            Response::Value(v) => Ok(v),
            other => Err(PlatformError::Os(format!(
                "respuesta del helper: {other:?}"
            ))),
        }
    }

    fn call(&self, request: &Request) -> Result<()> {
        match self.exchange(request)? {
            Response::Done => Ok(()),
            other => Err(PlatformError::Os(format!(
                "respuesta del helper: {other:?}"
            ))),
        }
    }

    /// Envía `request` y lee la respuesta; `Failed` pasa a error.
    fn exchange(&self, request: &Request) -> Result<Response> {
        let mut conn = self
            .0
            .conn
            .lock()
            .map_err(|_| PlatformError::Os("conexión con el helper rota".into()))?;
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
            Ok(Response::Failed(e)) => Err(PlatformError::Os(format!("helper: {e}"))),
            Ok(response) => Ok(response),
            Err(e) => Err(PlatformError::Os(format!("respuesta del helper: {e}"))),
        }
    }
}

/// Lado del helper: espera al cliente `parent` en `pipe` y atiende sus peticiones con la
/// plataforma de este proceso (elevado) hasta que cierre el pipe. Termina el proceso si
/// `parent` termina antes. `may_touch` decide qué procesos se pueden congelar, cerrar o cambiar
/// de prioridad (la lista de protegidos vive en `core`, por encima de este crate).
pub fn serve_helper(
    pipe: &str,
    parent: u32,
    sid: &str,
    may_touch: fn(&Target) -> bool,
    may_stop_service: fn(&str) -> bool,
    may_exclude: fn(&DefenderExclusion) -> bool,
) -> Result<()> {
    exit_with(parent)?;
    let file = listen(pipe, parent, sid)?;
    let policy = Policy {
        parent,
        may_touch,
        may_stop_service,
        may_exclude,
    };
    serve(file, &super::WindowsPlatform::new(), policy)
}

/// Nombre aleatorio para que otro proceso no pueda adelantarse a crearlo.
fn pipe_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(r"\\.\pipe\framerush-helper-{}-{nanos}", std::process::id())
}

/// SID del usuario de este proceso, como cadena (`S-1-5-21-…`).
fn current_user_sid() -> Result<String> {
    let mut raw: HANDLE = null_mut();
    // SAFETY: GetCurrentProcess devuelve un pseudo-handle; `raw` es un local vivo.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(os_error("abrir el token del proceso"));
    }
    let token = owned(raw);
    let mut len = 0u32;
    // SAFETY: con buffer nulo y tamaño 0 solo escribe en `len` el tamaño necesario.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut len) };
    // u64 para que el TOKEN_USER quede alineado.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` tiene al menos `len` bytes y vive durante la llamada.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(os_error("leer el usuario del token"));
    }
    // SAFETY: la llamada anterior escribió un TOKEN_USER al principio de `buf`, que está
    // alineado y sigue vivo mientras se usa el SID.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = null_mut();
    // SAFETY: `sid` apunta dentro de `buf`; `text` recibe una cadena que se libera abajo.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(os_error("convertir el SID"));
    }
    // SAFETY: `text` es una cadena UTF-16 terminada en nulo devuelta por el sistema.
    let s = unsafe {
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    };
    // SAFETY: `text` se reservó con LocalAlloc (documentado) y no se usa después.
    unsafe { LocalFree(text.cast()) };
    Ok(s)
}

/// Lanza `exe args` con el verbo `runas` (UAC) y devuelve el handle de su proceso.
fn run_elevated(exe: &Path, args: &str) -> Result<OwnedHandle> {
    // ShellExecuteEx puede delegar en extensiones del shell, que piden COM inicializado. Si ya
    // lo estaba en otro modo, la llamada falla sin consecuencias.
    // SAFETY: el puntero reservado debe ser nulo.
    unsafe {
        CoInitializeEx(
            null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(args));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: params.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // SAFETY: `info` está inicializado y las cadenas a las que apunta viven durante la llamada.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: sin argumentos.
        return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
            PlatformError::AccessDenied("se canceló la elevación (UAC)".into())
        } else {
            os_error("lanzar el helper elevado")
        });
    }
    if info.hProcess.is_null() {
        return Err(PlatformError::Os("el helper se lanzó sin proceso".into()));
    }
    Ok(owned(info.hProcess))
}

/// Abre el pipe del helper (esperando a que exista) y comprueba que lo sirve `server_pid`.
/// Si `process` termina antes, deja de esperar.
fn connect(pipe: &str, server_pid: u32, process: Option<&OwnedHandle>) -> Result<File> {
    let start = Instant::now();
    let file = loop {
        match File::options().read(true).write(true).open(pipe) {
            Ok(f) => break f,
            Err(e) => {
                // SAFETY: `p` es un handle de proceso abierto; espera 0 ms.
                let gone = process.is_some_and(
                    |p| unsafe { WaitForSingleObject(p.as_raw_handle(), 0) } == WAIT_OBJECT_0,
                );
                if gone || start.elapsed() > CONNECT_TIMEOUT {
                    return Err(io_error("conectar con el helper", e));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let mut pid = 0u32;
    // SAFETY: `file` es el extremo cliente de un pipe; `pid` es un local vivo.
    if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle(), &mut pid) } == 0 {
        return Err(os_error("leer el servidor del pipe"));
    }
    if pid != server_pid {
        return Err(PlatformError::AccessDenied(format!(
            "el pipe lo sirve el PID {pid}, no el helper"
        )));
    }
    Ok(file)
}

/// Un SID en texto solo tiene `S`, dígitos y guiones: así no puede alterar el SDDL.
fn valid_sid(sid: &str) -> bool {
    sid.starts_with("S-1-")
        && sid
            .chars()
            .all(|c| c == 'S' || c == '-' || c.is_ascii_digit())
}

/// Crea el pipe solo para `sid`, espera un cliente y comprueba que es `parent`.
fn listen(pipe: &str, parent: u32, sid: &str) -> Result<File> {
    if !valid_sid(sid) {
        return Err(PlatformError::AccessDenied(format!(
            "SID no válido: `{sid}`"
        )));
    }
    // Acceso total para SYSTEM y el usuario del cliente; etiqueta de integridad media para que
    // un cliente sin elevar pueda escribir en el pipe de un proceso elevado.
    let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})S:(ML;;NW;;;ME)"));
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: `sddl` termina en nulo; `sd` recibe un descriptor que se libera abajo.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(os_error("crear la ACL del pipe"));
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let name = wide(pipe);
    // SAFETY: `name` termina en nulo y `sa` apunta a un descriptor válido durante la llamada.
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            &sa,
        )
    };
    // SAFETY: `sd` lo reservó el sistema con LocalAlloc y el pipe ya copió lo que necesita.
    unsafe { LocalFree(sd) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(os_error("crear el pipe del helper"));
    }
    let server = owned(raw);
    // SAFETY: `server` es un pipe abierto en modo síncrono, así que no hace falta OVERLAPPED.
    if unsafe { ConnectNamedPipe(server.as_raw_handle(), null_mut()) } == 0
        // SAFETY: sin argumentos.
        && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED
    {
        return Err(os_error("esperar al cliente del helper"));
    }
    let mut client = 0u32;
    // SAFETY: `server` es un pipe conectado; `client` es un local vivo.
    if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut client) } == 0 {
        return Err(os_error("leer el cliente del pipe"));
    }
    if client != parent {
        return Err(PlatformError::AccessDenied(format!(
            "cliente inesperado del helper: PID {client}"
        )));
    }
    Ok(File::from(server))
}

/// Termina este proceso cuando termine `parent` (también si aún no se ha conectado).
fn exit_with(parent: u32) -> Result<()> {
    // SAFETY: OpenProcess no toma punteros; devuelve un handle nuevo o null.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent) };
    if raw.is_null() {
        return Err(os_error("abrir el proceso que lanzó el helper"));
    }
    let process = owned(raw);
    std::thread::spawn(move || {
        // SAFETY: `process` es un handle abierto con SYNCHRONIZE que vive en este hilo.
        unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) };
        std::process::exit(0);
    });
    Ok(())
}

/// Atiende peticiones hasta que el cliente cierre la conexión.
fn serve<
    S: Read + Write,
    T: ProcessControl + SystemTweaks + GameTuning + MemoryControl + ServiceControl + SecurityTweaks,
>(
    conn: S,
    platform: &T,
    policy: Policy,
) -> Result<()> {
    let mut conn = BufReader::new(conn);
    let mut line = String::new();
    loop {
        line.clear();
        if conn
            .read_line(&mut line)
            .map_err(|e| io_error("leer la petición", e))?
            == 0
        {
            return Ok(());
        }
        let response = match serde_json::from_str(&line) {
            Ok(request) => handle(request, platform, policy),
            Err(e) => Response::Failed(format!("petición no válida: {e}")),
        };
        let mut out = serde_json::to_string(&response)
            .map_err(|e| PlatformError::Os(format!("respuesta: {e}")))?;
        out.push('\n');
        conn.get_mut()
            .write_all(out.as_bytes())
            .map_err(|e| io_error("escribir la respuesta", e))?;
    }
}

/// Solo atiende lo que pide admin: lo demás lo hace el cliente sin elevar.
fn handle<
    T: ProcessControl + SystemTweaks + GameTuning + MemoryControl + ServiceControl + SecurityTweaks,
>(
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
    if let Request::HasDefenderExclusion(ex)
    | Request::AddDefenderExclusion(ex)
    | Request::RemoveDefenderExclusion(ex) = &request
        && !(policy.may_exclude)(ex)
    {
        return Response::Failed(format!("{ex:?} no es de un juego instalado"));
    }
    if let Request::ReadTweak(t) | Request::ApplyTweak(t) | Request::RestoreTweak(t, _) = &request
        && !platform.tweak_requires_admin(*t)
    {
        return Response::Failed(format!("`{}` no pide admin", t.id()));
    }
    let result = match request {
        Request::ReadTweak(t) => {
            return match platform.read_tweak(t) {
                Ok(saved) => Response::Saved(saved),
                Err(e) => Response::Failed(e.to_string()),
            };
        }
        Request::HasDefenderExclusion(ex) => {
            return match platform.has_defender_exclusion(&ex) {
                Ok(has) => Response::Value(u64::from(has)),
                Err(e) => Response::Failed(e.to_string()),
            };
        }
        Request::Acer(query) => {
            return match super::acer::query(query) {
                Ok(v) => Response::Value(v),
                Err(e) => Response::Failed(e.to_string()),
            };
        }
        Request::ApplyTweak(t) => platform.apply_tweak(t),
        Request::RestoreTweak(t, saved) => platform.restore_tweak(t, &saved),
        Request::SetPersistentPriority(exe) => platform.set_persistent_priority(&exe),
        Request::RestorePersistentPriority(exe, saved) => {
            platform.restore_persistent_priority(&exe, &saved)
        }
        Request::TrimWorkingSet(target) => platform.trim_working_set(&target),
        Request::PurgeStandbyList => platform.purge_standby_list(),
        Request::SetPriority(target, priority) => platform.set_priority(&target, priority),
        Request::Suspend(target) => platform.suspend(&target),
        Request::Resume(target) => platform.resume(&target),
        Request::Kill(target) => platform.kill(&target),
        Request::StopService(name) => platform.stop_service(&name),
        Request::StartService(name) => platform.start_service(&name),
        Request::AddDefenderExclusion(ex) => platform.add_defender_exclusion(&ex),
        Request::RemoveDefenderExclusion(ex) => platform.remove_defender_exclusion(&ex),
    };
    match result {
        Ok(()) => Response::Done,
        Err(e) => Response::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    //! Cliente y servidor en el mismo proceso, sin elevar; el servidor usa el mock.

    use super::*;
    use crate::mock::MockPlatform;
    use std::sync::Arc;

    /// Guardia de prueba: protegido es lo que se llama `protected.exe`.
    fn policy(parent: u32) -> Policy {
        Policy {
            parent,
            may_touch: |t| !t.exe_path.ends_with("protected.exe"),
            may_stop_service: |name| name.eq_ignore_ascii_case("WSearch"),
            may_exclude: |ex| match ex {
                DefenderExclusion::Path(p) | DefenderExclusion::Process(p) => {
                    p.starts_with(r"C:\Steam\steamapps\common")
                }
            },
        }
    }

    fn helper(file: File) -> Helper {
        Helper(Arc::new(Connection {
            conn: Mutex::new(BufReader::new(file)),
            _process: None,
            pid: None,
        }))
    }

    #[test]
    fn the_sid_is_a_user_sid() {
        let sid = current_user_sid().unwrap();
        assert!(valid_sid(&sid), "{sid}");
        assert!(!valid_sid("S-1-5)(A;;GA;;;WD"));
    }

    #[test]
    fn serves_admin_tweaks_and_refuses_the_rest() {
        let pipe = pipe_name();
        let me = std::process::id();
        let sid = current_user_sid().unwrap();
        let mock = Arc::new(MockPlatform::new(4));
        let server = {
            let (pipe, mock) = (pipe.clone(), Arc::clone(&mock));
            std::thread::spawn(move || serve(listen(&pipe, me, &sid)?, &*mock, policy(me)))
        };
        let client = helper(connect(&pipe, me, None).unwrap());

        let before = mock.read_tweak(Tweak::GameDvrPolicyOff).unwrap();
        assert_eq!(client.read_tweak(Tweak::GameDvrPolicyOff).unwrap(), before);
        client.apply_tweak(Tweak::GameDvrPolicyOff).unwrap();
        assert_ne!(mock.read_tweak(Tweak::GameDvrPolicyOff).unwrap(), before);
        client
            .restore_tweak(Tweak::GameDvrPolicyOff, &before)
            .unwrap();
        assert_eq!(mock.read_tweak(Tweak::GameDvrPolicyOff).unwrap(), before);

        let exe = Path::new(r"C:\Games\game.exe");
        client.set_persistent_priority(exe).unwrap();
        let set = mock.read_persistent_priority(exe).unwrap();
        assert_eq!(set[0].value, Some(crate::SavedData::Number(3)));
        let none = vec![SavedValue {
            location: set[0].location.clone(),
            value: None,
        }];
        client.restore_persistent_priority(exe, &none).unwrap();
        assert_eq!(mock.read_persistent_priority(exe).unwrap(), none);

        let t = mock.spawn(10, "a.exe", r"C:\a.exe", 1);
        client.trim_working_set(&t).unwrap();
        assert_eq!(mock.trims(&t).unwrap(), 1);
        client.purge_standby_list().unwrap();
        assert_eq!(mock.standby_purges(), 1);

        // Procesos: sí a uno cualquiera; no a uno protegido ni al cliente.
        let other = mock.spawn(20, "sync.exe", r"C:\sync.exe", 1);
        client.suspend(&other).unwrap();
        assert!(mock.is_suspended(&other).unwrap());
        client.resume(&other).unwrap();
        client.set_priority(&other, Priority::BelowNormal).unwrap();
        assert_eq!(mock.priority(&other).unwrap(), Priority::BelowNormal);
        let protected = mock.spawn(21, "protected.exe", r"C:\protected.exe", 1);
        assert!(client.suspend(&protected).is_err());
        assert!(!mock.is_suspended(&protected).unwrap());
        let me_target = mock.spawn(me, "cli.exe", r"C:\cli.exe", 1);
        assert!(client.kill(&me_target).is_err());
        client.kill(&other).unwrap();

        // Servicios: solo los de la lista blanca.
        mock.add_service("WSearch", crate::ServiceState::Running);
        mock.add_service("RpcSs", crate::ServiceState::Running);
        client.stop_service("WSearch").unwrap();
        assert_eq!(
            mock.service_state("WSearch").unwrap(),
            Some(crate::ServiceState::Stopped)
        );
        client.start_service("WSearch").unwrap();
        assert_eq!(
            mock.service_state("WSearch").unwrap(),
            Some(crate::ServiceState::Running)
        );
        assert!(client.stop_service("RpcSs").is_err());
        assert_eq!(
            mock.service_state("RpcSs").unwrap(),
            Some(crate::ServiceState::Running)
        );

        // Defender: solo lo que deja la lista blanca.
        let game = DefenderExclusion::Path(PathBuf::from(r"C:\Steam\steamapps\common\Game"));
        assert!(!client.has_defender_exclusion(&game).unwrap());
        client.add_defender_exclusion(&game).unwrap();
        assert!(client.has_defender_exclusion(&game).unwrap());
        client.remove_defender_exclusion(&game).unwrap();
        assert!(!mock.has_defender_exclusion(&game).unwrap());
        let windows = DefenderExclusion::Path(PathBuf::from(r"C:\Windows"));
        assert!(client.add_defender_exclusion(&windows).is_err());
        assert!(client.has_defender_exclusion(&windows).is_err());
        assert!(!mock.has_defender_exclusion(&windows).unwrap());

        let user = mock.read_tweak(Tweak::GameDvrOff).unwrap();
        assert!(client.read_tweak(Tweak::GameDvrOff).is_err());
        assert!(client.apply_tweak(Tweak::GameDvrOff).is_err());
        assert_eq!(mock.read_tweak(Tweak::GameDvrOff).unwrap(), user);

        drop(client);
        server.join().unwrap().unwrap();
    }

    /// Un cliente que no es el proceso esperado se queda sin servicio.
    #[test]
    fn refuses_an_unexpected_client() {
        let pipe = pipe_name();
        let sid = current_user_sid().unwrap();
        let server = {
            let pipe = pipe.clone();
            // PID 4 es System: nunca es este proceso.
            std::thread::spawn(move || listen(&pipe, 4, &sid).map(drop))
        };
        let client = helper(connect(&pipe, std::process::id(), None).unwrap());
        assert!(matches!(
            server.join().unwrap(),
            Err(PlatformError::AccessDenied(_))
        ));
        assert!(client.apply_tweak(Tweak::GameDvrPolicyOff).is_err());
    }

    /// El cliente no habla con un pipe que sirve otro proceso.
    #[test]
    fn the_client_checks_the_server() {
        let pipe = pipe_name();
        let me = std::process::id();
        let sid = current_user_sid().unwrap();
        let server = {
            let pipe = pipe.clone();
            std::thread::spawn(move || listen(&pipe, me, &sid).map(drop))
        };
        assert!(matches!(
            connect(&pipe, 4, None),
            Err(PlatformError::AccessDenied(_))
        ));
        let _ = server.join();
    }
}
