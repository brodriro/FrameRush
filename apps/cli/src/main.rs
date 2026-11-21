//! `framerush`: CLI sobre el mismo núcleo que la GUI.
//!
//! `scan` lista procesos y `plan --level` muestra qué se haría (dry-run, PLAN.md It3.3).
//! `apply` aplica el plan (solo con `--yes`), `undo` deshace el último apply y `revert` toda la
//! sesión. Todo cambio pasa por el journal `%LOCALAPPDATA%/framerush/session.json`; la variable
//! `FRAMERUSH_DATA_DIR` cambia esa carpeta (tests). `sensors` y `power` llegan en M2.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use framerush_core::{
    ApplyRequest, FileStore, JournalStore, Level, Outcome, Plan, PlanItem, PlanOptions, Report,
    Snapshot, Subject, defaults, plan,
};
use framerush_device::DeviceProfile;
use framerush_platform::{Os, scan_processes};
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
    },
    /// Aplica el plan de un nivel. Sin `--yes` solo lo muestra.
    Apply {
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
        /// Confirma que se apliquen los cambios.
        #[arg(long)]
        yes: bool,
    },
    /// Deshace el último `apply`.
    Undo,
    /// Deshace toda la sesión (Restaurar).
    Revert,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Scan => scan(),
        Command::Plan { level, admin } => show_plan(level, admin),
        Command::Apply {
            level,
            admin,
            include,
            exclude,
            yes,
        } => apply(level, admin, &include, &exclude, yes),
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

fn build_plan(level: Level, admin: bool) -> Result<Plan> {
    let Some(os) = Os::current() else {
        bail!("sistema operativo no admitido");
    };
    // La detección del equipo llega en M2; hasta entonces el perfil solo tiene el SO.
    let device = DeviceProfile::undetected(os);
    let rules = framerush_rules::embedded().context("las reglas embebidas no son válidas")?;
    let snapshot = Snapshot {
        processes: scan_processes().context("no se pudo listar los procesos")?,
        // Los servicios se consultan con el SCM / systemd en M5.
        services: vec![],
        game: None,
    };
    let options = PlanOptions {
        elevated: admin,
        keep_alive: defaults::keep_alive(&device),
    };
    Ok(plan(level, &snapshot, &rules, &device, &options))
}

fn show_plan(level: Level, admin: bool) -> Result<()> {
    notify_open_session();
    let plan = build_plan(level, admin)?;
    println!("Plan para el nivel {level} (dry-run: no se toca nada)");
    let checked: Vec<&PlanItem> = plan.checked().collect();
    print_plan(&plan, &checked);
    Ok(())
}

/// Filas marcadas por el nivel, más `include` y menos `exclude`. Las filas con candado no se
/// pueden marcar.
fn select<'a>(plan: &'a Plan, include: &[String], exclude: &[String]) -> Result<Vec<&'a PlanItem>> {
    for id in include.iter().chain(exclude) {
        if !plan.items.iter().any(|i| &i.rule_id == id) {
            bail!("la regla `{id}` no está en el plan (¿no está en ejecución?)");
        }
    }
    if let Some(i) = plan
        .items
        .iter()
        .find(|i| i.locked && include.contains(&i.rule_id))
    {
        bail!("la regla `{}` requiere admin: usa --admin", i.rule_id);
    }
    Ok(plan
        .items
        .iter()
        .filter(|i| !i.locked && (i.default_checked || include.contains(&i.rule_id)))
        .filter(|i| !exclude.contains(&i.rule_id))
        .collect())
}

fn apply(
    level: Level,
    admin: bool,
    include: &[String],
    exclude: &[String],
    yes: bool,
) -> Result<()> {
    notify_open_session();
    let plan = build_plan(level, admin)?;
    let checked = select(&plan, include, exclude)?;
    if !yes {
        println!("Plan para el nivel {level} (no se aplica sin --yes)");
        print_plan(&plan, &checked);
        println!("\nRevisa las filas y repite con --yes para aplicarlas.");
        return Ok(());
    }
    if checked.is_empty() {
        println!("No hay filas marcadas: no se toca nada.");
        return Ok(());
    }
    let request = ApplyRequest {
        items: checked,
        // La detección del juego en primer plano está pendiente (M1).
        game: None,
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

fn print_plan(plan: &Plan, checked: &[&PlanItem]) {
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
