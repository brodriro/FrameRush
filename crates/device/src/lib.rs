//! `DeviceProfile`: inventario real del equipo (PLAN.md R8, Iteración 6).
//!
//! Listas, opciones, valores por defecto y visibilidad salen de aquí; un valor fijo solo se usa
//! como fallback y se marca como "estimado".

mod profile;

pub use profile::{
    Battery, Core, CoreKind, Cpu, DeviceProfile, Display, Gpu, GpuVendor, Mode, Source, Sourced,
};
