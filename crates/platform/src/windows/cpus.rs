//! CPU lógicas de todo el equipo, en todos los grupos de procesadores (PLAN.md V23).
//!
//! Hasta 64 CPU lógicas hay un solo grupo y la afinidad es la máscara de
//! `SetProcessAffinityMask`, cuyo bit `i` es la CPU `i`. Con más hay varios grupos, y esa
//! función solo alcanza uno (y no cambia un proceso que ya ocupa varios), así que se usan los
//! CPU Sets por defecto del proceso, que valen para todos los grupos. La numeración global es
//! la de `(grupo, índice en el grupo)` en orden: con un solo grupo coincide con el bit.
#![allow(unsafe_code)]

use crate::{PlatformError, Result};
use windows_sys::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};

/// Un CPU Set: su id para el SO y la CPU lógica que representa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CpuSet {
    pub id: u32,
    pub group: u16,
    pub index: u8,
    /// Núcleo, relativo al grupo: lo comparten los hilos SMT de un núcleo (Microsoft Learn,
    /// `SYSTEM_CPU_SET_INFORMATION`).
    pub core: u8,
    /// Mayor = núcleo más rápido y menos eficiente; distinto de 0 solo en CPUs híbridas.
    pub efficiency_class: u8,
}

/// Los CPU Sets del equipo, ordenados por `(grupo, índice)`: la posición es el número global.
pub(super) fn system_cpu_sets() -> Result<Vec<CpuSet>> {
    let mut len = 0u32;
    // SAFETY: con buffer nulo y tamaño 0 solo escribe en `len` el tamaño necesario.
    unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, std::ptr::null_mut(), 0)
    };
    if len == 0 {
        return Err(PlatformError::Os(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    // Buffer de u64 para que las entradas queden alineadas.
    let mut buf = vec![0u64; (len as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` tiene al menos `len` bytes y vive durante la llamada.
    let ok = unsafe {
        GetSystemCpuSetInformation(
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return Err(PlatformError::Os(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let bytes = buf.as_ptr().cast::<u8>();
    let (mut offset, mut sets) = (0usize, Vec::new());
    while offset + size_of::<u32>() * 2 <= len as usize {
        // SAFETY: `offset` está dentro de los `len` bytes escritos y cada entrada empieza en un
        // límite de su propio tamaño, que el SO alinea; se lee sin suponer alineación.
        let entry = unsafe {
            bytes
                .add(offset)
                .cast::<SYSTEM_CPU_SET_INFORMATION>()
                .read_unaligned()
        };
        if entry.Size == 0 {
            break;
        }
        if entry.Type == CpuSetInformation {
            // SAFETY: con `Type == CpuSetInformation` la unión contiene `CpuSet`.
            let s = unsafe { entry.Anonymous.CpuSet };
            sets.push(CpuSet {
                id: s.Id,
                group: s.Group,
                index: s.LogicalProcessorIndex,
                core: s.CoreIndex,
                efficiency_class: s.EfficiencyClass,
            });
        }
        offset += entry.Size as usize;
    }
    sets.sort_by_key(|s| (s.group, s.index));
    Ok(sets)
}

/// Hay más de un grupo de procesadores.
pub(super) fn multi_group(sets: &[CpuSet]) -> bool {
    sets.iter().any(|s| s.group != sets[0].group)
}

/// Ids de los CPU Sets de las CPU globales `cpus`.
pub(super) fn ids_for(sets: &[CpuSet], cpus: &[usize]) -> Result<Vec<u32>> {
    cpus.iter()
        .map(|&c| {
            sets.get(c)
                .map(|s| s.id)
                .ok_or_else(|| PlatformError::Os(format!("la CPU {c} no existe")))
        })
        .collect()
}

/// CPU globales de los ids `ids`, ordenadas. Los ids que el equipo ya no tiene se ignoran.
pub(super) fn cpus_for(sets: &[CpuSet], ids: &[u32]) -> Vec<usize> {
    let mut cpus: Vec<usize> = ids
        .iter()
        .filter_map(|id| sets.iter().position(|s| s.id == *id))
        .collect();
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2 grupos de 64: ids como los del SO, que no empiezan en 0 ni son la posición.
    fn two_groups() -> Vec<CpuSet> {
        (0..128u32)
            .map(|i| CpuSet {
                id: 256 + i,
                group: (i / 64) as u16,
                index: (i % 64) as u8,
                core: ((i % 64) / 2) as u8,
                efficiency_class: 0,
            })
            .collect()
    }

    #[test]
    fn global_numbers_cross_groups() {
        let sets = two_groups();
        assert!(multi_group(&sets));
        let ids = ids_for(&sets, &[2, 63, 64, 127]).unwrap();
        assert_eq!(ids, [258, 319, 320, 383]);
        assert_eq!(cpus_for(&sets, &ids), [2, 63, 64, 127]);
        assert!(ids_for(&sets, &[128]).is_err());
        assert_eq!(cpus_for(&sets, &[999]), Vec::<usize>::new());
    }

    /// En este equipo: un CPU Set por CPU lógica, sin repetir.
    #[test]
    fn reads_one_set_per_logical_cpu() {
        let sets = system_cpu_sets().unwrap();
        let logical = std::thread::available_parallelism().map_or(0, |n| n.get());
        assert!(sets.len() >= logical, "{} < {logical}", sets.len());
        let mut ids: Vec<u32> = sets.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), sets.len());
    }
}
