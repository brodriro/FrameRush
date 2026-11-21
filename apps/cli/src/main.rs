//! `framerush`: CLI sobre el mismo núcleo que la GUI.
//!
//! Por ahora solo lee: `scan` lista procesos y `plan --level` muestra qué se haría (dry-run,
//! PLAN.md It3.3). `apply`, `revert`, `sensors` y `power` llegan con las implementaciones de
//! plataforma (M1 y M2).

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use framerush_core::{Level, PlanItem, PlanOptions, Snapshot, Subject, defaults, plan};
use framerush_device::DeviceProfile;
use framerush_platform::{Os, scan_processes};
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
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Scan => scan(),
        Command::Plan { level, admin } => show_plan(level, admin),
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

fn show_plan(level: Level, admin: bool) -> Result<()> {
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
    let plan = plan(level, &snapshot, &rules, &device, &options);

    println!("Plan para el nivel {level} (dry-run: no se toca nada)");
    println!("Equipo sin detectar todavía: los valores por defecto son estimados.");
    println!("Servicios: todavía no se consultan.\n");
    if plan.items.is_empty() {
        println!("Ninguna regla coincide con lo que está en ejecución.");
    }
    for item in &plan.items {
        print_item(item);
    }
    let checked: Vec<&PlanItem> = plan.checked().collect();
    let memory: u64 = checked.iter().map(|i| i.memory_bytes()).sum();
    println!(
        "\n{} de {} filas marcadas · {:.0} MB de RAM en procesos marcados",
        checked.len(),
        plan.items.len(),
        mib(memory)
    );
    Ok(())
}

fn print_item(item: &PlanItem) {
    let mark = if item.default_checked { "x" } else { " " };
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
