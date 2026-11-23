//! Un `GameAdapter` por juego (CS2 primero, PLAN.md It5.1): la config del juego, el cfg propio
//! con el tope de FPS, el cfg de medición y las opciones de lanzamiento.
//!
//! Un adaptador no escribe nada: dice qué archivos dejar y cómo ([`GameFileWrite`]), y `core`
//! lo aplica con el journal (persistente, solo lo revierte Restaurar).

mod cs2;

pub use cs2::Cs2;

use framerush_core::GameFileWrite;
use framerush_device::DeviceProfile;
use std::path::{Path, PathBuf};

/// Errores al leer la config del juego.
#[derive(Debug, thiserror::Error)]
pub enum GameError {
    #[error("no se pudo leer {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, GameError>;

/// Dónde está el juego en este equipo.
#[derive(Debug, Clone, Copy)]
pub struct Install<'a> {
    /// Carpeta del juego (`steamapps/common/<installdir>`).
    pub dir: &'a Path,
    /// Carpeta de Steam (la de `userdata`).
    pub steam_root: &'a Path,
}

/// Lo que el usuario marcó en el panel del adaptador (design/SETTINGS.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameOptions {
    /// `game-config-tune` (opt-in): ajustes de vídeo recomendados según el equipo.
    pub tune: bool,
    /// `game-config-lock`: la config de vídeo queda de solo lectura. Sin `tune` no hace nada.
    pub lock: bool,
    /// `fps-cap` en el cfg propio: 0 = sin tope; `None` = no se escribe el cfg propio.
    pub fps_cap: Option<u32>,
    /// `bench-cfg`: cfg de medición.
    pub bench: bool,
}

/// Lo que se le hará al juego y lo que no se pudo.
#[derive(Debug, Default)]
pub struct GamePlan {
    pub writes: Vec<GameFileWrite>,
    /// Motivos por los que algo no se hará (p. ej. no hay `cs2_video.txt` todavía).
    pub notes: Vec<String>,
}

/// Adaptador de un juego.
pub trait GameAdapter {
    fn appid(&self) -> u32;
    fn name(&self) -> &'static str;
    /// Archivos de config que lee y escribe, para mostrarlos.
    fn config_files(&self, install: Install<'_>) -> Vec<PathBuf>;
    /// Comando para medir con el cfg de medición (p. ej. `exec fpsbench`).
    fn bench_command(&self) -> &'static str;
    /// Opciones de lanzamiento recomendadas (`launch-options-copy`).
    fn launch_options(&self) -> &'static str;
    /// Qué archivos dejar y cómo, según el equipo y lo marcado. Lee la config actual.
    fn plan(
        &self,
        install: Install<'_>,
        device: &DeviceProfile,
        options: GameOptions,
    ) -> Result<GamePlan>;
}

/// El adaptador del juego `appid`, si lo hay.
pub fn adapter(appid: u32) -> Option<&'static dyn GameAdapter> {
    match appid {
        Cs2::APPID => Some(&Cs2),
        _ => None,
    }
}
