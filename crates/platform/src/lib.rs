//! Traits de acceso al SO e implementaciones bajo `cfg(target_os)`.
//!
//! El código `unsafe` o específico de un SO solo vive en este crate.

mod data;
mod error;
mod lang;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod os;
mod process;
mod scan;
mod system;
#[cfg(windows)]
mod windows;
#[cfg(any(test, target_os = "linux"))]
mod xdg;

pub use data::data_dir;
pub use error::{PlatformError, Result};
pub use lang::Lang;
#[cfg(target_os = "linux")]
pub use linux::{
    Helper, LinuxPlatform, downloads_dir, serve_helper, steam_launch, steam_root, steam_running_app,
};
#[cfg(target_os = "macos")]
pub use macos::{MacosPlatform, downloads_dir};
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
    AcerSensor, HardwareChange, HardwareWatch, Helper, Nvapi, WindowsPlatform, add_to_system_path,
    downloads_dir, remove_from_system_path, serve_helper, steam_launch, steam_root,
    steam_running_app, watch_hardware,
};
