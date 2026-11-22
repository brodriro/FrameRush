//! `DeviceProfile`: inventario real del equipo (PLAN.md R8, Iteración 6).
//!
//! Listas, opciones, valores por defecto y visibilidad salen de aquí; un valor fijo solo se usa
//! como fallback y se marca como "estimado".

mod profile;
#[cfg(any(test, feature = "samples"))]
pub mod samples;

// Los tipos del hardware viven en `platform` porque los devuelven sus traits.
pub use framerush_platform::{Battery, Core, CoreKind, Cpu, Display, Gpu, GpuVendor, Mode};
pub use profile::{DeviceProfile, Source, Sourced};
