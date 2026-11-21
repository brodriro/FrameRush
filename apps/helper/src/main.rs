//! `framerush-helper`: proceso elevado mínimo (PLAN.md It2.1). Lo lanza la CLI o la GUI con UAC
//! cuando hace falta admin; atiende solo a ese proceso y termina con él. No se instala como
//! servicio.

use anyhow::Result;
use clap::Parser;

/// Argumentos que pone quien lanza el helper; no está pensado para usarse a mano.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Nombre completo del pipe (`\.\pipe\...`).
    #[arg(long)]
    pipe: String,
    /// PID del único proceso al que se atiende.
    #[arg(long)]
    parent: u32,
    /// SID del usuario de ese proceso: el único que puede abrir el pipe.
    #[arg(long)]
    sid: String,
}

#[cfg(windows)]
fn main() -> Result<()> {
    let args = Args::parse();
    framerush_platform::serve_helper(&args.pipe, args.parent, &args.sid)?;
    Ok(())
}

#[cfg(not(windows))]
fn main() -> Result<()> {
    let _ = Args::parse();
    anyhow::bail!("el helper solo está implementado en Windows por ahora")
}
