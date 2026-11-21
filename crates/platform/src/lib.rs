//! Traits de acceso al SO e implementaciones bajo `cfg(target_os)`.
//!
//! El código `unsafe` o específico de un SO solo vive en este crate.

mod error;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod os;
mod process;
mod system;

pub use error::{PlatformError, Result};
pub use os::Os;
pub use process::{Priority, ProcessControl, ProcessInfo, StartTime, Target};
pub use system::{
    Capabilities, CapabilitySet, GameTuning, Platform, PowerControl, PowerPlan, ServiceControl,
    ServiceState,
};
