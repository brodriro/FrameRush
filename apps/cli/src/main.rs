//! `fpsboost`: CLI sobre el mismo núcleo que la GUI.
//!
//! Los subcomandos (`scan`, `plan --level`, `apply`, `revert`, `sensors`, `power`) llegan en M1 y M2.

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Sube los FPS limitando procesos de fondo por niveles.
#[derive(Debug, Parser)]
#[command(name = "fpsboost", version, about)]
struct Cli {}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let _cli = Cli::parse();
    tracing::debug!("sin subcomandos todavía");
}
