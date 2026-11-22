//! Traits de acceso al SO e implementaciones bajo `cfg(target_os)`.
//!
//! El código `unsafe` o específico de un SO solo vive en este crate.

mod error;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod os;
mod process;
mod scan;
mod system;
#[cfg(windows)]
mod windows;

pub use error::{PlatformError, Result};
pub use os::Os;
pub use process::{Priority, ProcessControl, ProcessInfo, StartTime, Target};
pub use scan::scan_processes;
pub use system::{
    Battery, Capabilities, CapabilitySet, Core, CoreKind, Cpu, Display, DisplayControl, GameTuning,
    Gpu, GpuVendor, Inventory, MemoryControl, Mode, Platform, PowerControl, PowerPlan, SavedData,
    SavedValue, ServiceControl, ServiceState, SystemTweaks, Tweak,
};
#[cfg(windows)]
pub use windows::{Helper, WindowsPlatform, serve_helper};
