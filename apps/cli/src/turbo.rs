//! `framerush turbo` (It5.2 en la CLI): lanza el juego de Steam si no está abierto, espera a que
//! arranque, aplica el nivel con cerrar → congelar (P6), congela cada `--rescan` las instancias
//! nuevas y, al cerrarse el juego o con Ctrl+C, deshace lo de sesión o solo reanuda lo
//! congelado (`--no-auto-revert`, P5). Una sola plataforma para toda la sesión: el helper
//! elevado, si hace falta, se abre una vez.

use super::{
    ApplyArgs, Op, TurboApply, apply_with, build_plan, freeze_instead_of_kill, platform_for,
    print_report, run_on,
};
use anyhow::{Context, Result, bail};
use clap::Args;
use framerush_core::{Action, ApplyRequest, PlanItem, Report, game_running, is_protected};
use framerush_platform::{ProcessInfo, Target, WindowsPlatform, scan_processes};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Cada cuánto se mira si el juego sigue (el mismo intervalo que el watchdog de la GUI).
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Args)]
pub(super) struct TurboArgs {
    #[command(flatten)]
    apply: ApplyArgs,
    /// `turbo-rescan`: segundos entre re-escaneos que congelan los procesos nuevos.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
    rescan: u64,
    /// `keep-alive-list`: nombres que nunca se congelan (separados por comas). Sin ella, las
    /// apps de voz detectadas en el equipo.
    #[arg(long, value_delimiter = ',')]
    keep_alive: Option<Vec<String>>,
    /// Al cerrar el juego solo reanuda lo congelado; el resto queda hasta `framerush revert`.
    #[arg(long)]
    no_auto_revert: bool,
}

/// Juego de Steam de Turbo: `--appid` o el que Steam tiene en marcha.
struct SteamGame {
    appid: u32,
    name: String,
    app: framerush_steam::App,
}

fn steam_game(appid: Option<u32>) -> Result<SteamGame> {
    let root = framerush_steam::locate()
        .context("no se pudo leer Steam")?
        .context("Steam no está instalado")?;
    let apps = framerush_steam::installed_apps(&root)
        .context("no se pudo leer la biblioteca de Steam")?
        .apps;
    let appid = match appid {
        Some(id) => id,
        None => framerush_steam::running_app_id()
            .context("no se pudo leer Steam")?
            .context("indica el juego con --appid (o ábrelo desde Steam)")?,
    };
    let app = apps
        .into_iter()
        .find(|a| a.appid == appid)
        .with_context(|| format!("el appid {appid} no está instalado en Steam"))?;
    Ok(SteamGame {
        appid,
        name: app.name.clone(),
        app,
    })
}

/// Procesos del juego en marcha, sin los protegidos.
fn game_processes(game: &SteamGame, processes: &[ProcessInfo]) -> Vec<Target> {
    framerush_steam::game_processes(&game.app, processes)
        .into_iter()
        .filter(|p| !is_protected(&p.name))
        .map(|p| p.target.clone())
        .collect()
}

pub(super) fn turbo(args: &TurboArgs) -> Result<()> {
    let game = steam_game(args.apply.game.appid)?;
    if args.apply.game.game.is_some() {
        bail!("Turbo usa el juego de Steam (--appid): --game no se admite aquí");
    }
    let opts = TurboApply {
        keep_alive: args.keep_alive.as_deref(),
    };
    if !args.apply.yes {
        println!(
            "Turbo para «{}» ({}): con --yes lo lanza si no está abierto, aplica este plan \
             congelando en vez de cerrar y lo deshace al salir del juego.\n",
            game.name, game.appid
        );
        apply_with(&args.apply, Some(&opts), |_| {
            bail!("sin --yes no se aplica")
        })?;
        return Ok(());
    }

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .context("no se pudo instalar el manejador de Ctrl+C")?;

    if !wait_for_game(&game, &stop)? {
        println!("Cancelado: no se ha tocado nada.");
        return Ok(());
    }

    let mut platform: Option<WindowsPlatform> = None;
    let applied = apply_with(&args.apply, Some(&opts), |op| {
        let p = platform_for(&op)?;
        let report = run_on(&p, op);
        platform = Some(p);
        report
    })?;
    let (Some(applied), Some(platform)) = (applied, platform) else {
        return Ok(());
    };
    tracing::info!(appid = game.appid, frozen = ?applied.suspended, "turbo aplicado");
    println!(
        "\nTurbo activo: vigilando «{}». Al cerrarlo se {}. Ctrl+C hace lo mismo ya.",
        game.name,
        if args.no_auto_revert {
            "reanuda lo congelado"
        } else {
            "deshace lo de sesión"
        }
    );

    let every = Duration::from_secs(args.rescan);
    let mut last_rescan = Instant::now();
    let reason = loop {
        std::thread::sleep(WATCH_INTERVAL);
        if stop.load(Ordering::Relaxed) {
            break "Ctrl+C";
        }
        // Un escaneo fallido no es un juego cerrado: se vuelve a mirar.
        let Ok(processes) = scan_processes() else {
            continue;
        };
        if !game_running(&applied.game, &processes) {
            break "el juego se cerró";
        }
        if last_rescan.elapsed() >= every {
            last_rescan = Instant::now();
            match rescan(&platform, args, &applied.suspended) {
                Ok(report) if !report.lines.is_empty() => {
                    println!("\nRe-escaneo:");
                    print_report(&report);
                }
                Ok(_) => {}
                Err(e) => println!("\nRe-escaneo fallido: {e:#}"),
            }
        }
    };

    println!("\nTurbo termina ({reason}).");
    tracing::info!(reason, "turbo termina");
    let op = if args.no_auto_revert {
        Op::ResumeFrozen
    } else {
        Op::RevertSession
    };
    print_report(&run_on(&platform, op)?);
    if args.no_auto_revert {
        println!("Lo persistente y lo de sesión que no se congeló siguen: `framerush revert`.");
    }
    Ok(())
}

/// Lanza el juego con Steam si no está abierto y espera a que arranque y Steam lo marque en
/// marcha (`apply` toma el juego de `RunningAppID`). `false` si se canceló con Ctrl+C.
fn wait_for_game(game: &SteamGame, stop: &AtomicBool) -> Result<bool> {
    let running = || -> Result<bool> {
        let processes = scan_processes().context("no se pudo listar los procesos")?;
        let steam = framerush_steam::running_app_id().context("no se pudo leer Steam")?;
        Ok(steam == Some(game.appid) && !game_processes(game, &processes).is_empty())
    };
    if running()? {
        return Ok(true);
    }
    framerush_platform::steam_launch(game.appid).context("no se pudo lanzar el juego")?;
    println!(
        "Lanzando «{}» desde Steam; esperando a que arranque (Ctrl+C cancela sin tocar nada)…",
        game.name
    );
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(WATCH_INTERVAL);
        if running()? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `turbo-rescan`: congela las instancias nuevas de las reglas congeladas al aplicar. Lo ya
/// congelado se salta (el journal lo tiene).
fn rescan(platform: &WindowsPlatform, args: &TurboArgs, rows: &[String]) -> Result<Report> {
    let a = &args.apply;
    let (plan, _, _) = build_plan(a.level, a.admin, None, args.keep_alive.as_deref())?;
    let owned: Vec<PlanItem> = plan
        .items
        .into_iter()
        .filter(|i| !i.locked && rows.contains(&i.rule_id))
        .map(freeze_instead_of_kill)
        .filter(|i| i.action == Action::Suspend)
        .collect();
    if owned.is_empty() {
        return Ok(Report::default());
    }
    let request = ApplyRequest {
        items: owned.iter().collect(),
        ..ApplyRequest::default()
    };
    run_on(platform, Op::Apply(Box::new(request)))
}
