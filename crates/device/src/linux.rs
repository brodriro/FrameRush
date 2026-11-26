//! `DeviceProfile` en Linux a partir de sysfs y procfs, sin comandos externos (R8).
//!
//! Las rutas cuelgan de una raíz (`/` en el equipo, un árbol de ejemplo en los tests). Lo que
//! no se puede leer queda en `None` o vacío, nunca en 0 (P15). Fuentes:
//! - CPU: modelo de `model name` en `/proc/cpuinfo` (proc_cpuinfo(5): su contenido depende de la
//!   arquitectura; en ARM puede faltar y el modelo queda vacío, como en Windows). CPU lógicas de
//!   `/sys/devices/system/cpu/online` y núcleos de `cpuX/topology/core_cpus_list` (nombre antiguo
//!   `thread_siblings_list`), ambos con formato de lista «0-3, 8-11, 14,17»
//!   (Documentation/admin-guide/cputopology.rst y ABI/stable/sysfs-devices-system-cpu). En CPUs
//!   heterogéneas, `cpuX/cpu_capacity` (ABI/testing/sysfs-devices-system-cpu) más baja que la
//!   máxima son núcleos de eficiencia, como el `EfficiencyClass` de Windows.
//! - GPU: cada `/sys/class/drm/cardN` con un dispositivo PCI. Ids y ranura salen de
//!   `device/uevent` (`PCI_ID`, `PCI_SUBSYS_ID`, `PCI_SLOT_NAME`, que escribe `pci_uevent` en
//!   drivers/pci/pci-driver.c). VRAM solo con amdgpu: `device/mem_info_vram_total`, en bytes
//!   (Documentation/gpu/amdgpu/driver-misc.rst). Fabricante por id PCI (pci-ids: 10de NVIDIA,
//!   1002 AMD/ATI, 8086 Intel).
//! - Batería y CA: `/sys/class/power_supply/*` (ABI/testing/sysfs-class-power): `type`
//!   («Battery», «Mains», «USB»…), `status` («Unknown», «Charging», «Discharging», «Not
//!   charging», «Full») y `online` (0 = desconectado). `scope` «Device» es la batería de un
//!   periférico (ratón, mando), no la del equipo (`POWER_SUPPLY_SCOPE_TEXT` en
//!   drivers/power/supply/power_supply_sysfs.c).
//! - Sistema: fabricante de `/sys/class/dmi/id/sys_vendor` y chasis de `chassis_type`
//!   (drivers/firmware/dmi-id.c); memoria de `MemTotal` en `/proc/meminfo` (proc_meminfo(5),
//!   en kB) y versión del kernel de `/proc/sys/kernel/osrelease` (proc_sys_kernel(5)).
//!
//! Pendiente: monitores y Hz (sysfs no da el modo actual ni las frecuencias: KMS o
//! Wayland/X11), perfiles de energía (power-profiles-daemon por D-Bus) y el nombre comercial de
//! la GPU (pci.ids, cuya ruta depende de la distribución).

use crate::DeviceProfile;
use framerush_platform::{Battery, Core, CoreKind, Cpu, Gpu, GpuVendor, Os, PciId};
use std::fs;
use std::path::Path;

/// Perfil del equipo con lo que exponen sysfs y procfs, y los perfiles de energía de
/// power-profiles-daemon (sin él, ninguno).
#[cfg(target_os = "linux")]
pub fn detect() -> DeviceProfile {
    use framerush_platform::{DisplayControl, LinuxPlatform, PowerControl};
    let mut device = detect_at(Path::new("/"));
    let platform = LinuxPlatform::new();
    device.power_plans = platform.power_plans().unwrap_or_default();
    device.active_power_plan = platform.active_power_plan().ok().map(|p| p.id);
    // Monitores y Hz por DRM/KMS; sin permisos en los nodos, ninguno.
    device.displays = platform.displays().unwrap_or_default();
    device
}

fn detect_at(root: &Path) -> DeviceProfile {
    let mut device = DeviceProfile::undetected(Os::Linux);
    device.os_build = read_trimmed(&root.join("proc/sys/kernel/osrelease"));
    let dmi = root.join("sys/class/dmi/id");
    device.manufacturer = read_trimmed(&dmi.join("sys_vendor"));
    device.laptop = read_trimmed(&dmi.join("chassis_type"))
        .and_then(|t| t.parse().ok())
        .and_then(is_portable_chassis);
    device.memory_bytes = fs::read_to_string(root.join("proc/meminfo"))
        .ok()
        .and_then(|m| mem_total(&m));
    device.cpu = cpu(root);
    device.gpus = gpus(root);
    device.battery = battery(root);
    device
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Si el chasis SMBIOS (`chassis_type`) es portátil; `None` si es «Other», «Unknown» o un
/// código desconocido. Códigos de SMBIOS 7.4.1 tal como los nombra `dmi_chassis_type` en
/// dmidecode.c: 8 Portable, 9 Laptop, 10 Notebook, 14 Sub Notebook, 30 Tablet, 31 Convertible y
/// 32 Detachable son portátiles; 3-7, 11-13 y 15-29, 33-36 no. El bit 7 es el de cerrojo.
fn is_portable_chassis(code: u8) -> Option<bool> {
    match code & 0x7f {
        8 | 9 | 10 | 14 | 30..=32 => Some(true),
        3..=7 | 11..=13 | 15..=29 | 33..=36 => Some(false),
        _ => None,
    }
}

/// `MemTotal` de `/proc/meminfo` en bytes («MemTotal: 16318412 kB»).
fn mem_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line
        .trim_start_matches("MemTotal:")
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse()
        .ok()?;
    kb.checked_mul(1024).filter(|&b| b > 0)
}

/// Lista de CPU del kernel («0-3, 8-11, 14,17») ordenada; `None` si no se entiende.
fn cpu_list(text: &str) -> Option<Vec<usize>> {
    let mut cpus = vec![];
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                if a > b {
                    return None;
                }
                cpus.extend(a..=b);
            }
            None => cpus.push(part.parse().ok()?),
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    (!cpus.is_empty()).then_some(cpus)
}

fn cpu(root: &Path) -> Option<Cpu> {
    let base = root.join("sys/devices/system/cpu");
    let online = cpu_list(&read_trimmed(&base.join("online"))?)?;
    let model = fs::read_to_string(root.join("proc/cpuinfo"))
        .ok()
        .and_then(|info| {
            info.lines()
                .filter_map(|l| l.split_once(':'))
                .find(|(key, _)| key.trim() == "model name")
                .map(|(_, value)| value.trim().to_owned())
        })
        .unwrap_or_default();
    Some(Cpu {
        model,
        cores: cores(&base, &online),
        logical_cpus: online.len(),
        tjmax_c: None,
    })
}

/// Núcleos físicos en orden de CPU lógica. Vacío si falta la topología de alguna CPU.
fn cores(base: &Path, online: &[usize]) -> Vec<Core> {
    let mut siblings = vec![];
    let mut capacities = vec![];
    for cpu in online {
        let dir = base.join(format!("cpu{cpu}"));
        let topology = dir.join("topology");
        let Some(list) = read_trimmed(&topology.join("core_cpus_list"))
            .or_else(|| read_trimmed(&topology.join("thread_siblings_list")))
            .and_then(|l| cpu_list(&l))
        else {
            return vec![];
        };
        siblings.push(list);
        capacities
            .push(read_trimmed(&dir.join("cpu_capacity")).and_then(|c| c.parse::<u64>().ok()));
    }
    // Solo hay núcleos de eficiencia si todas las CPU dan su capacidad y no es la misma: los
    // de capacidad menor que la máxima.
    let known: Option<Vec<u64>> = capacities.into_iter().collect();
    let fastest = known.as_ref().and_then(|c| c.iter().copied().max());

    let mut cores: Vec<(Vec<usize>, Core)> = vec![];
    for (i, (&cpu, list)) in online.iter().zip(siblings).enumerate() {
        match cores.iter_mut().find(|(key, _)| *key == list) {
            Some((_, core)) => core.logical.push(cpu),
            None => {
                let slower = known.as_ref().is_some_and(|c| Some(c[i]) < fastest);
                let kind = if slower {
                    CoreKind::Efficiency
                } else {
                    CoreKind::Performance
                };
                cores.push((
                    list,
                    Core {
                        kind,
                        logical: vec![cpu],
                    },
                ));
            }
        }
    }
    cores.into_iter().map(|(_, core)| core).collect()
}

fn vendor(id: u16) -> GpuVendor {
    match id {
        0x10de => GpuVendor::Nvidia,
        0x1002 => GpuVendor::Amd,
        0x8086 => GpuVendor::Intel,
        _ => GpuVendor::Other,
    }
}

/// «10DE:2484» del uevent.
fn id_pair(text: &str) -> Option<(u16, u16)> {
    let (a, b) = text.split_once(':')?;
    Some((
        u16::from_str_radix(a, 16).ok()?,
        u16::from_str_radix(b, 16).ok()?,
    ))
}

/// Rutas del `pci.ids` de las distribuciones: Arch y Fedora lo traen en `hwdata`, Debian y
/// Ubuntu en `misc` y otras en `/usr/share`. La primera que exista es la que se lee.
const PCI_IDS: [&str; 3] = [
    "usr/share/hwdata/pci.ids",
    "usr/share/misc/pci.ids",
    "usr/share/pci.ids",
];

/// Texto del `pci.ids` del equipo, o `None` si no está instalado (no es obligatorio: sin él la
/// GPU se queda con sus ids).
fn pci_ids(root: &Path) -> Option<String> {
    PCI_IDS
        .iter()
        .find_map(|path| fs::read_to_string(root.join(path)).ok())
}

/// Nombre del dispositivo en el formato de `pci.ids` (pciutils): el fabricante va en una línea
/// sin tabular (`10de  NVIDIA Corporation`), sus dispositivos con un tabulador (`\t1f91  GA107M
/// [GeForce GTX 1650 Mobile]`) y los subsistemas con dos (no cuentan).
fn pci_ids_device(text: &str, vendor: u16, device: u16) -> Option<String> {
    let mut current = None;
    for line in text.lines() {
        match line.strip_prefix('\t') {
            // Dos tabuladores es un subsistema: no se mira.
            Some(rest) if !rest.starts_with('\t') => {
                if current == Some(vendor) {
                    let (id, name) = rest.split_once("  ")?;
                    if u16::from_str_radix(id.trim(), 16).ok() == Some(device) {
                        return Some(name.trim().to_owned()).filter(|n| !n.is_empty());
                    }
                }
            }
            Some(_) => {}
            None => {
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                current = u16::from_str_radix(line.get(..4)?, 16).ok();
            }
        }
    }
    None
}

/// `Model:` de `/proc/driver/nvidia/gpus/<ranura>/information`, la interfaz `/proc` del driver
/// propietario de NVIDIA («Model: the model name of the GPU», NVIDIA Driver README, capítulo de
/// `/proc`): da el nombre comercial tal cual (`NVIDIA GeForce GTX 1650`).
fn nvidia_model(root: &Path, slot: &str) -> Option<String> {
    let path = root
        .join("proc/driver/nvidia/gpus")
        .join(slot)
        .join("information");
    fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Model:"))
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
}

/// GPU de una tarjeta DRM; `None` si su dispositivo no es PCI. `ids` es el texto de `pci.ids`, si
/// el equipo lo tiene.
fn gpu(card: &Path, root: &Path, ids_file: Option<&str>) -> Option<Gpu> {
    let device = card.join("device");
    let uevent = fs::read_to_string(device.join("uevent")).ok()?;
    let field = |key: &str| {
        uevent
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let (vendor_id, device_id) = id_pair(field("PCI_ID")?)?;
    let slot = field("PCI_SLOT_NAME")?.to_owned();
    // Como el `SubSysId` de DXGI: dispositivo en los 16 bits altos y fabricante en los bajos.
    let subsystem = field("PCI_SUBSYS_ID")
        .and_then(id_pair)
        .map_or(0, |(v, d)| (u32::from(d) << 16) | u32::from(v));
    let vendor = vendor(vendor_id);
    let ids = format!("{vendor_id:04x}:{device_id:04x}");
    // Nombre comercial: el del driver de NVIDIA o el del `pci.ids`; si no hay ninguno, los ids
    // con el fabricante (P15: se dice lo que es, no se adivina).
    let label = match vendor {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Apple | GpuVendor::Other => "",
    };
    let name = (vendor == GpuVendor::Nvidia)
        .then(|| nvidia_model(root, &slot))
        .flatten()
        .or_else(|| {
            let device = pci_ids_device(ids_file?, vendor_id, device_id)?;
            Some(match label {
                "" => device,
                label => format!("{label} {device}"),
            })
        })
        .unwrap_or_else(|| match label {
            "" => ids,
            label => format!("{label} {ids}"),
        });
    let vram = (field("DRIVER") == Some("amdgpu"))
        .then(|| {
            read_trimmed(&device.join("mem_info_vram_total"))?
                .parse()
                .ok()
        })
        .flatten();
    Some(Gpu {
        id: slot,
        name,
        vendor,
        dedicated_vram_bytes: vram,
        slowdown_c: None,
        pci: Some(PciId {
            vendor: vendor_id,
            device: device_id,
            subsystem,
        }),
    })
}

/// Tarjetas `cardN` (no los conectores `cardN-HDMI-A-1` ni `renderD*`), por número.
fn gpus(root: &Path) -> Vec<Gpu> {
    let Ok(entries) = fs::read_dir(root.join("sys/class/drm")) else {
        return vec![];
    };
    let mut cards: Vec<(u32, std::path::PathBuf)> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let n = e.file_name().to_str()?.strip_prefix("card")?.parse().ok()?;
            Some((n, e.path()))
        })
        .collect();
    cards.sort();
    let ids_file = pci_ids(root);
    let mut gpus: Vec<Gpu> = vec![];
    for (_, card) in cards {
        // Dos tarjetas DRM pueden ser el mismo dispositivo PCI.
        if let Some(g) =
            gpu(&card, root, ids_file.as_deref()).filter(|g| gpus.iter().all(|o| o.id != g.id))
        {
            gpus.push(g);
        }
    }
    gpus
}

/// Batería del equipo: descargándose si alguna lo dice; con estado «Unknown» o sin él, según
/// haya o no corriente externa. `None` sin batería o sin forma de saberlo.
fn battery(root: &Path) -> Option<Battery> {
    let entries = fs::read_dir(root.join("sys/class/power_supply")).ok()?;
    let mut supplies: Vec<_> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
    supplies.sort();
    let mut statuses = vec![];
    let mut external: Option<bool> = None;
    for supply in supplies {
        if read_trimmed(&supply.join("scope")).as_deref() == Some("Device") {
            continue;
        }
        match read_trimmed(&supply.join("type")).as_deref() {
            Some("Battery") => statuses.push(read_trimmed(&supply.join("status"))),
            Some("Mains" | "USB") => {
                if let Some(online) = read_trimmed(&supply.join("online")) {
                    external = Some(external.unwrap_or(false) || online != "0");
                }
            }
            _ => {}
        }
    }
    if statuses.is_empty() {
        return None;
    }
    if statuses.iter().any(|s| s.as_deref() == Some("Discharging")) {
        return Some(Battery { discharging: true });
    }
    let known =
        |s: &Option<String>| matches!(s.as_deref(), Some("Charging" | "Full" | "Not charging"));
    if statuses.iter().all(known) {
        return Some(Battery { discharging: false });
    }
    external.map(|on| Battery { discharging: !on })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Árbol de ejemplo que se borra al terminar.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("framerush-device-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn write(&self, file: &str, value: &str) -> &Self {
            let path = self.0.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{value}\n")).unwrap();
            self
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn cpu_topology(t: &Tree, cpu: usize, siblings: &str, capacity: Option<u32>) {
        let dir = format!("sys/devices/system/cpu/cpu{cpu}");
        t.write(&format!("{dir}/topology/core_cpus_list"), siblings);
        if let Some(c) = capacity {
            t.write(&format!("{dir}/cpu_capacity"), &c.to_string());
        }
    }

    #[test]
    fn an_empty_tree_detects_nothing() {
        let t = Tree::new("empty");
        assert_eq!(detect_at(&t.0), DeviceProfile::undetected(Os::Linux));
    }

    #[test]
    fn reads_system_data_from_dmi_and_procfs() {
        let t = Tree::new("system");
        t.write("proc/sys/kernel/osrelease", "6.8.0-45-generic")
            .write("sys/class/dmi/id/sys_vendor", "Acme Inc.")
            .write("sys/class/dmi/id/chassis_type", "10")
            .write("proc/meminfo", "MemTotal:       16318412 kB\nMemFree: 1 kB");
        let d = detect_at(&t.0);
        assert_eq!(d.os, Os::Linux);
        assert_eq!(d.os_build.as_deref(), Some("6.8.0-45-generic"));
        assert_eq!(d.windows_11, None);
        assert_eq!(d.manufacturer.as_deref(), Some("Acme Inc."));
        assert_eq!(d.laptop, Some(true));
        assert_eq!(d.memory_bytes, Some(16_318_412 * 1024));
    }

    #[test]
    fn chassis_and_meminfo_edge_cases() {
        assert_eq!(is_portable_chassis(3), Some(false));
        assert_eq!(is_portable_chassis(9 | 0x80), Some(true));
        assert_eq!(is_portable_chassis(2), None);
        assert_eq!(is_portable_chassis(0x50), None);
        assert_eq!(mem_total("MemTotal: 0 kB"), None);
        assert_eq!(mem_total("MemTotal: raro"), None);
        assert_eq!(mem_total(""), None);
    }

    #[test]
    fn parses_kernel_cpu_lists() {
        assert_eq!(
            cpu_list("0-3, 8-9, 14,17"),
            Some(vec![0, 1, 2, 3, 8, 9, 14, 17])
        );
        assert_eq!(cpu_list("5"), Some(vec![5]));
        assert_eq!(cpu_list(""), None);
        assert_eq!(cpu_list("3-1"), None);
        assert_eq!(cpu_list("a"), None);
    }

    #[test]
    fn groups_smt_threads_into_cores_and_marks_efficiency_cores() {
        let t = Tree::new("hybrid");
        t.write("sys/devices/system/cpu/online", "0-5").write(
            "proc/cpuinfo",
            "processor\t: 0\nmodel name\t: Acme Core 9\n",
        );
        // Dos P-cores con SMT (0,1 y 2,3) y dos E-cores (4 y 5).
        cpu_topology(&t, 0, "0-1", Some(1024));
        cpu_topology(&t, 1, "0-1", Some(1024));
        cpu_topology(&t, 2, "2-3", Some(1024));
        cpu_topology(&t, 3, "2-3", Some(1024));
        cpu_topology(&t, 4, "4", Some(512));
        cpu_topology(&t, 5, "5", Some(512));
        let cpu = detect_at(&t.0).cpu.unwrap();
        assert_eq!(cpu.model, "Acme Core 9");
        assert_eq!(cpu.logical_cpus, 6);
        let got: Vec<(CoreKind, Vec<usize>)> =
            cpu.cores.into_iter().map(|c| (c.kind, c.logical)).collect();
        assert_eq!(
            got,
            [
                (CoreKind::Performance, vec![0, 1]),
                (CoreKind::Performance, vec![2, 3]),
                (CoreKind::Efficiency, vec![4]),
                (CoreKind::Efficiency, vec![5]),
            ]
        );
    }

    #[test]
    fn without_capacities_all_cores_are_performance_and_missing_topology_leaves_none() {
        let t = Tree::new("smp");
        t.write("sys/devices/system/cpu/online", "0-1");
        cpu_topology(&t, 0, "0", None);
        t.write(
            "sys/devices/system/cpu/cpu1/topology/thread_siblings_list",
            "1",
        );
        let cpu = detect_at(&t.0).cpu.unwrap();
        // Sin `model name` (p. ej. ARM) el modelo queda vacío, como en Windows.
        assert_eq!(cpu.model, "");
        assert_eq!(cpu.physical_cores(), Some(2));
        assert!(cpu.cores.iter().all(|c| c.kind == CoreKind::Performance));

        // Una CPU sin topología: no se sabe cuántos núcleos hay.
        t.write("sys/devices/system/cpu/online", "0-2");
        let cpu = detect_at(&t.0).cpu.unwrap();
        assert_eq!(cpu.logical_cpus, 3);
        assert_eq!(cpu.physical_cores(), None);
    }

    #[test]
    fn reads_the_device_name_from_pci_ids() {
        let text = "10de  NVIDIA Corporation\n\t1f91  GA107M [GeForce GTX 1650 Mobile]\n\t\t17aa 3fc1  portátil\n121a  3dfx\n";
        assert_eq!(
            pci_ids_device(text, 0x10de, 0x1f91).as_deref(),
            Some("GA107M [GeForce GTX 1650 Mobile]")
        );
        // Otro dispositivo, otro fabricante o un archivo vacío: nada.
        assert_eq!(pci_ids_device(text, 0x10de, 0x1f90), None);
        assert_eq!(pci_ids_device(text, 0x121a, 0x1f91), None);
        assert_eq!(pci_ids_device("", 0x10de, 0x1f91), None);
    }

    #[test]
    fn names_gpus_from_pci_ids_and_the_nvidia_driver() {
        let t = Tree::new("gpu-names");
        t.write(
            "usr/share/hwdata/pci.ids",
            "8086  Intel Corporation\n\t3e9b  CoffeeLake-H GT2 [UHD Graphics 630]\n\
             10de  NVIDIA Corporation\n\t1f91  GA107M [GeForce GTX 1650 Mobile]\n",
        )
        .write(
            "sys/class/drm/card0/device/uevent",
            "PCI_ID=8086:3E9B\nPCI_SLOT_NAME=0000:00:02.0",
        );
        let gpus = detect_at(&t.0).gpus;
        assert_eq!(gpus[0].name, "Intel CoffeeLake-H GT2 [UHD Graphics 630]");

        // El driver de NVIDIA da el nombre comercial y gana al `pci.ids`.
        t.write(
            "sys/class/drm/card1/device/uevent",
            "PCI_ID=10DE:1F91\nPCI_SLOT_NAME=0000:01:00.0",
        )
        .write(
            "proc/driver/nvidia/gpus/0000:01:00.0/information",
            "Model:\t\t NVIDIA GeForce GTX 1650\nIRQ:\t\t 132",
        );
        let gpus = detect_at(&t.0).gpus;
        assert_eq!(gpus[1].name, "NVIDIA GeForce GTX 1650");
        assert_eq!(gpus[1].id, "0000:01:00.0");
    }

    #[test]
    fn reads_pci_gpus_from_drm_cards() {
        let t = Tree::new("drm");
        t.write(
            "sys/class/drm/card1/device/uevent",
            "DRIVER=amdgpu\nPCI_CLASS=30000\nPCI_ID=1002:73BF\nPCI_SUBSYS_ID=1DA2:E445\n\
             PCI_SLOT_NAME=0000:03:00.0",
        )
        .write(
            "sys/class/drm/card1/device/mem_info_vram_total",
            "17163091968",
        )
        .write(
            "sys/class/drm/card0/device/uevent",
            "DRIVER=i915\nPCI_ID=8086:A780\nPCI_SUBSYS_ID=1043:8882\nPCI_SLOT_NAME=0000:00:02.0",
        )
        // Conector y nodo de render: no son tarjetas.
        .write("sys/class/drm/card1-HDMI-A-1/status", "connected")
        .write(
            "sys/class/drm/renderD128/device/uevent",
            "PCI_ID=8086:A780\nPCI_SLOT_NAME=0000:00:02.0",
        )
        // Framebuffer del firmware sin PCI (simpledrm): no es una GPU.
        .write(
            "sys/class/drm/card2/device/uevent",
            "DRIVER=simple-framebuffer",
        );
        let gpus = detect_at(&t.0).gpus;
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].id, "0000:00:02.0");
        assert_eq!(gpus[0].vendor, GpuVendor::Intel);
        assert_eq!(gpus[0].name, "Intel 8086:a780");
        assert_eq!(gpus[0].dedicated_vram_bytes, None);
        assert_eq!(gpus[1].vendor, GpuVendor::Amd);
        assert_eq!(gpus[1].dedicated_vram_bytes, Some(17_163_091_968));
        assert_eq!(
            gpus[1].pci,
            Some(PciId {
                vendor: 0x1002,
                device: 0x73bf,
                subsystem: 0xe445_1da2,
            })
        );
    }

    fn supply(t: &Tree, name: &str, kind: &str, fields: &[(&str, &str)]) {
        t.write(&format!("sys/class/power_supply/{name}/type"), kind);
        for (file, value) in fields {
            t.write(&format!("sys/class/power_supply/{name}/{file}"), value);
        }
    }

    #[test]
    fn battery_status_and_mains() {
        let t = Tree::new("power");
        assert_eq!(detect_at(&t.0).battery, None);

        // Batería de un ratón y un cargador: el equipo no tiene batería.
        supply(
            &t,
            "hid-mouse",
            "Battery",
            &[("scope", "Device"), ("status", "Discharging")],
        );
        supply(&t, "AC", "Mains", &[("online", "1")]);
        assert_eq!(detect_at(&t.0).battery, None);

        supply(&t, "BAT0", "Battery", &[("status", "Charging")]);
        assert_eq!(
            detect_at(&t.0).battery,
            Some(Battery { discharging: false })
        );

        supply(&t, "BAT0", "Battery", &[("status", "Discharging")]);
        assert_eq!(detect_at(&t.0).battery, Some(Battery { discharging: true }));

        // Estado desconocido: decide la corriente externa.
        supply(&t, "BAT0", "Battery", &[("status", "Unknown")]);
        assert_eq!(
            detect_at(&t.0).battery,
            Some(Battery { discharging: false })
        );
        supply(&t, "AC", "Mains", &[("online", "0")]);
        assert_eq!(detect_at(&t.0).battery, Some(Battery { discharging: true }));
        fs::remove_dir_all(t.0.join("sys/class/power_supply/AC")).unwrap();
        assert_eq!(detect_at(&t.0).battery, None);
    }
}
