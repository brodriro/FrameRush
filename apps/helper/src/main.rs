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
    // Lo protegido no se toca ni elevado: misma lista que el plan (nombre o ruta del exe).
    let may_touch = |t: &framerush_platform::Target| {
        let path = t.exe_path.to_string_lossy();
        let name = t.exe_path.file_name().map(|n| n.to_string_lossy());
        !framerush_core::is_protected(&path)
            && !name.is_some_and(|n| framerush_core::is_protected(&n))
    };
    // Servicios: solo los que detiene alguna regla (lista blanca, PLAN.md It2.1).
    let may_stop_service = |name: &str| {
        framerush_rules::embedded().is_ok_and(|rules| {
            rules.iter().any(|r| {
                r.action == framerush_core::Action::StopService
                    && r.matches_service(framerush_platform::Os::Windows, name)
            })
        })
    };
    framerush_platform::serve_helper(
        &args.pipe,
        args.parent,
        &args.sid,
        may_touch,
        may_stop_service,
    )?;
    Ok(())
}

#[cfg(not(windows))]
fn main() -> Result<()> {
    let _ = Args::parse();
    anyhow::bail!("el helper solo está implementado en Windows por ahora")
}
