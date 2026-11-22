//! Inventario del hardware para el `DeviceProfile` (R8).
//!
//! - CPU: topología con los CPU Sets (`GetSystemCpuSetInformation`): un núcleo físico es un
//!   `(Group, CoreIndex)`, y en CPUs híbridas el `EfficiencyClass` más alto son los P-cores
//!   (Microsoft Learn, `SYSTEM_CPU_SET_INFORMATION`). El modelo lo da `sysinfo`.
//! - GPU: DXGI (`CreateDXGIFactory1` + `EnumAdapters1` + `GetDesc1`), sin los adaptadores de
//!   software (`DXGI_ADAPTER_FLAG_SOFTWARE`). El fabricante sale del id PCI del vendedor
//!   (pci-ids: 10de NVIDIA, 1002 AMD/ATI, 8086 Intel).
//! - Batería: `GetSystemPowerStatus` (`BatteryFlag` 128 = sin batería, 255 = desconocido;
//!   `ACLineStatus` 0 = desenchufado).
#![allow(unsafe_code)]

use super::cpus::{CpuSet, system_cpu_sets};
use crate::{Battery, Core, CoreKind, Cpu, Gpu, GpuVendor, PlatformError, Result};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

/// Núcleos físicos en orden de CPU lógica, con su tipo.
fn cores(sets: &[CpuSet]) -> Vec<Core> {
    let hybrid = sets
        .iter()
        .any(|s| s.efficiency_class != sets[0].efficiency_class);
    let fastest = sets.iter().map(|s| s.efficiency_class).max().unwrap_or(0);
    let mut cores: Vec<((u16, u8), Core)> = vec![];
    // `sets` está ordenado por (grupo, índice): la posición es el número global de la CPU.
    for (cpu, s) in sets.iter().enumerate() {
        let key = (s.group, s.core);
        match cores.iter_mut().find(|(k, _)| *k == key) {
            Some((_, core)) => core.logical.push(cpu),
            None => {
                let kind = if hybrid && s.efficiency_class < fastest {
                    CoreKind::Efficiency
                } else {
                    CoreKind::Performance
                };
                cores.push((
                    key,
                    Core {
                        kind,
                        logical: vec![cpu],
                    },
                ));
            }
        }
    }
    cores.into_iter().map(|(_, c)| c).collect()
}

pub(super) fn cpu() -> Result<Option<Cpu>> {
    let sets = system_cpu_sets()?;
    if sets.is_empty() {
        return Ok(None);
    }
    let mut system = sysinfo::System::new();
    system.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
    let model = system
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_owned())
        .filter(|b| !b.is_empty())
        .unwrap_or_default();
    Ok(Some(Cpu {
        model,
        logical_cpus: sets.len(),
        cores: cores(&sets),
        tjmax_c: None,
    }))
}

fn vendor(id: u32) -> GpuVendor {
    match id {
        0x10de => GpuVendor::Nvidia,
        0x1002 => GpuVendor::Amd,
        0x8086 => GpuVendor::Intel,
        _ => GpuVendor::Other,
    }
}

pub(super) fn gpus() -> Result<Vec<Gpu>> {
    let os = |e: windows::core::Error| PlatformError::Os(format!("DXGI: {e}"));
    // SAFETY: llamada COM sin argumentos de puntero; el crate `windows` gestiona la referencia.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(os)?;
    let mut gpus = vec![];
    for index in 0.. {
        // SAFETY: `factory` es una interfaz válida; un índice fuera de rango da NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(a) => a,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(os(e)),
        };
        // SAFETY: `adapter` es una interfaz válida que devolvió EnumAdapters1.
        let desc = unsafe { adapter.GetDesc1() }.map_err(os)?;
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let luid = desc.AdapterLuid;
        gpus.push(Gpu {
            id: format!("luid:{:08x}{:08x}", luid.HighPart, luid.LowPart),
            name: String::from_utf16_lossy(&desc.Description[..len]),
            vendor: vendor(desc.VendorId),
            dedicated_vram_bytes: Some(desc.DedicatedVideoMemory as u64),
            slowdown_c: None,
        });
    }
    Ok(gpus)
}

pub(super) fn battery() -> Result<Option<Battery>> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` vive durante la llamada.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return Err(PlatformError::Os(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(match status.BatteryFlag {
        128 | 255 => None,
        _ => Some(Battery {
            discharging: status.ACLineStatus == 0,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(i: u8, core: u8, efficiency_class: u8) -> CpuSet {
        CpuSet {
            id: 256 + u32::from(i),
            group: 0,
            index: i,
            core,
            efficiency_class,
        }
    }

    #[test]
    fn smt_threads_share_a_core_and_hybrids_have_e_cores() {
        // 2 P-cores con SMT y 2 E-cores sin SMT.
        let sets = [
            set(0, 0, 1),
            set(1, 0, 1),
            set(2, 1, 1),
            set(3, 1, 1),
            set(4, 2, 0),
            set(5, 3, 0),
        ];
        let c = cores(&sets);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].logical, [0, 1]);
        assert_eq!(c[0].kind, CoreKind::Performance);
        assert_eq!(c[3].logical, [5]);
        assert_eq!(c[3].kind, CoreKind::Efficiency);
        // Sin híbrido, todos son P.
        let flat = [set(0, 0, 0), set(1, 1, 0)];
        assert!(cores(&flat).iter().all(|c| c.kind == CoreKind::Performance));
    }

    /// Solo lee: la topología cubre todas las CPU lógicas una vez.
    #[test]
    fn this_machine_has_a_cpu_and_reads_gpus_and_battery() {
        let cpu = cpu().unwrap().unwrap();
        let mut all: Vec<usize> = cpu.cores.iter().flat_map(|c| c.logical.clone()).collect();
        all.sort_unstable();
        assert_eq!(all, (0..cpu.logical_cpus).collect::<Vec<_>>());
        assert!(!cpu.model.is_empty());
        for g in gpus().unwrap() {
            assert!(!g.name.is_empty(), "{g:?}");
        }
        battery().unwrap();
    }
}
