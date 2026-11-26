//! El resto de [`Platform`](crate::Platform) en Linux: servicios (systemd), energía
//! (power-profiles-daemon), archivos del juego y lo que en Linux no existe o aún no está.

use super::{LinuxPlatform, power, services};
use crate::{
    Battery, Capabilities, CapabilitySet, Cpu, DefenderExclusion, Display, DisplayControl,
    GameFiles, Gpu, Inventory, MemoryControl, PlatformError, PowerControl, PowerPlan, Result,
    SavedValue, SecurityTweaks, ServiceControl, ServiceState, SystemTweaks, Target, Tweak,
};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

impl ServiceControl for LinuxPlatform {
    fn service_state(&self, name: &str) -> Result<Option<ServiceState>> {
        services::service_state(name)
    }

    fn stop_service(&self, name: &str) -> Result<()> {
        services::stop_service(name)
    }

    fn start_service(&self, name: &str) -> Result<()> {
        services::start_service(name)
    }
}

impl PowerControl for LinuxPlatform {
    fn power_plans(&self) -> Result<Vec<PowerPlan>> {
        power::power_plans()
    }

    fn active_power_plan(&self) -> Result<PowerPlan> {
        power::active_power_plan()
    }

    fn set_active_power_plan(&self, id: &str) -> Result<()> {
        power::set_active_power_plan(id)
    }

    fn create_custom_power_plan(&self, _: &str, _: &str, _: bool) -> Result<PowerPlan> {
        Err(PlatformError::Unsupported("plan de energía propio"))
    }

    fn delete_power_plan(&self, _id: &str) -> Result<()> {
        Err(PlatformError::Unsupported("plan de energía propio"))
    }
}

/// sysfs no da el modo actual de los monitores; el compositor sí, pero aún no está.
impl DisplayControl for LinuxPlatform {
    fn displays(&self) -> Result<Vec<Display>> {
        Err(PlatformError::Unsupported("monitores"))
    }

    fn set_refresh_rate(&self, _id: &str, _hz: u32) -> Result<()> {
        Err(PlatformError::Unsupported("frecuencia de refresco"))
    }
}

/// El inventario de Linux lo lee `framerush_device::linux` (sysfs y procfs). Lo que es de
/// Windows (VBS, ESS, HAGS, Defender, Security Center) no existe aquí.
impl Inventory for LinuxPlatform {
    fn cpu(&self) -> Result<Option<Cpu>> {
        Err(PlatformError::Unsupported(
            "CPU por Inventory (ver framerush_device::linux)",
        ))
    }

    fn gpus(&self) -> Result<Vec<Gpu>> {
        Err(PlatformError::Unsupported(
            "GPU por Inventory (ver framerush_device::linux)",
        ))
    }

    fn battery(&self) -> Result<Option<Battery>> {
        Err(PlatformError::Unsupported(
            "batería por Inventory (ver framerush_device::linux)",
        ))
    }

    fn os_build(&self) -> Result<Option<String>> {
        Err(PlatformError::Unsupported(
            "versión por Inventory (ver framerush_device::linux)",
        ))
    }

    fn manufacturer(&self) -> Result<Option<String>> {
        Err(PlatformError::Unsupported(
            "fabricante por Inventory (ver framerush_device::linux)",
        ))
    }

    fn laptop(&self) -> Result<Option<bool>> {
        Err(PlatformError::Unsupported(
            "chasis por Inventory (ver framerush_device::linux)",
        ))
    }

    fn total_memory(&self) -> Result<Option<u64>> {
        Err(PlatformError::Unsupported(
            "memoria por Inventory (ver framerush_device::linux)",
        ))
    }

    fn vbs_running(&self) -> Result<Option<bool>> {
        Ok(None)
    }

    fn hello_ess_enabled(&self) -> Result<Option<bool>> {
        Ok(None)
    }

    fn installed_apps(&self) -> Result<Vec<String>> {
        Err(PlatformError::Unsupported("apps instaladas"))
    }

    fn hags_supported(&self) -> Result<Option<bool>> {
        Ok(None)
    }

    fn hypervisor_features(&self) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn antivirus(&self) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn defender_active(&self) -> Result<Option<bool>> {
        Ok(None)
    }
}

fn file_error(path: &Path, e: std::io::Error) -> PlatformError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            PlatformError::AccessDenied(path.display().to_string())
        }
        _ => PlatformError::Os(format!("{}: {e}", path.display())),
    }
}

/// Permiso de escritura del dueño (`S_IWUSR`, 0o200): sin él, es de solo lectura para el juego.
const OWNER_WRITE: u32 = 0o200;

fn read_only(mode: u32) -> bool {
    mode & OWNER_WRITE == 0
}

/// Solo cambia la escritura del dueño: quitar el solo lectura no da escritura al grupo ni a
/// otros (lo que haría `Permissions::set_readonly(false)` en Unix).
fn with_read_only(mode: u32, read_only: bool) -> u32 {
    if read_only {
        mode & !0o222
    } else {
        mode | OWNER_WRITE
    }
}

fn set_read_only(path: &Path, ro: bool) -> Result<()> {
    let mut perms = std::fs::metadata(path)
        .map_err(|e| file_error(path, e))?
        .permissions();
    let mode = perms.mode();
    if read_only(mode) != ro {
        perms.set_mode(with_read_only(mode, ro));
        std::fs::set_permissions(path, perms).map_err(|e| file_error(path, e))?;
    }
    Ok(())
}

impl GameFiles for LinuxPlatform {
    fn read_game_file(&self, path: &Path) -> Result<Option<(String, bool)>> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(file_error(path, e)),
        };
        let mode = std::fs::metadata(path)
            .map_err(|e| file_error(path, e))?
            .permissions()
            .mode();
        Ok(Some((content, read_only(mode))))
    }

    fn write_game_file(&self, path: &Path, content: &str, ro: bool) -> Result<()> {
        if path.exists() {
            set_read_only(path, false)?;
        } else if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| file_error(dir, e))?;
        }
        std::fs::write(path, content).map_err(|e| file_error(path, e))?;
        if ro {
            set_read_only(path, true)?;
        }
        Ok(())
    }

    /// En Unix borrar depende de la carpeta, no del modo del archivo.
    fn remove_game_file(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(file_error(path, e)),
        }
    }
}

/// Los ajustes persistentes de Linux (`gamemode`, M5) aún no están.
impl SystemTweaks for LinuxPlatform {
    fn supported_tweaks(&self) -> Vec<Tweak> {
        vec![]
    }

    fn tweak_requires_admin(&self, _tweak: Tweak) -> bool {
        false
    }

    fn read_tweak(&self, _tweak: Tweak) -> Result<Vec<SavedValue>> {
        Err(PlatformError::Unsupported("ajustes de sistema"))
    }

    fn apply_tweak(&self, _tweak: Tweak) -> Result<()> {
        Err(PlatformError::Unsupported("ajustes de sistema"))
    }

    fn restore_tweak(&self, _tweak: Tweak, _saved: &[SavedValue]) -> Result<()> {
        Err(PlatformError::Unsupported("ajustes de sistema"))
    }
}

/// `memory-trim` y `standby-purge` no tienen equivalente definido en Linux (PLAN.md §3).
impl MemoryControl for LinuxPlatform {
    fn trim_working_set(&self, _target: &Target) -> Result<()> {
        Err(PlatformError::Unsupported("memory-trim"))
    }

    fn purge_standby_list(&self) -> Result<()> {
        Err(PlatformError::Unsupported("standby-purge"))
    }
}

/// Microsoft Defender es de Windows.
impl SecurityTweaks for LinuxPlatform {
    fn has_defender_exclusion(&self, _exclusion: &DefenderExclusion) -> Result<bool> {
        Err(PlatformError::Unsupported("exclusiones de Defender"))
    }

    fn add_defender_exclusion(&self, _exclusion: &DefenderExclusion) -> Result<()> {
        Err(PlatformError::Unsupported("exclusiones de Defender"))
    }

    fn remove_defender_exclusion(&self, _exclusion: &DefenderExclusion) -> Result<()> {
        Err(PlatformError::Unsupported("exclusiones de Defender"))
    }
}

impl Capabilities for LinuxPlatform {
    /// Sin el helper, lo que pide privilegios (bajar el `nice`, servicios) da acceso denegado
    /// y la app lo avisa.
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            set_priority: true,
            suspend: true,
            kill: true,
            services: true,
            power_plans: true,
            affinity: true,
            ..CapabilitySet::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El motor necesita `Platform` entero.
    #[test]
    fn linux_is_a_full_platform() {
        fn platform(p: &impl crate::Platform) -> CapabilitySet {
            p.capabilities()
        }
        let caps = platform(&LinuxPlatform::new());
        assert!(caps.services && caps.power_plans && !caps.custom_power_plan);
    }

    #[test]
    fn read_only_is_the_owner_write_bit_and_lifting_it_writes_only_for_the_owner() {
        assert!(!read_only(0o644));
        assert!(read_only(0o444));
        assert_eq!(with_read_only(0o664, true), 0o444);
        assert_eq!(with_read_only(0o444, false), 0o644);
    }

    #[test]
    fn game_files_round_trip_with_the_read_only_lock() {
        let dir = std::env::temp_dir().join(format!("framerush-gamefiles-{}", std::process::id()));
        let path = dir.join("sub").join("video.txt");
        let p = LinuxPlatform::new();
        assert_eq!(p.read_game_file(&path).unwrap(), None);
        p.write_game_file(&path, "a", true).unwrap();
        assert_eq!(p.read_game_file(&path).unwrap(), Some(("a".into(), true)));
        p.write_game_file(&path, "b", false).unwrap();
        assert_eq!(p.read_game_file(&path).unwrap(), Some(("b".into(), false)));
        p.remove_game_file(&path).unwrap();
        p.remove_game_file(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
