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
    Battery, Capabilities, CapabilitySet, Core, CoreKind, Cpu, DefenderExclusion, Display,
    DisplayControl, GameFiles, GameTuning, Gpu, GpuVendor, Inventory, MemoryControl, Mode, PciId,
    Platform, PowerControl, PowerPlan, SavedData, SavedValue, SecurityTweaks, ServiceControl,
    ServiceState, SystemTweaks, Tweak,
};
#[cfg(windows)]
pub use windows::{
    AcerSensor, HardwareChange, HardwareWatch, Helper, Nvapi, WindowsPlatform, downloads_dir,
    serve_helper, steam_launch, steam_root, steam_running_app, watch_hardware,
};
