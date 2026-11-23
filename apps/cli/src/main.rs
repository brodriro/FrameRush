//! `framerush`: CLI sobre el mismo núcleo que la GUI.
//!
//! `scan` lista procesos y `plan --level` muestra qué se haría (dry-run, PLAN.md It3.3).
//! `apply` aplica el plan (solo con `--yes`), `undo` deshace el último apply y `revert` toda la
//! sesión. Todo cambio pasa por el journal `%LOCALAPPDATA%/framerush/session.json`; la variable
//! `FRAMERUSH_DATA_DIR` cambia esa carpeta (tests). `power` lista los planes de energía y `power
//! set` activa uno como un apply más (journal, `undo`). Desde Agresivo, `apply` duplica el plan
//! del nivel como «FrameRush Max» (`custom-power-plan`; `--exclude` lo quita) y sube cada
//! monitor a su máxima frecuencia (`refresh-rate-max`). `sensors` llega
//! en M2.
//!
//! Lo que pide admin lo hace `framerush-helper` (junto al ejecutable), que se abre con UAC solo
//! cuando hace falta: `apply --admin` con ajustes que lo piden, o `undo`/`revert` de cambios
//! hechos así. El helper termina al terminar la CLI.
//!
//! La salida es siempre UTF-8. PowerShell decodifica la salida capturada de un programa con
//! `[Console]::OutputEncoding` (en muchos equipos la página OEM, p. ej. 850), así que para
//! capturarla sin caracteres rotos hay que poner antes
//! `[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()`.
//!
//! El juego se indica con `--game <exe|pid>`: desde una terminal, la ventana en primer plano es
//! la propia terminal. Sin `--game` se usa el juego de Steam en marcha (`RunningAppID`).
//! Las opciones del adaptador del juego (`--tune`, `--lock`, `--fps-cap`, `--bench`) son opt-in
//! y van al juego de `--appid` (aunque esté cerrado) o al de Steam en marcha.

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use framerush_core::{
    ApplyRequest, CUSTOM_POWER_PLAN_NAME, Change, EntryStatus, FileStore, GameBoost, JournalStore,
    Level, MemoryOp, Outcome, Plan, PlanItem, PlanOptions, RefreshRate, Report, Scope, Snapshot,
    StateLine, Subject, defaults, is_protected, persistent_priority_default, plan, status_lines,
    system_state, tweak_requires_reboot, tweak_scope,
};
use framerush_device::{CoreKind, DeviceProfile};
use framerush_platform::{
    DefenderExclusion, Os, PowerPlan, Priority, ProcessInfo, SavedData, SavedValue, Tweak,
    scan_processes,
};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

/// Sube los FPS limitando procesos de fondo por niveles.
#[derive(Debug, Parser)]
#[command(name = "framerush", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Lista los procesos en ejecución.
    Scan,
    /// Muestra el plan de un nivel sin tocar el sistema (dry-run).
    Plan {
        /// normal, agresivo o extremo.
        #[arg(long, default_value_t = Level::default())]
        level: Level,
        /// Muestra el plan como si hubiera elevación (quita los candados del dry-run).
        #[arg(long)]
        admin: bool,
        #[command(flatten)]
        game: GameArgs,
    },
    /// Aplica el plan de un nivel. Sin `--yes` solo lo muestra.
    Apply(ApplyArgs),
    /// Deshace el último `apply`.
    Undo,
    /// Deshace toda la sesión (Restaurar).
    Revert,
    /// Vista Estado: lo que la sesión tiene aplicado, el valor actual de cada ajuste del
    /// sistema, VBS/HVCI, plan de energía, Hz y config de los juegos. No toca el sistema.
    Status {
        /// Salida en JSON (exportable).
        #[arg(long)]
        json: bool,
    },
    /// Temperaturas y ventiladores (R4). Lo que no se puede leer sale como "No disponible".
    Sensors {
        /// Sigue leyendo cada segundo hasta Ctrl+C.
        #[arg(long)]
        watch: bool,
        /// Abre el helper elevado (UAC) para las fuentes que solo se leen con admin, como el
        /// EC de los portátiles gaming de Acer (ventiladores de CPU y GPU).
        #[arg(long)]
        elevate: bool,
    },
    /// Planes de energía del equipo (R5). Sin subcomando, los lista.
    Power {
        #[command(subcommand)]
        command: Option<PowerCommand>,
    },
    /// Steam: juegos instalados y el que está en marcha (`RunningAppID` o la ventana en primer
    /// plano bajo la carpeta del juego). No toca el sistema.
    Steam,
}

#[derive(Debug, Subcommand)]
enum PowerCommand {
    /// Lista los planes y marca el activo.
    Get,
    /// Activa un plan durante la sesión (pasa por el journal: `undo` o `revert` lo devuelven).
    Set {
        /// Id del plan (GUID en Windows), como lo muestra `framerush power`.
        id: String,
        /// Confirma el cambio.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Args)]
struct ApplyArgs {
    /// normal, agresivo o extremo.
    #[arg(long, default_value_t = Level::default())]
    level: Level,
    /// Quita los candados de lo que pide admin. Con `--yes`, si algo marcado lo pide, abre el
    /// helper elevado (UAC).
    #[arg(long)]
    admin: bool,
    /// Marca además estas reglas (ids separados por comas), como en el checklist.
    #[arg(long, value_delimiter = ',')]
    include: Vec<String>,
    /// Desmarca estas reglas (ids separados por comas).
    #[arg(long, value_delimiter = ',')]
    exclude: Vec<String>,
    #[command(flatten)]
    game: GameArgs,
    /// Confirma `vbs-hvci-off` (Extremo): baja la seguridad de Windows y, hasta restaurarlo,
    /// dejan de funcionar WSL2, Docker, Hyper-V, Windows Sandbox y los emuladores con WHPX.
    #[arg(long)]
    confirm_compat: bool,
    /// Confirma que se apliquen los cambios.
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct GameArgs {
    /// Proceso del juego: nombre del ejecutable (cs2.exe) o PID. Sin él se usa el juego de Steam
    /// en marcha (`RunningAppID`), con todos sus procesos. El juego queda fuera del plan.
    #[arg(long)]
    game: Option<String>,
    /// Prioridad del juego (PLAN.md R3, P1).
    #[arg(long, value_enum, default_value_t = GamePriority::Alta)]
    game_priority: GamePriority,
    /// Segunda confirmación para `--game-priority realtime` (nunca se persiste).
    #[arg(long)]
    confirm_realtime: bool,
    /// Desactiva las optimizaciones de pantalla completa solo para el exe del juego (opt-in,
    /// persistente). Puede empeorar juegos DX12 (PLAN.md V10).
    #[arg(long)]
    fso_off: bool,
    /// `gpu-preference = alto`: el exe del juego usa la GPU de alto rendimiento (persistente).
    /// Solo con más de una GPU detectada (R8).
    #[arg(long)]
    gpu_alto: bool,
    /// `cpu-affinity` (opt-in, de sesión): CPU lógicas para el juego, como `2,3,6-11`, o `auto`
    /// para `cpu-affinity-cores` del equipo. Con anticheat se rechaza (P2).
    #[arg(long, value_name = "CPUS|auto")]
    affinity: Option<String>,
    /// Juego de Steam para las opciones del adaptador (`--tune`, `--lock`, `--fps-cap`,
    /// `--bench`), aunque esté cerrado. Sin él, el juego de Steam en marcha.
    #[arg(long)]
    appid: Option<u32>,
    /// `game-config-tune` (opt-in, persistente): ajustes de vídeo recomendados según el equipo.
    /// Mejor con el juego cerrado: al salir puede reescribir su config.
    #[arg(long)]
    tune: bool,
    /// `game-config-lock`: con `--tune`, la config de vídeo queda de solo lectura.
    #[arg(long)]
    lock: bool,
    /// `fps-cap`: escribe el cfg propio del juego con este tope (0 = sin tope). Se carga con
    /// las opciones de lanzamiento que muestra `plan`.
    #[arg(long, value_name = "FPS")]
    fps_cap: Option<u32>,
    /// `bench-cfg`: cfg de medición del juego.
    #[arg(long)]
    bench: bool,
    /// `resolution-override`: resolución del juego, como `1280x960`; tiene que ser un modo del
    /// monitor principal. Va a la config del juego (no cambia el escritorio).
    #[arg(long, value_name = "ANCHOxALTO")]
    resolution: Option<String>,
    /// `defender-exclusions` (opt-in, persistente, pide `--admin`): excluye de Microsoft
    /// Defender la carpeta del juego de Steam (`--appid` o el abierto) y sus exes en marcha.
    /// Solo con Defender como antivirus activo; nunca se desactiva Defender.
    #[arg(long)]
    defender_exclusions: bool,
}

impl GameArgs {
    /// Alguna opción del adaptador del juego (todas opt-in en la CLI).
    fn wants_adapter(&self) -> bool {
        self.tune || self.lock || self.fps_cap.is_some() || self.bench || self.resolution.is_some()
    }
}

/// Valores de `game-priority` (design/SETTINGS.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum GamePriority {
    Normal,
    Alta,
    Realtime,
}

impl From<GamePriority> for Priority {
    fn from(p: GamePriority) -> Self {
        match p {
            GamePriority::Normal => Priority::Normal,
            GamePriority::Alta => Priority::High,
            GamePriority::Realtime => Priority::Realtime,
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Scan => scan(),
        Command::Plan { level, admin, game } => show_plan(level, admin, &game),
        Command::Apply(args) => apply(&args),
        Command::Undo => undo(),
        Command::Revert => revert(),
        Command::Status { json } => status(json),
        Command::Sensors { watch, elevate } => sensors(watch, elevate),
        Command::Power { command } => match command {
            None | Some(PowerCommand::Get) => power_get(),
            Some(PowerCommand::Set { id, yes }) => power_set(&id, yes),
        },
        Command::Steam => steam(),
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn scan() -> Result<()> {
    let procs = scan_processes().context("no se pudo listar los procesos")?;
    println!("{:>7}  {:>9}  {:<6}  NOMBRE", "PID", "MEMORIA", "PROPIO");
    for p in &procs {
        // P12: lo que no es propio (otro usuario, SYSTEM o elevado) pide admin.
        let own = match p.own {
            Some(true) => "sí",
            Some(false) => "no",
            None => "?",
        };
        println!(
            "{:>7}  {:>6.0} MB  {own:<6}  {}",
            p.target.pid,
            mib(p.memory_bytes),
            p.name
        );
    }
    println!("{} procesos", procs.len());
    Ok(())
}

/// Mismo criterio que la lista de protegidos: sin `.exe` y sin mayúsculas.
fn exe_stem(name: &str) -> String {
    let name = name.to_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_owned()
}

/// Busca el juego por PID o por nombre. Varios procesos con el mismo nombre piden el PID.
fn find_game(processes: &[ProcessInfo], query: &str) -> Result<ProcessInfo> {
    let found: Vec<&ProcessInfo> = match query.parse::<u32>() {
        Ok(pid) => processes.iter().filter(|p| p.target.pid == pid).collect(),
        Err(_) => {
            let want = exe_stem(query);
            processes
                .iter()
                .filter(|p| exe_stem(&p.name) == want)
                .collect()
        }
    };
    let game = match found.as_slice() {
        [] => bail!("no hay ningún proceso `{query}` en ejecución"),
        [one] => *one,
        many => {
            let pids: Vec<String> = many.iter().map(|p| p.target.pid.to_string()).collect();
            bail!(
                "hay {} procesos `{query}` (PID {}): indica el PID con --game",
                many.len(),
                pids.join(", ")
            )
        }
    };
    if is_protected(&game.name) {
        bail!(
            "`{}` está en la lista de protegidos: no puede ser el juego",
            game.name
        );
    }
    Ok(game.clone())
}

/// El juego: lo que indica `--game` o, sin él, el juego de Steam en marcha.
#[derive(Debug)]
struct Game {
    /// Cómo se muestra: `cs2.exe (pid 123)` o `Counter-Strike 2 (730, Steam) · pid 123, 124`.
    label: String,
    processes: Vec<ProcessInfo>,
    /// `appid` si el juego salió de Steam.
    appid: Option<u32>,
}

fn resolve_game(processes: &[ProcessInfo], query: Option<&str>) -> Result<Option<Game>> {
    let Some(query) = query else {
        return Ok(steam_game(processes));
    };
    let p = find_game(processes, query)?;
    Ok(Some(Game {
        label: format!("{} (pid {})", p.name, p.target.pid),
        processes: vec![p],
        appid: None,
    }))
}

/// El juego que Steam tiene en marcha (`RunningAppID`) con todos sus procesos, salvo los
/// protegidos. Si Steam no se puede leer, se avisa y se sigue sin juego.
fn steam_game(processes: &[ProcessInfo]) -> Option<Game> {
    let read = || -> framerush_steam::Result<Option<(framerush_steam::App, Vec<ProcessInfo>)>> {
        let Some(root) = framerush_steam::locate()? else {
            return Ok(None);
        };
        let found = framerush_steam::installed_apps(&root)?;
        let running = framerush_steam::running_app_id()?;
        // Desde una terminal el primer plano es la terminal: solo cuenta `RunningAppID`.
        let Some((app, _)) = framerush_steam::detect(&found.apps, running, None) else {
            return Ok(None);
        };
        let procs = framerush_steam::game_processes(app, processes)
            .into_iter()
            .filter(|p| !is_protected(&p.name))
            .cloned()
            .collect();
        Ok(Some((app.clone(), procs)))
    };
    match read() {
        Ok(Some((app, procs))) if procs.is_empty() => {
            println!(
                "Aviso: Steam tiene «{}» en marcha pero no hay procesos bajo {}.",
                app.name,
                app.dir.display()
            );
            None
        }
        Ok(Some((app, procs))) => {
            let pids: Vec<String> = procs.iter().map(|p| p.target.pid.to_string()).collect();
            Some(Game {
                label: format!(
                    "{} ({}, detectado en Steam) · pid {}",
                    app.name,
                    app.appid,
                    pids.join(", ")
                ),
                processes: procs,
                appid: Some(app.appid),
            })
        }
        Ok(None) => None,
        Err(e) => {
            println!("Aviso: no se pudo leer Steam ({e}); sigue sin juego.");
            None
        }
    }
}

/// Las opciones del juego necesitan un juego (`--game` o uno de Steam en marcha).
fn require_game(args: &GameArgs, game: Option<&Game>) -> Result<()> {
    let wants = args.fso_off || args.gpu_alto || args.affinity.is_some() || args.confirm_realtime;
    if wants && game.is_none() {
        bail!("esa opción necesita un juego: indica --game o abre el juego desde Steam");
    }
    Ok(())
}

fn build_plan(
    level: Level,
    admin: bool,
    game: Option<&str>,
) -> Result<(Plan, Option<Game>, DeviceProfile)> {
    let Some(os) = Os::current() else {
        bail!("sistema operativo no admitido");
    };
    let device = native_device(os);
    let rules = framerush_rules::embedded().context("las reglas embebidas no son válidas")?;
    let processes = scan_processes().context("no se pudo listar los procesos")?;
    let game = resolve_game(&processes, game)?;
    let (tweaks, admin_tweaks) = native_tweaks();
    let snapshot = Snapshot {
        processes,
        services: native_services(&rules, os),
        game: game
            .iter()
            .flat_map(|g| g.processes.iter().map(|p| p.target.clone()))
            .collect(),
        tweaks,
        admin_tweaks,
        memory: native_memory(),
    };
    let options = PlanOptions {
        elevated: admin,
        keep_alive: defaults::keep_alive(&device).value,
    };
    let plan = plan(level, &snapshot, &rules, &device, &options);
    Ok((plan, game, device))
}

/// Lo que escribe el adaptador del juego (persistente) y cómo mostrarlo.
struct AdapterPlan {
    name: &'static str,
    launch_options: &'static str,
    bench_command: &'static str,
    plan: framerush_games::GamePlan,
}

/// Opciones del adaptador: `None` si no se pidió ninguna. El juego es `--appid` o el de Steam
/// en marcha; sin adaptador o sin instalar en Steam es un error, no se ignora.
fn adapter_plan(
    args: &GameArgs,
    game: Option<&Game>,
    device: &DeviceProfile,
) -> Result<Option<AdapterPlan>> {
    if !args.wants_adapter() {
        if args.appid.is_some() && !args.defender_exclusions {
            bail!(
                "--appid solo sirve con --tune, --lock, --fps-cap, --bench, --resolution o                  --defender-exclusions"
            );
        }
        return Ok(None);
    }
    if args.lock && !args.tune && args.resolution.is_none() {
        bail!("--lock bloquea la config de vídeo: úsalo junto a --tune o --resolution");
    }
    let resolution = args
        .resolution
        .as_deref()
        .map(|spec| game_resolution(spec, device))
        .transpose()?;
    let Some(appid) = args.appid.or_else(|| game.and_then(|g| g.appid)) else {
        bail!(
            "las opciones del adaptador necesitan un juego de Steam: indica --appid \
             (`framerush steam` los lista) o abre el juego desde Steam"
        );
    };
    let Some(adapter) = framerush_games::adapter(appid) else {
        bail!(
            "el juego {appid} no tiene adaptador: --tune, --lock, --fps-cap, --bench y --resolution no aplican"
        );
    };
    let Some(root) = framerush_steam::locate().context("no se pudo buscar Steam")? else {
        bail!("Steam no está instalado");
    };
    let found = framerush_steam::installed_apps(&root).context("no se pudo leer Steam")?;
    let Some(app) = found.apps.iter().find(|a| a.appid == appid) else {
        bail!("{} ({appid}) no está instalado en Steam", adapter.name());
    };
    let install = framerush_games::Install {
        dir: &app.dir,
        steam_root: &root,
    };
    let options = framerush_games::GameOptions {
        tune: args.tune,
        lock: args.lock,
        fps_cap: args.fps_cap,
        bench: args.bench,
        resolution,
    };
    let mut plan = adapter
        .plan(install, device, options)
        .with_context(|| format!("no se pudo leer la config de {}", adapter.name()))?;
    // El juego reescribe su config al salir (referencia CS2-FPS-Boost): sin bloqueo se pierde.
    let running = game.is_some_and(|g| g.appid == Some(appid) && !g.processes.is_empty());
    if running && args.tune && !args.lock {
        plan.notes.push(format!(
            "{} está abierto: al cerrarse puede reescribir su config de vídeo (ciérralo antes o usa --lock)",
            adapter.name()
        ));
    }
    Ok(Some(AdapterPlan {
        name: adapter.name(),
        launch_options: adapter.launch_options(),
        bench_command: adapter.bench_command(),
        plan,
    }))
}

/// `--resolution 1280x960`: tiene que ser un modo del monitor principal (R8).
fn game_resolution(spec: &str, device: &DeviceProfile) -> Result<(u32, u32)> {
    let parsed = spec
        .split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)));
    let Some((width, height)) = parsed else {
        bail!("--resolution: usa ANCHOxALTO, por ejemplo 1280x960");
    };
    let Some(display) = device
        .displays
        .iter()
        .find(|d| d.primary)
        .or(device.displays.first())
    else {
        bail!("no se detectaron los modos del monitor: --resolution no se puede comprobar");
    };
    if !display
        .modes
        .iter()
        .any(|m| m.width == width && m.height == height)
    {
        bail!(
            "el monitor principal ({}) no admite {width}x{height}",
            display.id
        );
    }
    Ok((width, height))
}

/// `--defender-exclusions`: la carpeta del juego de Steam y los exes en marcha que hay dentro.
fn defender_plan(
    args: &GameArgs,
    game: Option<&Game>,
    device: &DeviceProfile,
) -> Result<Vec<DefenderExclusion>> {
    if !args.defender_exclusions {
        return Ok(vec![]);
    }
    if device.defender_active != Some(true) {
        bail!("--defender-exclusions: Microsoft Defender no es el antivirus activo");
    }
    let Some(appid) = args.appid.or_else(|| game.and_then(|g| g.appid)) else {
        bail!(
            "--defender-exclusions necesita un juego de Steam: indica --appid              (`framerush steam` los lista) o abre el juego desde Steam"
        );
    };
    let Some(root) = framerush_steam::locate().context("no se pudo buscar Steam")? else {
        bail!("Steam no está instalado");
    };
    let found = framerush_steam::installed_apps(&root).context("no se pudo leer Steam")?;
    let Some(app) = found.apps.into_iter().find(|a| a.appid == appid) else {
        bail!("el juego {appid} no está instalado en Steam");
    };
    // Windows no distingue mayúsculas en las rutas; Steam y el SO pueden escribirlas distinto.
    let dir = format!("{}\\", app.dir.to_string_lossy().to_lowercase());
    let mut exes: Vec<PathBuf> = game
        .filter(|g| g.appid == Some(appid))
        .map(|g| {
            g.processes
                .iter()
                .map(|p| p.target.exe_path.clone())
                .collect()
        })
        .unwrap_or_default();
    exes.retain(|e| e.to_string_lossy().to_lowercase().starts_with(&dir));
    exes.sort();
    exes.dedup();
    Ok(std::iter::once(DefenderExclusion::Path(app.dir))
        .chain(exes.into_iter().map(DefenderExclusion::Process))
        .collect())
}

fn print_defender(exclusions: &[DefenderExclusion], admin: bool) {
    if exclusions.is_empty() {
        return;
    }
    let lock = if admin { "" } else { " · requiere --admin" };
    println!("Exclusiones de Defender (persistentes, las quita `revert`{lock}):");
    for e in exclusions {
        match e {
            DefenderExclusion::Path(p) => println!("  carpeta  {}", p.display()),
            DefenderExclusion::Process(p) => println!("  proceso  {}", p.display()),
        }
    }
    if !exclusions
        .iter()
        .any(|e| matches!(e, DefenderExclusion::Process(_)))
    {
        println!("  (el juego no está en marcha: solo se excluye su carpeta)");
    }
    println!();
}

fn print_adapter(adapter: Option<&AdapterPlan>, args: &GameArgs) {
    let Some(a) = adapter else {
        return;
    };
    println!("Config de {} (persistente, la deshace `revert`):", a.name);
    for w in &a.plan.writes {
        let lock = if w.read_only { " · solo lectura" } else { "" };
        println!("[x] {}{lock}", w.path.display());
    }
    if let Some(r) = &args.resolution {
        println!("    Resolución del juego: {r} (en su config; el escritorio no cambia)");
    }
    if args.fps_cap.is_some() {
        println!("    Opciones de lanzamiento en Steam: {}", a.launch_options);
    }
    if args.bench {
        println!(
            "    Para medir, en la consola del juego: {}",
            a.bench_command
        );
    }
    for n in &a.plan.notes {
        println!("Aviso: {n}");
    }
    println!();
}

/// `--gpu-alto` solo se ofrece si el equipo tiene más de una GPU (PLAN.md It6: con una sola no
/// hay nada que elegir).
fn check_gpu_preference(args: &GameArgs, device: &DeviceProfile) -> Result<()> {
    if args.gpu_alto && defaults::gpu_preference(device).is_none() {
        bail!(
            "`gpu-preference` solo se ofrece con más de una GPU detectada (este equipo: {})",
            device.gpus.len()
        );
    }
    Ok(())
}

/// CPU de `--affinity`, o `None` si no se pidió. Falla con anticheat (P2) o si `auto` no tiene
/// topología con la que elegir (R8: no se inventa una).
fn game_affinity(
    args: &GameArgs,
    plan: &Plan,
    device: &DeviceProfile,
) -> Result<Option<Vec<usize>>> {
    let Some(spec) = args.affinity.as_deref() else {
        return Ok(None);
    };
    if !plan.game_tuning_allowed() {
        bail!(
            "anticheat en ejecución ({}): la afinidad del juego queda desactivada (P2)",
            plan.anticheats.join(", ")
        );
    }
    let cpus = if spec == "auto" {
        let cores = defaults::affinity_cpus(device).value;
        if cores.is_empty() {
            bail!(
                "no se pudo detectar la topología de la CPU: indica las CPU, \
                 por ejemplo --affinity 2-7"
            );
        }
        cores
    } else {
        parse_cpus(spec)?
    };
    if defaults::affinity_warning(device) == Some(true) {
        println!("Aviso: con 4 núcleos físicos o menos la afinidad manual suele empeorar (V18).");
    }
    Ok(Some(cpus))
}

/// `2,3,6-11` → `[2, 3, 6, 7, 8, 9, 10, 11]`, ordenado y sin repetir.
fn parse_cpus(spec: &str) -> Result<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in spec.split(',').map(str::trim) {
        let number = |s: &str| {
            s.trim()
                .parse::<usize>()
                .with_context(|| format!("`{s}` no es un número de CPU"))
        };
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (number(a)?, number(b)?);
                if a > b {
                    bail!("rango de CPU al revés: `{part}`");
                }
                cpus.extend(a..=b);
            }
            None => cpus.push(number(part)?),
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

/// Id de design/SETTINGS.md de la prioridad persistente del juego.
const PERSISTENT_PRIORITY: &str = "game-priority-persistent";
/// Id de design/SETTINGS.md del plan de energía del nivel.
const POWER_PLAN: &str = "power-plan";

/// Id de design/SETTINGS.md del plan propio "FrameRush Max".
const CUSTOM_POWER_PLAN: &str = "custom-power-plan";

/// Id de design/SETTINGS.md de los C-states desactivados (dentro del plan propio).
const CSTATES_OFF: &str = "cstates-off";

/// Id de design/SETTINGS.md del refresco al máximo.
const REFRESH_RATE_MAX: &str = "refresh-rate-max";

/// Frecuencias del nivel, salvo con `--exclude refresh-rate-max`. Vacío si no se ofrece.
fn level_refresh(level: Level, device: &DeviceProfile, exclude: &[String]) -> Vec<RefreshRate> {
    if exclude.iter().any(|id| id == REFRESH_RATE_MAX) {
        return vec![];
    }
    defaults::refresh_rate_max(level, device)
        .into_iter()
        .map(|(d, hz)| RefreshRate {
            display: d.id.clone(),
            hz,
        })
        .collect()
}

/// Se oculta si todos los monitores ya están al máximo (It6).
fn print_refresh(level: Level, device: &DeviceProfile, rates: &[RefreshRate]) {
    for (d, hz) in defaults::refresh_rate_max(level, device) {
        let mark = if rates.iter().any(|r| r.display == d.id) {
            "x"
        } else {
            " "
        };
        println!(
            "[{mark}] {REFRESH_RATE_MAX:<32} sesión · {} {} → {hz} Hz",
            d.id, d.current.hz
        );
    }
}

/// Energía del nivel (P3): el plan del nivel y, si `custom-power-plan` está marcado, la base
/// que se duplica (entonces sustituye al plan del nivel).
#[derive(Debug, Default)]
struct LevelPower {
    plan: Option<PowerPlan>,
    /// `Some` si el plan propio se ofrece en este equipo y nivel.
    custom_base: Option<PowerPlan>,
    custom_checked: bool,
    /// `cstates-off`: `Some` si se ofrece (Extremo con el plan propio marcado) y si está marcado.
    cstates_off: Option<bool>,
}

/// `--exclude power-plan` quita los dos (no hay base); `--exclude custom-power-plan` deja solo
/// el plan del nivel.
fn level_power(level: Level, device: &DeviceProfile, exclude: &[String]) -> LevelPower {
    let excluded = |id: &str| exclude.iter().any(|e| e == id);
    if excluded(POWER_PLAN) {
        return LevelPower::default();
    }
    let custom_base = defaults::custom_power_plan_base(level, device).cloned();
    let custom_checked = custom_base.is_some() && !excluded(CUSTOM_POWER_PLAN);
    LevelPower {
        plan: defaults::power_plan_for(level, device).cloned(),
        cstates_off: (custom_checked && level == Level::Extremo).then(|| !excluded(CSTATES_OFF)),
        custom_checked,
        custom_base,
    }
}

fn print_power(power: &LevelPower, device: &DeviceProfile) {
    match (&power.plan, &power.custom_base) {
        (_, Some(base)) if power.custom_checked => {
            println!("[x] {POWER_PLAN:<32} base · «{}»", base.name)
        }
        (Some(p), _) => println!("[x] {POWER_PLAN:<32} sesión · «{}»", p.name),
        (None, _) => {
            let active = device
                .active_power_plan
                .as_deref()
                .and_then(|id| device.power_plans.iter().find(|p| p.id == id));
            match active {
                Some(a) => println!("[ ] {POWER_PLAN:<32} sin cambio · «{}»", a.name),
                None => println!("[ ] {POWER_PLAN:<32} no disponible"),
            }
        }
    }
    if power.custom_base.is_some() {
        let mark = if power.custom_checked { "x" } else { " " };
        println!(
            "[{mark}] {CUSTOM_POWER_PLAN:<32} sesión · «{CUSTOM_POWER_PLAN_NAME}» (solo CA; se borra al restaurar)"
        );
        if let Some(checked) = power.cstates_off {
            let mark = if checked { "x" } else { " " };
            println!(
                "[{mark}] {CSTATES_OFF:<32} sesión · dentro de «{CUSTOM_POWER_PLAN_NAME}» (más calor; vigila la temperatura)"
            );
        }
    }
    println!();
}

/// Cómo sale una opción en el checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    On,
    Off,
    /// Pide admin y no se pasó `--admin` (P14).
    Locked,
}

/// `game-priority-persistent` para este juego, o `None` si no aplica (sin juego, por el nivel o
/// por la prioridad, P1). `--exclude game-priority-persistent` lo desmarca.
fn persistent_mark(
    level: Level,
    admin: bool,
    args: &GameArgs,
    has_game: bool,
    exclude: &[String],
) -> Option<Mark> {
    if !has_game || !persistent_priority_default(level, args.game_priority.into()) {
        return None;
    }
    Some(if !admin {
        Mark::Locked
    } else if exclude.iter().any(|id| id == PERSISTENT_PRIORITY) {
        Mark::Off
    } else {
        Mark::On
    })
}

/// Perfil de hardware detectado (R8). Lo que falta sale como "no detectado".
fn print_device(device: &DeviceProfile) {
    let mut system = vec![match device.windows_11 {
        Some(true) => "Windows 11".to_owned(),
        Some(false) => "Windows anterior a 11".to_owned(),
        None => format!("{:?}", device.os),
    }];
    if let Some(build) = &device.os_build {
        system.push(format!("build {build}"));
    }
    if let Some(maker) = &device.manufacturer {
        system.push(maker.clone());
    }
    match device.laptop {
        Some(true) => system.push("portátil".into()),
        Some(false) => system.push("no es portátil".into()),
        None => {}
    }
    if let Some(bytes) = device.memory_bytes {
        system.push(format!(
            "{:.1} GB de RAM",
            bytes as f64 / f64::from(1u32 << 30)
        ));
    }
    match device.vbs_hvci_enabled {
        Some(true) => system.push("VBS en ejecución".into()),
        Some(false) => system.push("VBS desactivado".into()),
        None => {}
    }
    if !device.installed_apps.is_empty() {
        system.push(format!("{} apps instaladas", device.installed_apps.len()));
    }
    println!("Sistema: {}", system.join(" · "));
    match &device.cpu {
        Some(cpu) => {
            let cores = cpu.physical_cores().map_or("¿?".into(), |n| n.to_string());
            let e = cpu
                .cores
                .iter()
                .filter(|c| c.kind == CoreKind::Efficiency)
                .count();
            let hybrid = if e > 0 {
                format!(", {e} E-cores")
            } else {
                String::new()
            };
            println!(
                "CPU: {} · {cores} núcleos físicos, {} lógicos{hybrid}",
                cpu.model, cpu.logical_cpus
            );
        }
        None => println!("CPU: no detectada (los valores por defecto son estimados)"),
    }
    for g in &device.gpus {
        let vram = g.dedicated_vram_bytes.map_or(String::new(), |b| {
            format!(" · {} MB dedicados", b / (1024 * 1024))
        });
        let slowdown = g
            .slowdown_c
            .map_or(String::new(), |c| format!(" · slowdown a {c:.0} °C"));
        println!("GPU: {}{vram}{slowdown}", g.name);
    }
    if device.gpus.is_empty() {
        println!("GPU: no detectada");
    }
    match device.hags_supported {
        Some(true) => println!("HAGS: admitido"),
        Some(false) => println!("HAGS: no admitido"),
        None => {}
    }
    if !device.antivirus.is_empty() {
        println!("Antivirus registrado: {}", device.antivirus.join(", "));
    }
    match device.defender_active {
        Some(true) => println!("Microsoft Defender: activo"),
        Some(false) => println!("Microsoft Defender: no es el antivirus activo"),
        None => {}
    }
    if !device.virtualization.is_empty() {
        println!(
            "Usa el hipervisor: {} (se rompe si se desactiva)",
            device.virtualization.join(", ")
        );
    }
    match device.battery {
        Some(b) if b.discharging => println!("Batería: descargándose (desenchufado)"),
        Some(_) => println!("Batería: enchufado"),
        None => println!("Batería: no hay o no se pudo leer"),
    }
    if defaults::battery_warning(device) {
        println!(
            "AVISO (battery-warning): el equipo está con batería; enchúfalo para jugar. \
             Aplicar sigue disponible."
        );
    }
    println!();
}

fn print_game(
    game: Option<&Game>,
    args: &GameArgs,
    persistent: Option<Mark>,
    affinity: Option<&[usize]>,
) {
    match game {
        Some(g) => {
            println!("Juego: {} → prioridad {:?}", g.label, args.game_priority);
            let mut exes: Vec<String> = g
                .processes
                .iter()
                .map(|p| p.target.exe_path.display().to_string())
                .collect();
            exes.sort();
            exes.dedup();
            let exes = exes.join(", ");
            if args.fso_off {
                println!("[x] fullscreen-optimizations-off   persistente · {exes}");
            }
            if args.gpu_alto {
                println!("[x] gpu-preference = alto             persistente · {exes}");
            }
            if let Some(cpus) = affinity {
                let list: Vec<String> = cpus.iter().map(ToString::to_string).collect();
                println!(
                    "[x] cpu-affinity                     sesión · CPU {}",
                    list.join(",")
                );
            }
            match persistent {
                Some(Mark::On) => println!("[x] {PERSISTENT_PRIORITY:<32} persistente"),
                Some(Mark::Off) => println!("[ ] {PERSISTENT_PRIORITY:<32} persistente"),
                Some(Mark::Locked) => {
                    println!("[ ] {PERSISTENT_PRIORITY:<32} persistente · requiere admin")
                }
                None => {}
            }
            println!();
        }
        None => {
            println!("Juego: ni --game ni juego de Steam en marcha; su prioridad no se toca.\n")
        }
    }
}

fn show_plan(level: Level, admin: bool, args: &GameArgs) -> Result<()> {
    notify_open_session();
    let (plan, game, device) = build_plan(level, admin, args.game.as_deref())?;
    require_game(args, game.as_ref())?;
    let affinity = game_affinity(args, &plan, &device)?;
    check_gpu_preference(args, &device)?;
    let adapter = adapter_plan(args, game.as_ref(), &device)?;
    let defender = defender_plan(args, game.as_ref(), &device)?;
    println!("Plan para el nivel {level} (dry-run: no se toca nada)");
    print_device(&device);
    print_game(
        game.as_ref(),
        args,
        persistent_mark(level, admin, args, game.is_some(), &[]),
        affinity.as_deref(),
    );
    print_adapter(adapter.as_ref(), args);
    print_defender(&defender, admin);
    print_refresh(level, &device, &level_refresh(level, &device, &[]));
    print_power(&level_power(level, &device, &[]), &device);
    let checked: Vec<&PlanItem> = plan.checked().collect();
    print_plan(&plan, &checked, &plan.tweaks, &plan.memory, &device);
    Ok(())
}

/// Lo marcado en el plan: filas, ajustes y operaciones de memoria.
type Selection<'a> = (Vec<&'a PlanItem>, Vec<Tweak>, Vec<MemoryOp>);

/// Filas, ajustes y memoria marcados por el nivel, más `include` y menos `exclude`. Lo que
/// tiene candado no se puede marcar.
fn select<'a>(plan: &'a Plan, include: &[String], exclude: &[String]) -> Result<Selection<'a>> {
    for id in include.iter().chain(exclude) {
        let known = plan.items.iter().any(|i| &i.rule_id == id)
            || plan
                .tweaks
                .iter()
                .chain(&plan.optional_tweaks)
                .chain(&plan.locked_tweaks)
                .chain(plan.blocked_tweaks.iter().map(|(t, _)| t))
                .any(|t| t.id() == id)
            || plan
                .memory
                .iter()
                .chain(&plan.locked_memory)
                .any(|m| m.id() == id);
        if !known {
            bail!("`{id}` no está en el plan (¿no está en ejecución o no existe en este SO?)");
        }
    }
    if let Some(i) = plan
        .items
        .iter()
        .find(|i| i.locked && include.contains(&i.rule_id))
    {
        bail!("la regla `{}` requiere admin: usa --admin", i.rule_id);
    }
    if let Some(t) = plan
        .locked_tweaks
        .iter()
        .find(|t| include.iter().any(|id| id == t.id()))
    {
        bail!("el ajuste `{}` requiere admin: usa --admin", t.id());
    }
    if let Some((t, why)) = plan
        .blocked_tweaks
        .iter()
        .find(|(t, _)| include.iter().any(|id| id == t.id()))
    {
        bail!("el ajuste `{}` está bloqueado: {why}", t.id());
    }
    if let Some(m) = plan
        .locked_memory
        .iter()
        .find(|m| include.iter().any(|id| id == m.id()))
    {
        bail!("`{}` requiere admin: usa --admin", m.id());
    }
    let items = plan
        .items
        .iter()
        .filter(|i| !i.locked && (i.default_checked || include.contains(&i.rule_id)))
        .filter(|i| !exclude.contains(&i.rule_id))
        .collect();
    let wanted = |t: &Tweak| include.iter().any(|id| id == t.id());
    let tweaks = plan
        .tweaks
        .iter()
        .chain(plan.optional_tweaks.iter().filter(|t| wanted(t)))
        .copied()
        .filter(|t| !exclude.iter().any(|id| id == t.id()))
        .collect();
    let memory = plan
        .memory
        .iter()
        .copied()
        .filter(|m| !exclude.iter().any(|id| id == m.id()))
        .collect();
    Ok((items, tweaks, memory))
}

fn apply(args: &ApplyArgs) -> Result<()> {
    notify_open_session();
    let level = args.level;
    let (plan, game, device) = build_plan(level, args.admin, args.game.game.as_deref())?;
    require_game(&args.game, game.as_ref())?;
    let affinity = game_affinity(&args.game, &plan, &device)?;
    check_gpu_preference(&args.game, &device)?;
    let adapter = adapter_plan(&args.game, game.as_ref(), &device)?;
    let defender = defender_plan(&args.game, game.as_ref(), &device)?;
    if !defender.is_empty() && !args.admin && args.yes {
        bail!("--defender-exclusions requiere admin: usa --admin");
    }
    let persistent = persistent_mark(level, args.admin, &args.game, game.is_some(), &args.exclude);
    if args.include.iter().any(|id| id == PERSISTENT_PRIORITY) {
        match persistent {
            None => bail!(
                "`{PERSISTENT_PRIORITY}` necesita un juego (--game o Steam) y el nivel agresivo o extremo"
            ),
            Some(Mark::Locked) => bail!("`{PERSISTENT_PRIORITY}` requiere admin: usa --admin"),
            Some(_) => {}
        }
    }
    if args.include.iter().any(|id| id == POWER_PLAN) {
        bail!("`{POWER_PLAN}` lo marca el nivel; para elegir otro plan usa `framerush power set`");
    }
    if args.include.iter().any(|id| id == CUSTOM_POWER_PLAN) {
        bail!("`{CUSTOM_POWER_PLAN}` lo marca el nivel (agresivo o extremo, solo Windows)");
    }
    if args.include.iter().any(|id| id == CSTATES_OFF) {
        bail!("`{CSTATES_OFF}` lo marca el nivel extremo con `{CUSTOM_POWER_PLAN}` (solo Windows)");
    }
    if args.include.iter().any(|id| id == REFRESH_RATE_MAX) {
        bail!("`{REFRESH_RATE_MAX}` lo marca el nivel (agresivo o extremo, solo Windows)");
    }
    let power = level_power(level, &device, &args.exclude);
    let refresh = level_refresh(level, &device, &args.exclude);
    let others = |ids: &[String]| -> Vec<String> {
        ids.iter()
            .filter(|id| {
                ![
                    PERSISTENT_PRIORITY,
                    POWER_PLAN,
                    CUSTOM_POWER_PLAN,
                    CSTATES_OFF,
                    REFRESH_RATE_MAX,
                ]
                .contains(&id.as_str())
            })
            .cloned()
            .collect()
    };
    let (checked, tweaks, memory) = select(&plan, &others(&args.include), &others(&args.exclude))?;
    if args.game.game_priority == GamePriority::Realtime && !args.game.confirm_realtime {
        bail!("Tiempo real puede congelar el equipo: confírmalo con --confirm-realtime");
    }
    if args.yes && tweaks.contains(&Tweak::VbsHvciOff) && !args.confirm_compat {
        bail!(
            "`vbs-hvci-off` baja la seguridad de Windows y, hasta restaurarlo, deja sin: {}.              Confírmalo con --confirm-compat o quítalo con --exclude vbs-hvci-off",
            compat_list(&device)
        );
    }
    if !args.yes {
        println!("Plan para el nivel {level} (no se aplica sin --yes)");
        print_device(&device);
        print_game(game.as_ref(), &args.game, persistent, affinity.as_deref());
        print_adapter(adapter.as_ref(), &args.game);
        print_defender(&defender, args.admin);
        print_refresh(level, &device, &refresh);
        print_power(&power, &device);
        print_plan(&plan, &checked, &tweaks, &memory, &device);
        println!("\nRevisa las filas y repite con --yes para aplicarlas.");
        return Ok(());
    }
    if checked.is_empty()
        && tweaks.is_empty()
        && memory.is_empty()
        && game.is_none()
        && power.plan.is_none()
        && !power.custom_checked
        && refresh.is_empty()
        && adapter.as_ref().is_none_or(|a| a.plan.writes.is_empty())
        && defender.is_empty()
    {
        println!("No hay filas ni ajustes marcados ni juego: no se toca nada.");
        return Ok(());
    }
    let request = ApplyRequest {
        memory,
        custom_power_plan: power.custom_base.filter(|_| power.custom_checked),
        cstates_off: power.cstates_off == Some(true),
        refresh_rates: refresh,
        power_plan: power.plan,
        items: checked,
        tweaks,
        game: game.map(|g| GameBoost {
            targets: g.processes.into_iter().map(|p| p.target).collect(),
            priority: args.game.game_priority.into(),
            realtime_confirmed: args.game.confirm_realtime,
            fullscreen_optimizations_off: args.game.fso_off,
            affinity,
            high_performance_gpu: args.game.gpu_alto,
            persistent_priority: persistent == Some(Mark::On),
        }),
        game_files: adapter.map(|a| a.plan.writes).unwrap_or_default(),
        defender_exclusions: defender,
    };
    let report = run(Op::Apply(Box::new(request)))?;
    print_report(&report);
    // Lo que no deja estado (memoria) no abre sesión: entonces no hay nada que deshacer.
    let journal = FileStore::new(data_dir()?.join("session.json")).load()?;
    if journal.is_open() {
        println!(
            "Deshacer: `framerush undo` (último apply) o `framerush revert` (toda la sesión)."
        );
    }
    Ok(())
}

fn power_get() -> Result<()> {
    let (plans, active) = native_power()?;
    for p in &plans {
        let mark = if p.id == active.id { "*" } else { " " };
        println!("{mark} {}  {}", p.id, p.name);
    }
    println!("(* = activo)");
    Ok(())
}

fn steam() -> Result<()> {
    let Some(root) = framerush_steam::locate().context("no se pudo buscar Steam")? else {
        println!("Steam no está instalado (en Linux y macOS aún no se busca).");
        return Ok(());
    };
    println!("Steam: {}", root.display());
    let found = framerush_steam::installed_apps(&root).context("no se pudo leer Steam")?;
    for app in &found.apps {
        println!("{:>8}  {}  {}", app.appid, app.name, app.dir.display());
    }
    for e in &found.errors {
        println!("aviso: {e}");
    }
    let running = framerush_steam::running_app_id().context("no se pudo leer RunningAppID")?;
    let foreground = foreground_exe()?;
    let Some((app, source)) = framerush_steam::detect(&found.apps, running, foreground.as_deref())
    else {
        println!("Juego en marcha: ninguno");
        return Ok(());
    };
    let source = match source {
        framerush_steam::Source::RunningAppId => "RunningAppID",
        framerush_steam::Source::Foreground => "primer plano",
    };
    println!(
        "Juego en marcha: {} ({}), según {source}",
        app.name, app.appid
    );
    let processes = scan_processes().context("no se pudo listar los procesos")?;
    for p in framerush_steam::game_processes(app, &processes) {
        println!("  {:>6}  {}", p.target.pid, p.target.exe_path.display());
    }
    Ok(())
}

/// Exe de la ventana en primer plano (desde una terminal, la propia terminal).
#[cfg(windows)]
fn foreground_exe() -> Result<Option<PathBuf>> {
    use framerush_platform::ProcessControl;
    let w = framerush_platform::WindowsPlatform::new();
    Ok(w.foreground()
        .context("no se pudo leer la ventana en primer plano")?
        .map(|t| t.exe_path))
}

#[cfg(not(windows))]
fn foreground_exe() -> Result<Option<PathBuf>> {
    Ok(None)
}

fn power_set(id: &str, yes: bool) -> Result<()> {
    let (plans, active) = native_power()?;
    let Some(plan) = plans.into_iter().find(|p| p.id.eq_ignore_ascii_case(id)) else {
        bail!("no hay ningún plan `{id}` en este equipo (`framerush power` los lista)");
    };
    if plan.id == active.id {
        println!("«{}» ya es el plan activo.", plan.name);
        return Ok(());
    }
    if !yes {
        println!(
            "Se activaría «{}» en lugar de «{}» (sesión). Repite con --yes.",
            plan.name, active.name
        );
        return Ok(());
    }
    let request = ApplyRequest {
        power_plan: Some(plan),
        ..ApplyRequest::default()
    };
    print_report(&run(Op::Apply(Box::new(request)))?);
    println!("Deshacer: `framerush undo` (último apply) o `framerush revert` (toda la sesión).");
    Ok(())
}

/// Fuentes de sensores del equipo (`framerush_sensors::detect`; el tacómetro de la GPU por
/// NVAPI es el último recurso). Se dice cuáles se consultaron.
fn sensor_providers(elevate: bool) -> Result<Vec<Box<dyn framerush_sensors::SensorProvider>>> {
    match framerush_sensors::NvidiaGpus::open() {
        Ok(gpus) => {
            for g in gpus.gpus() {
                match g.slowdown_c {
                    Some(c) => println!("{}: slowdown a {c:.0} °C", g.name),
                    None => println!("{}: umbral de slowdown no disponible", g.name),
                }
            }
        }
        Err(e) => println!("{e}"),
    }
    let mut sources = framerush_sensors::detect();
    if !sources.needs_admin.is_empty() {
        let names = sources.needs_admin.join(", ");
        if elevate {
            sources = elevated_sources(&names)?;
        } else {
            println!("{names} solo se lee con admin: repite con `--elevate` (UAC).");
        }
    }
    println!("Fuentes consultadas: {}", sources.checked.join(", "));
    Ok(sources.providers)
}

#[cfg(windows)]
fn elevated_sources(names: &str) -> Result<framerush_sensors::Sources> {
    println!("Se abre el helper elevado (UAC) para leer {names}.");
    let helper =
        framerush_platform::Helper::launch(&helper_exe()?).context("no se pudo abrir el helper")?;
    Ok(framerush_sensors::detect_elevated(helper))
}

/// Fuera de Windows ninguna fuente pide admin todavía.
#[cfg(not(windows))]
fn elevated_sources(names: &str) -> Result<framerush_sensors::Sources> {
    bail!("{names} pide admin y en este sistema no hay helper")
}

fn print_sensors(sensors: &[framerush_sensors::Sensor]) {
    if sensors.is_empty() {
        println!("No se encontró ningún sensor de CPU, GPU ni ventiladores.");
    }
    for s in sensors {
        println!("{s}");
    }
}

fn sensors(watch: bool, elevate: bool) -> Result<()> {
    let mut providers = sensor_providers(elevate)?;
    if !watch {
        let sensors: Vec<_> = providers.iter_mut().flat_map(|p| p.read()).collect();
        print_sensors(&sensors);
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let _poller = framerush_sensors::Poller::spawn(
        providers,
        framerush_sensors::POLL_INTERVAL,
        move |sensors| {
            let _ = tx.send(sensors);
        },
    );
    for sensors in rx {
        println!();
        print_sensors(&sensors);
    }
    Ok(())
}

/// Perfil del equipo con la plataforma nativa (R8).
#[cfg(windows)]
fn native_device(os: Os) -> DeviceProfile {
    let mut device = framerush_device::detect(&framerush_platform::WindowsPlatform::new(), os);
    // El umbral de slowdown viene de NVML (sensores), no de la plataforma.
    if let Ok(nvidia) = framerush_sensors::NvidiaGpus::open() {
        nvidia.fill_slowdown(&mut device.gpus);
    }
    device
}

/// Fuera de Windows aún no hay plataforma nativa: solo el SO.
#[cfg(not(windows))]
fn native_device(os: Os) -> DeviceProfile {
    DeviceProfile::undetected(os)
}

/// Planes de energía de la plataforma nativa y el activo.
#[cfg(windows)]
fn native_power() -> Result<(Vec<PowerPlan>, PowerPlan)> {
    use framerush_platform::PowerControl;
    let w = framerush_platform::WindowsPlatform::new();
    let plans = w
        .power_plans()
        .context("no se pudo leer los planes de energía")?;
    let active = w
        .active_power_plan()
        .context("no se pudo leer el plan de energía activo")?;
    Ok((plans, active))
}

#[cfg(not(windows))]
fn native_power() -> Result<(Vec<PowerPlan>, PowerPlan)> {
    bail!("los planes de energía solo están implementados en Windows por ahora")
}

fn undo() -> Result<()> {
    print_report(&run(Op::Undo)?);
    Ok(())
}

fn revert() -> Result<()> {
    print_report(&run(Op::Revert)?);
    Ok(())
}

/// Vista Estado (PLAN.md It5.6). Solo lee: el journal y el valor actual de cada ajuste.
fn status(json: bool) -> Result<()> {
    let path = data_dir()?.join("session.json");
    let journal = FileStore::new(&path)
        .load()
        .with_context(|| format!("no se pudo leer el journal {}", path.display()))?;
    let session = status_lines(&journal);
    let applied = |tweak: Tweak| {
        journal.entries.iter().any(|e| {
            e.status == EntryStatus::Done
                && matches!(e.change, Change::Tweak { tweak: t, .. } if t == tweak)
        })
    };
    let tweaks = tweak_states()?;
    let os = Os::current().context("sistema operativo no admitido")?;
    let mut system = system_state(&native_device(os));
    system.extend(game_configs());
    if json {
        let tweaks: Vec<_> = tweaks
            .iter()
            .map(|(t, values)| {
                serde_json::json!({ "tweak": t, "values": values, "applied": applied(*t) })
            })
            .collect();
        let out = serde_json::json!({ "session": session, "tweaks": tweaks, "system": system });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    if session.is_empty() {
        println!("Sesión: no hay sesión abierta.");
    } else {
        println!("Sesión: abierta ({} cambios)", session.len());
        for l in &session {
            let scope = match l.scope {
                Scope::Session => "sesión",
                Scope::Persistent => "persistente",
            };
            let state = match l.status {
                EntryStatus::Done => "hecho",
                EntryStatus::Pending => "pendiente",
                EntryStatus::Failed => "fallido",
            };
            println!("  lote {:<3} {scope:<11} {state:<9} {}", l.batch, l.what);
        }
    }
    println!(
        "
Equipo (ahora):"
    );
    for l in &system {
        println!("  {:<28} {}", l.label, l.value);
    }
    if !tweaks.is_empty() {
        println!("\nAjustes del sistema (valor actual):");
        for (t, values) in &tweaks {
            let mark = if applied(*t) {
                " · aplicado por FrameRush"
            } else {
                ""
            };
            let unread = if values.is_empty() {
                " · solo se lee como administrador"
            } else {
                ""
            };
            println!("  {}{mark}{unread}", t.id());
            for v in values {
                let value = match &v.value {
                    Some(SavedData::Number(n)) => n.to_string(),
                    Some(SavedData::Text(s)) => format!("\"{s}\""),
                    None => "(no existe)".to_owned(),
                };
                println!("      {} = {value}", v.location);
            }
        }
    }
    Ok(())
}

/// Config de cada juego de Steam instalado que tiene adaptador (`game-config-lock`). Sin Steam,
/// nada.
#[cfg(windows)]
fn game_configs() -> Vec<StateLine> {
    let w = framerush_platform::WindowsPlatform::new();
    let Ok(Some(root)) = framerush_steam::locate() else {
        return vec![];
    };
    let Ok(found) = framerush_steam::installed_apps(&root) else {
        return vec![];
    };
    found
        .apps
        .iter()
        .filter_map(|app| framerush_games::adapter(app.appid).map(|a| (a, app)))
        .flat_map(|(adapter, app)| {
            let install = framerush_games::Install {
                dir: &app.dir,
                steam_root: &root,
            };
            framerush_core::game_config_state(&w, adapter.name(), &adapter.config_files(install))
        })
        .collect()
}

#[cfg(not(windows))]
fn game_configs() -> Vec<StateLine> {
    vec![]
}

/// Valor actual de cada ajuste que admite la plataforma nativa. Leer no pide admin, salvo el
/// arranque de `vbs-hvci-off` (sale sin valores).
#[cfg(windows)]
fn tweak_states() -> Result<Vec<(Tweak, Vec<SavedValue>)>> {
    use framerush_platform::SystemTweaks;
    let w = framerush_platform::WindowsPlatform::new();
    w.supported_tweaks()
        .into_iter()
        .map(|t| {
            // El arranque de `vbs-hvci-off` solo se lee elevado: sin admin, sin valores.
            let values = match w.read_tweak(t) {
                Err(framerush_platform::PlatformError::AccessDenied(_)) => vec![],
                other => other.with_context(|| format!("no se pudo leer {}", t.id()))?,
            };
            Ok((t, values))
        })
        .collect()
}

#[cfg(not(windows))]
fn tweak_states() -> Result<Vec<(Tweak, Vec<SavedValue>)>> {
    Ok(vec![])
}

/// Operación del motor sobre la plataforma nativa.
#[derive(Debug)]
#[cfg_attr(
    not(windows),
    expect(dead_code, reason = "fuera de Windows `run` todavía no ejecuta nada")
)]
enum Op<'a> {
    Apply(Box<ApplyRequest<'a>>),
    Undo,
    Revert,
}

#[cfg(windows)]
fn run(op: Op<'_>) -> Result<Report> {
    use framerush_platform::{Helper, SystemTweaks, WindowsPlatform};

    let store = store()?;
    let unelevated = WindowsPlatform::new();
    let admin = |t: Tweak| unelevated.tweak_requires_admin(t);
    let needs_helper = match &op {
        Op::Apply(request) => {
            request.tweaks.iter().any(|t| admin(*t))
                || !request.memory.is_empty()
                || !request.defender_exclusions.is_empty()
                || request.game.as_ref().is_some_and(|g| g.persistent_priority)
                || request.items.iter().any(|i| match &i.subject {
                    Subject::Processes(ps) => ps.iter().any(|p| p.own != Some(true)),
                    // Detener un servicio siempre pide admin.
                    Subject::Services(names) => !names.is_empty(),
                })
                || foreign(request.game.iter().flat_map(|g| &g.targets))?
        }
        Op::Undo | Op::Revert => {
            let journal = store.load()?;
            let last = journal.last_batch();
            let pending: Vec<&Change> = journal
                .entries
                .iter()
                .filter(|e| e.status != EntryStatus::Failed)
                .filter(|e| matches!(op, Op::Revert) || Some(e.batch) == last)
                .map(|e| &e.change)
                .collect();
            pending.iter().any(|c| match c {
                Change::Tweak { tweak, .. } => admin(*tweak),
                Change::PersistentPriority { .. }
                | Change::StopService { .. }
                | Change::DefenderExclusion { .. } => true,
                _ => false,
            }) || foreign(pending.iter().filter_map(|c| c.target()))?
        }
    };
    let platform = if needs_helper {
        println!("Hay cambios que piden admin: se abre el helper elevado (UAC).");
        let helper = Helper::launch(&helper_exe()?).context("no se pudo abrir el helper")?;
        WindowsPlatform::with_helper(helper)
    } else {
        unelevated
    };
    let engine = framerush_core::Engine::new(&platform, &store);
    Ok(match op {
        Op::Apply(request) => engine.apply(&request).context("no se pudo aplicar")?,
        Op::Undo => engine.undo_last().context("no se pudo deshacer")?,
        Op::Revert => engine.revert_all().context("no se pudo restaurar")?,
    })
}

#[cfg(not(windows))]
fn run(_op: Op<'_>) -> Result<Report> {
    bail!("apply, undo y revert solo están implementados en Windows por ahora")
}

/// P12: alguno de `targets` sigue vivo y no es propio (otro usuario, SYSTEM o elevado), así
/// que solo el helper puede tocarlo.
#[cfg(windows)]
fn foreign<'a>(mut targets: impl Iterator<Item = &'a framerush_platform::Target>) -> Result<bool> {
    let running = scan_processes().context("no se pudo listar los procesos")?;
    Ok(targets.any(|t| {
        running
            .iter()
            .any(|p| p.target == *t && p.own != Some(true))
    }))
}

/// `framerush-helper.exe`, junto al ejecutable de la CLI.
#[cfg(windows)]
fn helper_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("no se encontró el ejecutable de la CLI")?;
    let helper = exe.with_file_name("framerush-helper.exe");
    if !helper.is_file() {
        bail!(
            "falta {} (compílalo con `cargo build --workspace`)",
            helper.display()
        );
    }
    Ok(helper)
}

/// Ajustes que admite la plataforma nativa y, de ellos, los que piden admin.
#[cfg(windows)]
fn native_tweaks() -> (Vec<Tweak>, Vec<Tweak>) {
    use framerush_platform::SystemTweaks;
    let w = framerush_platform::WindowsPlatform::new();
    let all = w.supported_tweaks();
    let admin = all
        .iter()
        .copied()
        .filter(|t| w.tweak_requires_admin(*t))
        .collect();
    (all, admin)
}

#[cfg(not(windows))]
fn native_tweaks() -> (Vec<Tweak>, Vec<Tweak>) {
    (vec![], vec![])
}

/// Servicios de las reglas que existen en este equipo, con su estado (SCM). Los que no se
/// pudieron leer se avisan y no se listan.
#[cfg(windows)]
fn native_services(
    rules: &[framerush_core::Rule],
    os: Os,
) -> Vec<(String, framerush_platform::ServiceState)> {
    let w = framerush_platform::WindowsPlatform::new();
    let (found, errors) = framerush_core::scan_services(&w, rules, os);
    for e in errors {
        println!("Aviso: no se pudo leer el servicio {e}");
    }
    found
}

#[cfg(not(windows))]
fn native_services(
    _rules: &[framerush_core::Rule],
    _os: Os,
) -> Vec<(String, framerush_platform::ServiceState)> {
    vec![]
}

/// Operaciones de memoria que admite la plataforma nativa.
fn native_memory() -> Vec<MemoryOp> {
    #[cfg(windows)]
    {
        use framerush_platform::Capabilities;
        if framerush_platform::WindowsPlatform::new()
            .capabilities()
            .memory
        {
            return MemoryOp::ALL.to_vec();
        }
    }
    vec![]
}

/// Carpeta de datos: `FRAMERUSH_DATA_DIR` o `%LOCALAPPDATA%/framerush` (PLAN.md It1.1).
fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("FRAMERUSH_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .context("no se encontró %LOCALAPPDATA% para guardar el journal")?;
    Ok(PathBuf::from(base).join("framerush"))
}

#[cfg(windows)]
fn store() -> Result<FileStore> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("no se pudo crear {}", dir.display()))?;
    Ok(FileStore::new(dir.join("session.json")))
}

/// Aviso de "Restaurar sesión anterior" (PLAN.md It1.1).
fn notify_open_session() {
    let Ok(dir) = data_dir() else { return };
    let path = dir.join("session.json");
    if !path.exists() {
        return;
    }
    match FileStore::new(&path).load() {
        Ok(j) if j.is_open() => {
            println!("Hay una sesión sin cerrar: `framerush revert` la restaura.\n")
        }
        Ok(_) => {}
        Err(e) => println!("El journal {} no se puede leer: {e}\n", path.display()),
    }
}

fn print_plan(
    plan: &Plan,
    checked: &[&PlanItem],
    tweaks: &[Tweak],
    memory_ops: &[MemoryOp],
    device: &DeviceProfile,
) {
    if !plan.game_tuning_allowed() {
        println!(
            "Anticheat en ejecución: {}. Al juego solo se le toca la prioridad; \
             afinidad y timer quedan desactivados.\n",
            plan.anticheats.join(", ")
        );
    }
    if plan.items.is_empty() {
        println!("Ninguna regla coincide con lo que está en ejecución.");
    }
    for item in &plan.items {
        let on = checked.iter().any(|c| c.rule_id == item.rule_id);
        print_item(item, on);
    }
    let memory: u64 = checked.iter().map(|i| i.memory_bytes()).sum();
    println!(
        "\n{} de {} filas marcadas · {:.0} MB de RAM en procesos marcados",
        checked.len(),
        plan.items.len(),
        mib(memory)
    );
    if !(plan.tweaks.is_empty()
        && plan.optional_tweaks.is_empty()
        && plan.locked_tweaks.is_empty()
        && plan.blocked_tweaks.is_empty())
    {
        println!(
            "\nAjustes del sistema (sesión: vuelve al cerrar el juego; persistente: solo `revert`):"
        );
        for t in plan.tweaks.iter().chain(&plan.optional_tweaks) {
            let mark = if tweaks.contains(t) { "x" } else { " " };
            println!("[{mark}] {:<28} {}", t.id(), scope_name(*t));
        }
        for t in &plan.locked_tweaks {
            println!("[ ] {:<28} {} · requiere admin", t.id(), scope_name(*t));
        }
        for (t, why) in &plan.blocked_tweaks {
            println!("[ ] {:<28} bloqueado: {why}", t.id());
        }
        if plan
            .tweaks
            .iter()
            .chain(&plan.optional_tweaks)
            .any(|t| *t == Tweak::VbsHvciOff)
        {
            println!(
                "    vbs-hvci-off: con --yes pide --confirm-compat; deja sin {}",
                compat_list(device)
            );
        }
    }
    if !(plan.memory.is_empty() && plan.locked_memory.is_empty()) {
        println!("\nMemoria (antes de todo lo demás; no hay nada que revertir):");
        for m in &plan.memory {
            let mark = if memory_ops.contains(m) { "x" } else { " " };
            println!("[{mark}] {}", m.id());
        }
        for m in &plan.locked_memory {
            println!("[ ] {:<28} requiere admin", m.id());
        }
    }
}

fn scope_name(tweak: Tweak) -> &'static str {
    match (tweak_scope(tweak), tweak_requires_reboot(tweak)) {
        (Scope::Session, _) => "sesión",
        (Scope::Persistent, false) => "persistente",
        (Scope::Persistent, true) => "persistente · requiere reinicio",
    }
}

/// `compat-warning` (PLAN.md V6): lo instalado que deja de funcionar sin el hipervisor.
fn compat_list(device: &DeviceProfile) -> String {
    if device.virtualization.is_empty() {
        "nada de lo detectado (WSL2, Docker, Hyper-V, Sandbox ni emuladores con WHPX)".into()
    } else {
        device.virtualization.join(", ")
    }
}

fn print_item(item: &PlanItem, checked: bool) {
    let mark = if checked { "x" } else { " " };
    let lock = if item.locked {
        " · requiere admin"
    } else {
        ""
    };
    let what = match &item.subject {
        Subject::Processes(ps) => format!(
            "{} proceso(s), {:.0} MB",
            ps.len(),
            mib(item.memory_bytes())
        ),
        Subject::Services(names) => names.join(", "),
    };
    println!(
        "[{mark}] {:<20} {:<14} {:?}/{:?} desde {}{lock}\n      {} — {what}",
        item.rule_id,
        format!("{:?}", item.action),
        item.category,
        item.risk,
        item.min_level,
        item.description,
    );
}

fn print_report(report: &Report) {
    if report.lines.is_empty() {
        println!("No había nada que hacer.");
        return;
    }
    for line in &report.lines {
        let (mark, why) = match &line.outcome {
            Outcome::Done => ("ok", String::new()),
            Outcome::Skipped(why) => ("--", format!(" ({why})")),
            Outcome::Failed(e) => ("!!", format!(" ({e})")),
        };
        println!("[{mark}] {}{why}", line.what);
    }
    println!(
        "{} hechos · {} fallidos · {} en total",
        report.done(),
        report.failed(),
        report.lines.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_platform::{StartTime, Target};

    #[test]
    fn reboot_and_compat_are_spelled_out() {
        assert_eq!(
            scope_name(Tweak::VbsHvciOff),
            "persistente · requiere reinicio"
        );
        assert_eq!(scope_name(Tweak::GameDvrOff), "persistente");
        let mut device = DeviceProfile::undetected(framerush_platform::Os::Windows);
        device.virtualization = vec!["WSL2".into(), "Docker Desktop".into()];
        assert_eq!(compat_list(&device), "WSL2, Docker Desktop");
        device.virtualization.clear();
        assert!(compat_list(&device).starts_with("nada de lo detectado"));
    }

    fn proc(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            target: Target {
                pid,
                start_time: StartTime(1),
                exe_path: PathBuf::from(name),
            },
            name: name.into(),
            memory_bytes: 0,
            cpu_percent: None,
            own: Some(true),
            framerush: false,
        }
    }

    #[test]
    fn finds_the_game_by_name_or_pid() {
        let ps = [proc(1, "Game.exe"), proc(2, "other.exe")];
        assert_eq!(find_game(&ps, "game").unwrap().target.pid, 1);
        assert_eq!(find_game(&ps, "GAME.EXE").unwrap().target.pid, 1);
        assert_eq!(find_game(&ps, "2").unwrap().target.pid, 2);
        assert!(find_game(&ps, "missing.exe").is_err());
    }

    #[test]
    fn duplicated_names_ask_for_the_pid() {
        let ps = [proc(1, "game.exe"), proc(2, "game.exe")];
        let err = find_game(&ps, "game.exe").unwrap_err().to_string();
        assert!(err.contains("PID 1, 2"), "{err}");
    }

    #[test]
    fn a_protected_process_cannot_be_the_game() {
        let ps = [proc(1, "dwm.exe")];
        assert!(find_game(&ps, "dwm.exe").is_err());
    }
}
