//! Counter-Strike 2 (appid 730). Rutas, claves y valores: el proyecto de referencia
//! CS2-FPS-Boost (`CS2-FPS.ps1`, probado en hardware real; PLAN.md It5.1).
//!
//! - `cs2_video.txt`: uno por cuenta, en `<Steam>/userdata/<id>/730/local/cfg/`. Se editan
//!   solo las claves de abajo, línea a línea, como la referencia.
//! - `framerush.cfg` y `fpsbench.cfg`: en `<juego>/game/csgo/cfg/`, son nuestros.

use crate::{GameAdapter, GameError, GameOptions, GamePlan, Install, Result};
use framerush_core::{GameFileWrite, defaults};
use framerush_device::{DeviceProfile, GpuVendor};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Cs2;

impl Cs2 {
    pub const APPID: u32 = 730;
}

/// Valores de rendimiento de la referencia que no dependen del equipo (V-Sync, MSAA, sombras,
/// partículas, AO, FSR…).
const COMPETITIVE: &[(&str, &str)] = &[
    ("setting.fullscreen", "1"),
    ("setting.fullscreen_min_on_focus_loss", "0"),
    ("setting.mat_vsync", "0"),
    ("setting.msaa_samples", "0"),
    ("setting.mat_antialias", "0"),
    ("setting.r_csgo_cmaa_enable", "0"),
    ("setting.shaderquality", "0"),
    ("setting.r_aoproxy_enable", "0"),
    ("setting.r_ssao", "0"),
    ("setting.r_texturefilteringquality", "0"),
    ("setting.r_particle_max_detail_level", "0"),
    ("setting.r_particle_shadows", "0"),
    ("setting.csm_viewmodel_shadows", "0"),
    ("setting.csm_quality_level", "0"),
    ("setting.cpu_level", "0"),
    ("setting.gpu_level", "0"),
    ("setting.r_csgo_fsr_upsample", "0"),
    ("setting.sc_hdr_enabled_override", "3"),
];

/// Texturas bajas con esta VRAM o menos (PLAN.md It5.1).
const LOW_TEXTURES_MAX_VRAM: u64 = 4 << 30;
/// `mem_level 2` con 16 GB instalados. El SO da algo menos de lo instalado (reservado por el
/// hardware: 15,8 GB en el equipo de la referencia), así que el corte es 15 GiB.
const MEM_LEVEL_2_MIN_RAM: u64 = 15 << 30;

/// La GPU con la que jugará: la que elige `gpu-preference` o, si solo hay una, esa.
fn game_gpu(device: &DeviceProfile) -> Option<&framerush_device::Gpu> {
    let only = match device.gpus.as_slice() {
        [only] => Some(only),
        _ => None,
    };
    defaults::gpu_preference(device).or(only)
}

/// Las claves que `game-config-tune` escribe en este equipo. Lo que depende del equipo solo se
/// pone si se conoce el dato; si no, se deja lo que haya (R8).
pub fn video_settings(device: &DeviceProfile) -> Vec<(&'static str, &'static str)> {
    let mut out = COMPETITIVE.to_vec();
    let gpu = game_gpu(device);
    // Reflex solo existe con NVIDIA.
    if gpu.is_some_and(|g| g.vendor == GpuVendor::Nvidia) {
        out.push(("setting.r_low_latency", "1"));
    }
    if gpu
        .and_then(|g| g.dedicated_vram_bytes)
        .is_some_and(|v| v <= LOW_TEXTURES_MAX_VRAM)
    {
        out.push(("setting.gpu_mem_level", "0"));
    }
    if device
        .memory_bytes
        .is_some_and(|m| m >= MEM_LEVEL_2_MIN_RAM)
    {
        out.push(("setting.mem_level", "2"));
    }
    out
}

/// Cambia o añade cada clave en el texto de `cs2_video.txt` sin tocar el resto: la línea de
/// la clave se sustituye y, si no está, se añade antes de la última `}`. Mantiene los saltos
/// de línea del archivo.
pub fn patch_video(text: &str, settings: &[(&str, &str)]) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for (key, value) in settings {
        let line = format!("\t\"{key}\"\t\t\"{value}\"");
        let quoted = format!("\"{key}\"");
        let found = lines.iter().position(|l| {
            l.trim_start()
                .strip_prefix(&quoted)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        });
        match found {
            Some(i) => lines[i] = line,
            None => match lines.iter().rposition(|l| l.trim() == "}") {
                Some(close) => lines.insert(close, line),
                None => lines.push(line),
            },
        }
    }
    let mut out = lines.join(newline);
    if text.ends_with('\n') {
        out.push_str(newline);
    }
    out
}

/// `framerush.cfg` (se carga con `+exec framerush`): el contenido de la referencia con el tope
/// de FPS elegido.
pub fn own_cfg(fps_cap: u32) -> String {
    [
        "// framerush - generado por FrameRush; se borra al Restaurar".to_owned(),
        format!("fps_max {fps_cap}"),
        "fps_max_ui 60".into(),
        "r_drawtracers_firstperson 0".into(),
        "cl_disablehtmlmotd 1".into(),
        "cl_showhelp 0".into(),
        "cl_hud_telemetry_frametime_show 0".into(),
        "cl_hud_telemetry_net_misdelivery_show 0".into(),
        "cl_hud_telemetry_ping_show 0".into(),
        "cl_hud_telemetry_serverrecvmargin_graph_show 0".into(),
        "// Reflex: duerme después del tick de cliente (menos latencia con fps_max 0)".into(),
        "engine_low_latency_sleep_after_client_tick 1".into(),
        "echo \"[framerush] perfil cargado\"".into(),
        String::new(),
    ]
    .join("\r\n")
}

/// `fpsbench.cfg` (`exec fpsbench`): el contador de FPS para medir antes y después.
pub fn bench_cfg() -> String {
    [
        "// fpsbench - para medir antes/despues:  exec fpsbench",
        "cl_showfps 1",
        "echo \"[fpsbench] contador de FPS activado (cl_showfps 0 para apagarlo)\"",
        "",
    ]
    .join("\r\n")
}

fn cfg_dir(dir: &Path) -> PathBuf {
    dir.join("game").join("csgo").join("cfg")
}

/// `cs2_video.txt` de cada cuenta de Steam que ya abrió CS2.
fn video_files(steam_root: &Path) -> Vec<PathBuf> {
    let Ok(users) = std::fs::read_dir(steam_root.join("userdata")) else {
        return vec![];
    };
    let mut out: Vec<PathBuf> = users
        .flatten()
        .map(|u| {
            u.path()
                .join("730")
                .join("local")
                .join("cfg")
                .join("cs2_video.txt")
        })
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out
}

impl GameAdapter for Cs2 {
    fn appid(&self) -> u32 {
        Self::APPID
    }

    fn name(&self) -> &'static str {
        "Counter-Strike 2"
    }

    fn config_files(&self, install: Install<'_>) -> Vec<PathBuf> {
        let mut files = video_files(install.steam_root);
        files.push(cfg_dir(install.dir).join("framerush.cfg"));
        files
    }

    fn bench_command(&self) -> &'static str {
        "exec fpsbench"
    }

    fn launch_options(&self) -> &'static str {
        "-console +exec framerush -fullscreen"
    }

    fn plan(
        &self,
        install: Install<'_>,
        device: &DeviceProfile,
        options: GameOptions,
    ) -> Result<GamePlan> {
        let mut plan = GamePlan::default();
        if options.tune {
            let videos = video_files(install.steam_root);
            if videos.is_empty() {
                plan.notes.push(
                    "no hay cs2_video.txt: abre CS2 una vez, ciérralo y vuelve a aplicar".into(),
                );
            }
            let settings = video_settings(device);
            for path in videos {
                let text = std::fs::read_to_string(&path).map_err(|source| GameError::Io {
                    path: path.clone(),
                    source,
                })?;
                plan.writes.push(GameFileWrite {
                    content: patch_video(&text, &settings),
                    read_only: options.lock,
                    path,
                });
            }
        }
        let cfg = cfg_dir(install.dir);
        plan.writes.push(GameFileWrite {
            path: cfg.join("framerush.cfg"),
            content: own_cfg(options.fps_cap),
            read_only: false,
        });
        if options.bench {
            plan.writes.push(GameFileWrite {
                path: cfg.join("fpsbench.cfg"),
                content: bench_cfg(),
                read_only: false,
            });
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use framerush_device::samples;

    const VIDEO: &str = "\"video.cfg\"\r\n{\r\n\t\"Version\"\t\t\"16\"\r\n\t\"setting.mat_vsync\"\t\t\"1\"\r\n\t\"setting.mat_vsync_extra\"\t\t\"7\"\r\n}\r\n";

    #[test]
    fn patch_replaces_the_key_adds_missing_ones_and_keeps_the_rest() {
        let out = patch_video(
            VIDEO,
            &[("setting.mat_vsync", "0"), ("setting.r_ssao", "0")],
        );
        assert_eq!(
            out,
            "\"video.cfg\"\r\n{\r\n\t\"Version\"\t\t\"16\"\r\n\t\"setting.mat_vsync\"\t\t\"0\"\r\n\t\"setting.mat_vsync_extra\"\t\t\"7\"\r\n\t\"setting.r_ssao\"\t\t\"0\"\r\n}\r\n"
        );
        // Aplicarlo dos veces da lo mismo.
        assert_eq!(patch_video(&out, &[("setting.r_ssao", "0")]), out);
    }

    #[test]
    fn hardware_settings_follow_the_device() {
        let has = |d: &DeviceProfile, k: &str| video_settings(d).iter().any(|(key, _)| *key == k);
        // Portátil híbrido de muestra: NVIDIA con poca VRAM y 16 GB.
        let laptop = samples::hybrid_laptop();
        let gpu = game_gpu(&laptop).unwrap();
        assert_eq!(
            has(&laptop, "setting.r_low_latency"),
            gpu.vendor == GpuVendor::Nvidia
        );
        assert_eq!(
            has(&laptop, "setting.gpu_mem_level"),
            gpu.dedicated_vram_bytes
                .is_some_and(|v| v <= LOW_TEXTURES_MAX_VRAM)
        );
        assert!(has(&laptop, "setting.mem_level"));
        // Sin datos del equipo solo van los valores fijos.
        let none = DeviceProfile::undetected(framerush_platform::Os::Windows);
        assert_eq!(video_settings(&none).len(), COMPETITIVE.len());
    }

    #[test]
    fn plan_writes_the_tuned_video_of_each_account_and_our_cfgs() {
        let base = std::env::temp_dir().join(format!("framerush-cs2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let steam = base.join("steam");
        let game = base.join("cs2");
        let video = steam.join("userdata/123/730/local/cfg/cs2_video.txt");
        std::fs::create_dir_all(video.parent().unwrap()).unwrap();
        std::fs::write(&video, VIDEO).unwrap();
        let install = Install {
            dir: &game,
            steam_root: &steam,
        };
        let device = samples::hybrid_laptop();

        let off = Cs2.plan(install, &device, GameOptions::default()).unwrap();
        assert_eq!(off.writes.len(), 1, "sin tune ni bench: solo framerush.cfg");
        assert!(off.writes[0].content.contains("fps_max 0"));

        let on = GameOptions {
            tune: true,
            lock: true,
            fps_cap: 240,
            bench: true,
        };
        let p = Cs2.plan(install, &device, on).unwrap();
        assert_eq!(p.writes.len(), 3);
        assert_eq!(p.writes[0].path, video);
        assert!(p.writes[0].read_only);
        assert!(
            p.writes[0]
                .content
                .contains("\"setting.mat_vsync\"\t\t\"0\"")
        );
        assert!(p.writes[1].content.contains("fps_max 240"));
        assert!(p.writes[2].path.ends_with("fpsbench.cfg"));
        assert!(p.notes.is_empty());

        std::fs::remove_dir_all(steam.join("userdata")).unwrap();
        let p = Cs2.plan(install, &device, on).unwrap();
        assert_eq!(p.notes.len(), 1, "sin cs2_video.txt se avisa");
        let _ = std::fs::remove_dir_all(&base);
    }
}
