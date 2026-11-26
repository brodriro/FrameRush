//! `framerush-helper`: proceso elevado mínimo (PLAN.md It2.1). Lo lanza la CLI o la GUI cuando
//! hace falta admin (UAC en Windows, `pkexec` en Linux); atiende solo a ese proceso y termina con
//! él. No se instala como servicio.

use anyhow::Result;
use clap::Parser;

/// Argumentos que pone quien lanza el helper; no está pensado para usarse a mano.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Nombre completo del pipe (`\.\pipe\...`).
    #[cfg(windows)]
    #[arg(long)]
    pipe: String,
    /// Socket Unix del cliente, en su `$XDG_RUNTIME_DIR`.
    #[cfg(target_os = "linux")]
    #[arg(long)]
    socket: std::path::PathBuf,
    /// PID del único proceso al que se atiende.
    #[arg(long)]
    parent: u32,
    /// SID del usuario de ese proceso: el único que puede abrir el pipe.
    #[cfg(windows)]
    #[arg(long)]
    sid: String,
}

/// Lo protegido no se toca ni elevado: misma lista que el plan (nombre o ruta del exe).
#[cfg(any(windows, target_os = "linux"))]
fn may_touch(t: &framerush_platform::Target) -> bool {
    let path = t.exe_path.to_string_lossy();
    let name = t.exe_path.file_name().map(|n| n.to_string_lossy());
    !framerush_core::is_protected(&path) && !name.is_some_and(|n| framerush_core::is_protected(&n))
}

#[cfg(windows)]
const OS: framerush_platform::Os = framerush_platform::Os::Windows;
#[cfg(target_os = "linux")]
const OS: framerush_platform::Os = framerush_platform::Os::Linux;

/// Servicios: solo los que detiene alguna regla en este SO (lista blanca, PLAN.md It2.1).
#[cfg(any(windows, target_os = "linux"))]
fn may_stop_service(name: &str) -> bool {
    framerush_rules::embedded().is_ok_and(|rules| {
        rules
            .iter()
            .any(|r| r.action == framerush_core::Action::StopService && r.matches_service(OS, name))
    })
}

#[cfg(target_os = "linux")]
fn main() -> Result<()> {
    let args = Args::parse();
    framerush_platform::serve_helper(&args.socket, args.parent, may_touch, may_stop_service)?;
    Ok(())
}

#[cfg(windows)]
fn main() -> Result<()> {
    let args = Args::parse();
    framerush_platform::serve_helper(
        &args.pipe,
        args.parent,
        &args.sid,
        may_touch,
        may_stop_service,
        may_exclude,
    )?;
    Ok(())
}

/// Exclusiones de Defender: solo las de un juego instalado de Steam (PLAN.md It5.9).
#[cfg(windows)]
fn may_exclude(exclusion: &framerush_platform::DefenderExclusion) -> bool {
    let games: Vec<std::path::PathBuf> = framerush_steam::locate()
        .ok()
        .flatten()
        .and_then(|root| framerush_steam::installed_apps(&root).ok())
        .map(|apps| apps.apps.into_iter().map(|a| a.dir).collect())
        .unwrap_or_default();
    exclusion_allowed(exclusion, &games)
}

/// La carpeta de uno de `games` o un `.exe` dentro de ella, con ruta absoluta y sin `..`.
#[cfg(windows)]
fn exclusion_allowed(
    exclusion: &framerush_platform::DefenderExclusion,
    games: &[std::path::PathBuf],
) -> bool {
    use framerush_platform::DefenderExclusion;
    use std::path::{Component, Path};
    let plain =
        |p: &Path| p.is_absolute() && !p.components().any(|c| matches!(c, Component::ParentDir));
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    match exclusion {
        DefenderExclusion::Path(p) => plain(p) && games.iter().any(|g| lower(g) == lower(p)),
        DefenderExclusion::Process(p) => {
            plain(p)
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                && games
                    .iter()
                    .any(|g| lower(p).starts_with(&format!("{}\\", lower(g))))
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use framerush_platform::DefenderExclusion::{Path, Process};
    use std::path::PathBuf;

    #[test]
    fn only_installed_game_folders_and_their_exes() {
        let game =
            PathBuf::from(r"D:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive");
        let games = [game.clone()];
        let ok = |e| exclusion_allowed(&e, &games);
        assert!(ok(Path(game.clone())));
        assert!(ok(Path(PathBuf::from(
            r"d:\steamlibrary\steamapps\common\counter-strike global offensive"
        ))));
        assert!(ok(Process(game.join(r"game\bin\win64\cs2.exe"))));
        // Ni otra carpeta, ni la biblioteca, ni salir con `..`, ni algo que no sea un exe, ni
        // otra carpeta que empieza igual.
        assert!(!ok(Path(PathBuf::from(r"C:\Windows"))));
        assert!(!ok(Path(PathBuf::from(
            r"D:\SteamLibrary\steamapps\common"
        ))));
        assert!(!ok(Process(game.join(r"..\..\..\..\Windows\evil.exe"))));
        assert!(!ok(Process(game.join("cs2.dll"))));
        assert!(!ok(Process(PathBuf::from(
            r"D:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive2\x.exe"
        ))));
        assert!(!ok(Path(PathBuf::from("relativa"))));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn main() -> Result<()> {
    let _ = Args::parse();
    anyhow::bail!("el helper solo está implementado en Windows y Linux por ahora")
}
