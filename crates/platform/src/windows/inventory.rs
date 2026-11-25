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
//! - Sistema (WMI, sin admin): build de `Win32_OperatingSystem.BuildNumber`, fabricante de
//!   `Win32_ComputerSystem.Manufacturer` y VBS de `Win32_DeviceGuard`
//!   (`VirtualizationBasedSecurityStatus` 2 = en ejecución).
//! - Apps: `DisplayName` de las subclaves de `...\CurrentVersion\Uninstall` en HKLM (64 y 32
//!   bits) y HKCU (Microsoft Learn, "Uninstall Registry Key"). Las apps MSIX no salen ahí.
//! - HAGS: `D3DKMTQueryAdapterInfo(KMTQAITYPE_WDDM_2_7_CAPS)` por cada adaptador DXGI; el bit 0
//!   es `HwSchSupported` (Windows SDK 10.0.26100, `shared/d3dkmdt.h`).
//! - Hipervisor (V6): `Win32_OptionalFeature` con `InstallState` 1 (Enabled, Microsoft Learn).
//! - Antivirus: nombres de la API de Security Center (`IWSCProductList::Initialize` con
//!   `WSC_SECURITY_PROVIDER_ANTIVIRUS` + `IWscProduct::get_ProductName`; Windows 8+, solo
//!   ediciones cliente). El estado no se lee.
#![allow(unsafe_code)]

use super::cpus::{CpuSet, system_cpu_sets};
use super::registry::{self, Hive};
use super::wmi;
use crate::{Battery, Core, CoreKind, Cpu, Gpu, GpuVendor, PciId, PlatformError, Result};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
    IDXGIFactory1,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::SecurityCenter::{
    IWSCProductList, WSC_SECURITY_PROVIDER_ANTIVIRUS, WSCProductList,
};
use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, D3DKMT_WDDM_2_7_CAPS,
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo,
    KMTQAITYPE_WDDM_2_7_CAPS,
};
use windows_sys::Win32::Foundation::LUID;
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

/// Adaptadores DXGI de hardware (sin los de software).
fn adapters() -> Result<Vec<DXGI_ADAPTER_DESC1>> {
    let os = |e: windows::core::Error| PlatformError::Os(format!("DXGI: {e}"));
    // SAFETY: llamada COM sin argumentos de puntero; el crate `windows` gestiona la referencia.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(os)?;
    let mut adapters = vec![];
    for index in 0.. {
        // SAFETY: `factory` es una interfaz válida; un índice fuera de rango da NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(a) => a,
            Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(e) => return Err(os(e)),
        };
        // SAFETY: `adapter` es una interfaz válida que devolvió EnumAdapters1.
        let desc = unsafe { adapter.GetDesc1() }.map_err(os)?;
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
            adapters.push(desc);
        }
    }
    Ok(adapters)
}

/// Id PCI del adaptador; `None` si fabricante o dispositivo no caben en 16 bits.
fn pci_id(desc: &DXGI_ADAPTER_DESC1) -> Option<PciId> {
    Some(PciId {
        vendor: u16::try_from(desc.VendorId).ok()?,
        device: u16::try_from(desc.DeviceId).ok()?,
        subsystem: desc.SubSysId,
    })
}

pub(super) fn gpus() -> Result<Vec<Gpu>> {
    let mut gpus = vec![];
    for desc in adapters()? {
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
            pci: pci_id(&desc),
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

/// `HwSchSupported` de un adaptador, o `None` si el driver no responde a la consulta (p. ej.
/// un driver anterior a WDDM 2.7).
fn adapter_hags(luid: windows::Win32::Foundation::LUID) -> Option<bool> {
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: LUID {
            LowPart: luid.LowPart,
            HighPart: luid.HighPart,
        },
        hAdapter: 0,
    };
    // SAFETY: `open` es un local vivo con el LUID que dio DXGI.
    if unsafe { D3DKMTOpenAdapterFromLuid(&mut open) } < 0 {
        return None;
    }
    let mut caps = D3DKMT_WDDM_2_7_CAPS::default();
    let mut query = D3DKMT_QUERYADAPTERINFO {
        hAdapter: open.hAdapter,
        Type: KMTQAITYPE_WDDM_2_7_CAPS,
        pPrivateDriverData: (&raw mut caps).cast(),
        PrivateDriverDataSize: size_of::<D3DKMT_WDDM_2_7_CAPS>() as u32,
    };
    // SAFETY: `query` apunta a `caps`, del tamaño indicado, y el adaptador está abierto.
    let status = unsafe { D3DKMTQueryAdapterInfo(&mut query) };
    // SAFETY: cierra el adaptador abierto arriba.
    unsafe {
        D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
            hAdapter: open.hAdapter,
        })
    };
    // SAFETY: la unión es un u32 entero; `Value` siempre es válido.
    let value = unsafe { caps.Anonymous.Value };
    (status >= 0).then_some(value & HW_SCH_SUPPORTED != 0)
}

/// Bit 0 de `D3DKMT_WDDM_2_7_CAPS` (Windows SDK 10.0.26100, `shared/d3dkmdt.h`).
const HW_SCH_SUPPORTED: u32 = 1;

/// Si alguna GPU admite HAGS; `None` si ningún driver lo dice.
pub(super) fn hags_supported() -> Result<Option<bool>> {
    let answers: Vec<bool> = adapters()?
        .into_iter()
        .filter_map(|d| adapter_hags(d.AdapterLuid))
        .collect();
    Ok((!answers.is_empty()).then(|| answers.contains(&true)))
}

/// Características opcionales de Windows que dependen del hipervisor (V6), con su nombre de
/// DISM: Hyper-V, Windows Sandbox, WSL, WSL2 y WHPX.
const HYPERVISOR_FEATURES: [&str; 5] = [
    "Microsoft-Hyper-V",
    "Containers-DisposableClientVM",
    "Microsoft-Windows-Subsystem-Linux",
    "VirtualMachinePlatform",
    "HypervisorPlatform",
];

/// Las de [`HYPERVISOR_FEATURES`] activadas (`Win32_OptionalFeature.InstallState` 1).
pub(super) fn hypervisor_features() -> Result<Vec<String>> {
    let names = HYPERVISOR_FEATURES
        .iter()
        .map(|n| format!("Name='{n}'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let query =
        format!("SELECT Name FROM Win32_OptionalFeature WHERE InstallState=1 AND ({names})");
    let mut features: Vec<String> = wmi::rows(CIMV2, &query, |o| wmi::string(o, "Name"))?
        .into_iter()
        .flatten()
        .collect();
    features.sort();
    Ok(features)
}

/// Nombres de los antivirus registrados en Security Center, ordenados y sin repetir. Falla en
/// Windows Server, que no tiene esta API.
pub(super) fn antivirus() -> Result<Vec<String>> {
    let os = |e: windows::core::Error| PlatformError::Os(format!("Security Center: {e}"));
    let _com = wmi::Com::init()?;
    // SAFETY: COM está inicializado en este hilo (`_com`); el crate `windows` gestiona las
    // referencias de las interfaces.
    let list: IWSCProductList =
        unsafe { CoCreateInstance(&WSCProductList, None, CLSCTX_INPROC_SERVER) }.map_err(os)?;
    // SAFETY: `list` es válido e `Initialize` se llama una sola vez (documentado).
    unsafe { list.Initialize(WSC_SECURITY_PROVIDER_ANTIVIRUS) }.map_err(os)?;
    // SAFETY: `list` ya está inicializado.
    let count = unsafe { list.Count() }.map_err(os)?;
    let mut names = vec![];
    for index in 0..u32::try_from(count).unwrap_or(0) {
        // SAFETY: `index` está dentro de `Count`.
        let product = unsafe { list.get_Item(index) }.map_err(os)?;
        // SAFETY: `product` es una interfaz válida que devolvió `get_Item`.
        let name = unsafe { product.ProductName() }.map_err(os)?.to_string();
        if !name.trim().is_empty() {
            names.push(name.trim().to_owned());
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

const CIMV2: &str = r"ROOT\CIMV2";

/// Número de build del SO (p. ej. "26100").
pub(super) fn os_build() -> Result<Option<String>> {
    wmi::first(
        CIMV2,
        "SELECT BuildNumber FROM Win32_OperatingSystem",
        |o| wmi::string(o, "BuildNumber"),
    )
    .map(Option::flatten)
}

pub(super) fn manufacturer() -> Result<Option<String>> {
    wmi::first(
        CIMV2,
        "SELECT Manufacturer FROM Win32_ComputerSystem",
        |o| wmi::string(o, "Manufacturer"),
    )
    .map(Option::flatten)
}

/// Valores de `Win32_ComputerSystem.PCSystemType` (Microsoft Learn): 0 = Unspecified,
/// 2 = Mobile (portátil).
const PC_SYSTEM_TYPE_UNSPECIFIED: u32 = 0;
const PC_SYSTEM_TYPE_MOBILE: u32 = 2;

/// Si es un portátil según `PCSystemType`; `None` si el firmware no lo especifica.
pub(super) fn laptop() -> Result<Option<bool>> {
    wmi::first(
        CIMV2,
        "SELECT PCSystemType FROM Win32_ComputerSystem",
        |o| wmi::u32(o, "PCSystemType"),
    )
    .map(|t| match t.flatten() {
        None | Some(PC_SYSTEM_TYPE_UNSPECIFIED) => None,
        Some(t) => Some(t == PC_SYSTEM_TYPE_MOBILE),
    })
}

/// Si VBS está en ejecución; `None` si el sistema no tiene la clase `Win32_DeviceGuard`.
pub(super) fn vbs_running() -> Result<Option<bool>> {
    wmi::first(
        r"ROOT\Microsoft\Windows\DeviceGuard",
        "SELECT VirtualizationBasedSecurityStatus FROM Win32_DeviceGuard",
        |o| wmi::u32(o, "VirtualizationBasedSecurityStatus"),
    )
    .map(|status| status.flatten().map(|s| s == 2))
}

/// ESS de Windows Hello: `Scenarios\WindowsHello\Enabled` de Device Guard (1 = activo). Sin
/// el valor no se sabe (`None`). Ver PLAN.md §6, «Por verificar».
pub(super) fn hello_ess_enabled() -> Result<Option<bool>> {
    registry::read_dword(
        Hive::LocalMachine,
        r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\WindowsHello",
        "Enabled",
    )
    .map(|v| v.map(|v| v == 1))
}

const UNINSTALL: [(Hive, &str); 3] = [
    (
        Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        Hive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
];

/// Nombres de las apps instaladas, ordenados y sin repetir. Una entrada que no se puede leer
/// se salta.
pub(super) fn installed_apps() -> Result<Vec<String>> {
    let mut apps = vec![];
    for (hive, root) in UNINSTALL {
        for key in registry::subkeys(hive, root)? {
            let path = format!(r"{root}\{key}");
            if let Ok(Some(name)) = registry::read_string(hive, &path, "DisplayName") {
                let name = name.trim();
                if !name.is_empty() {
                    apps.push(name.to_owned());
                }
            }
        }
    }
    apps.sort();
    apps.dedup();
    Ok(apps)
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

    #[test]
    fn this_machine_reads_the_system() {
        let build = os_build().unwrap().unwrap();
        assert!(build.parse::<u32>().is_ok(), "{build}");
        assert!(manufacturer().unwrap().is_some());
        laptop().unwrap();
        vbs_running().unwrap();
        hello_ess_enabled().unwrap();
        hags_supported().unwrap();
        // Windows Server (la CI) no tiene Security Center: ahí basta con que no reviente.
        if let Ok(names) = antivirus() {
            assert!(names.iter().all(|n| !n.is_empty()));
        }
        let features = hypervisor_features().unwrap();
        assert!(
            features
                .iter()
                .all(|f| HYPERVISOR_FEATURES.contains(&f.as_str()))
        );
        let apps = installed_apps().unwrap();
        assert!(
            apps.windows(2).all(|w| w[0] < w[1]),
            "ordenadas y sin repetir"
        );
    }
}
