//! `DeviceProfile`: inventario real del equipo (PLAN.md R8, Iteración 6).
//!
//! Listas, opciones, valores por defecto y visibilidad salen de aquí; un valor fijo solo se usa
//! como fallback y se marca como "estimado".

mod profile;
#[cfg(any(test, feature = "samples"))]
pub mod samples;

// `Display` y `Mode` viven en `platform` porque los usa `DisplayControl`.
pub use framerush_platform::{Display, Mode};
pub use profile::{Battery, Core, CoreKind, Cpu, DeviceProfile, Gpu, GpuVendor, Source, Sourced};
