//! `framerush`: CLI sobre el mismo núcleo que la GUI.
//!
//! `scan` lista procesos y `plan --level` muestra qué se haría (dry-run, PLAN.md It3.3).
//! `apply` aplica el plan (solo con `--yes`), `undo` deshace el último apply y `revert` toda la
//! sesión. Todo cambio pasa por el journal `%LOCALAPPDATA%/framerush/session.json`; la variable
//! `FRAMERUSH_DATA_DIR` cambia esa carpeta (tests). `sensors` y `power` llegan en M2.
//!
//! El juego se indica con `--game <exe|pid>`: desde una terminal, la ventana en primer plano es
//! la propia terminal. El reconocimiento automático por la biblioteca de Steam llega en M3.

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use framerush_core::{
    ApplyRequest, FileStore, GameBoost, JournalStore, Level, Outcome, Plan, PlanItem, PlanOptions,
    Report, Scope, Snapshot, Subject, defaults, is_protected, plan, tweak_scope,
};
use framerush_device::DeviceProfile;
use framerush_platform::{Os, Priority, ProcessInfo, Tweak, scan_processes};
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
}

#[derive(Debug, Args)]
struct ApplyArgs {
    /// normal, agresivo o extremo.
    #[arg(long, default_value_t = Level::default())]
    level: Level,
    /// Trata la ejecución como elevada (quita los candados de las filas que piden admin).
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
    /// Confirma que se apliquen los cambios.
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct GameArgs {
    /// Proceso del juego: nombre del ejecutable (cs2.exe) o PID. Queda fuera del plan.
    #[arg(long)]
    game: Option<String>,
    /// Prioridad del juego (PLAN.md R3, P1).
    #[arg(long, value_enum, default_value_t = GamePriority::Alta, requires = "game")]
    game_priority: GamePriority,
    /// Segunda confirmación para `--game-priority realtime` (nunca se persiste).
    #[arg(long, requires = "game")]
    confirm_realtime: bool,
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
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn scan() -> Result<()> {
    let procs = scan_processes().context("no se pudo listar los procesos")?;
    println!("{:>7}  {:>9}  NOMBRE", "PID", "MEMORIA");
    for p in &procs {
        println!(
            "{:>7}  {:>6.0} MB  {}",
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

fn build_plan(
    level: Level,
    admin: bool,
    game: Option<&str>,
) -> Result<(Plan, Option<ProcessInfo>)> {
    let Some(os) = Os::current() else {
        bail!("sistema operativo no admitido");
    };
    // La detección del equipo llega en M2; hasta entonces el perfil solo tiene el SO.
    let device = DeviceProfile::undetected(os);
    let rules = framerush_rules::embedded().context("las reglas embebidas no son válidas")?;
    let processes = scan_processes().context("no se pudo listar los procesos")?;
    let game = game.map(|q| find_game(&processes, q)).transpose()?;
    let snapshot = Snapshot {
        processes,
        // Los servicios se consultan con el SCM / systemd en M5.
        services: vec![],
        game: game.as_ref().map(|g| g.target.clone()),
        tweaks: native_tweaks(),
    };
    let options = PlanOptions {
        elevated: admin,
        keep_alive: defaults::keep_alive(&device).value,
    };
    Ok((plan(level, &snapshot, &rules, &device, &options), game))
}

fn print_game(game: Option<&ProcessInfo>, args: &GameArgs) {
    match game {
        Some(g) => println!(
            "Juego: {} (pid {}) → prioridad {:?}\n",
            g.name, g.target.pid, args.game_priority
        ),
        None => println!("Juego: sin indicar (--game); su prioridad no se toca.\n"),
    }
}

fn show_plan(level: Level, admin: bool, args: &GameArgs) -> Result<()> {
    notify_open_session();
    let (plan, game) = build_plan(level, admin, args.game.as_deref())?;
    println!("Plan para el nivel {level} (dry-run: no se toca nada)");
    print_game(game.as_ref(), args);
    let checked: Vec<&PlanItem> = plan.checked().collect();
    print_plan(&plan, &checked, &plan.tweaks);
    Ok(())
}

/// Filas y ajustes marcados por el nivel, más `include` y menos `exclude`. Las filas con candado
/// no se pueden marcar.
fn select<'a>(
    plan: &'a Plan,
    include: &[String],
    exclude: &[String],
) -> Result<(Vec<&'a PlanItem>, Vec<Tweak>)> {
    for id in include.iter().chain(exclude) {
        let known =
            plan.items.iter().any(|i| &i.rule_id == id) || plan.tweaks.iter().any(|t| t.id() == id);
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
    let items = plan
        .items
        .iter()
        .filter(|i| !i.locked && (i.default_checked || include.contains(&i.rule_id)))
        .filter(|i| !exclude.contains(&i.rule_id))
        .collect();
    let tweaks = plan
        .tweaks
        .iter()
        .copied()
        .filter(|t| !exclude.iter().any(|id| id == t.id()))
        .collect();
    Ok((items, tweaks))
}

fn apply(args: &ApplyArgs) -> Result<()> {
    notify_open_session();
    let level = args.level;
    let (plan, game) = build_plan(level, args.admin, args.game.game.as_deref())?;
    let (checked, tweaks) = select(&plan, &args.include, &args.exclude)?;
    if args.game.game_priority == GamePriority::Realtime && !args.game.confirm_realtime {
        bail!("Tiempo real puede congelar el equipo: confírmalo con --confirm-realtime");
    }
    if !args.yes {
        println!("Plan para el nivel {level} (no se aplica sin --yes)");
        print_game(game.as_ref(), &args.game);
        print_plan(&plan, &checked, &tweaks);
        println!("\nRevisa las filas y repite con --yes para aplicarlas.");
        return Ok(());
    }
    if checked.is_empty() && tweaks.is_empty() && game.is_none() {
        println!("No hay filas ni ajustes marcados ni juego: no se toca nada.");
        return Ok(());
    }
    let request = ApplyRequest {
        items: checked,
        tweaks,
        game: game.map(|g| GameBoost {
            target: g.target,
            priority: args.game.game_priority.into(),
            realtime_confirmed: args.game.confirm_realtime,
            fullscreen_optimizations_off: false,
        }),
    };
    let report = run(Op::Apply(request))?;
    print_report(&report);
    println!("Deshacer: `framerush undo` (último apply) o `framerush revert` (toda la sesión).");
    Ok(())
}

fn undo() -> Result<()> {
    print_report(&run(Op::Undo)?);
    Ok(())
}

fn revert() -> Result<()> {
    print_report(&run(Op::Revert)?);
    Ok(())
}

/// Operación del motor sobre la plataforma nativa.
#[derive(Debug)]
enum Op<'a> {
    Apply(ApplyRequest<'a>),
    Undo,
    Revert,
}

#[cfg(windows)]
fn run(op: Op<'_>) -> Result<Report> {
    let platform = framerush_platform::WindowsPlatform::new();
    let store = store()?;
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

/// Ajustes persistentes que admite la plataforma nativa.
#[cfg(windows)]
fn native_tweaks() -> Vec<Tweak> {
    use framerush_platform::SystemTweaks;
    framerush_platform::WindowsPlatform::new().supported_tweaks()
}

#[cfg(not(windows))]
fn native_tweaks() -> Vec<Tweak> {
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

fn print_plan(plan: &Plan, checked: &[&PlanItem], tweaks: &[Tweak]) {
    if !plan.game_tuning_allowed() {
        println!(
            "Anticheat en ejecución: {}. Al juego solo se le toca la prioridad; \
             afinidad y timer quedan desactivados.\n",
            plan.anticheats.join(", ")
        );
    }
    println!("Equipo sin detectar todavía: los valores por defecto son estimados.");
    println!("Servicios: todavía no se consultan.\n");
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
    if !plan.tweaks.is_empty() {
        println!(
            "\nAjustes del sistema (sesión: vuelve al cerrar el juego; persistente: solo `revert`):"
        );
        for t in &plan.tweaks {
            let mark = if tweaks.contains(t) { "x" } else { " " };
            let scope = match tweak_scope(*t) {
                Scope::Session => "sesión",
                Scope::Persistent => "persistente",
            };
            println!("[{mark}] {:<28} {scope}", t.id());
        }
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
