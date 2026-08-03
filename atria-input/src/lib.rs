//! Evdev report assembly and key normalization for Atria.
//!
//! This package deliberately exposes only the key vocabulary it normalizes. Pointer, touch,
//! tablet, switch, and gesture records remain unhandled instead of being approximated.

mod device;
mod epoch;
mod error;
mod key;
mod readiness;
mod report;
mod set;
mod uapi;

pub use device::{DeviceClass, EventTypeSet, InputDevice, enumerate};
pub use epoch::{Epoch, EpochChange, InputBatch, InputPipeline, PipelineStatus};
pub use error::InputError;
pub use key::{KeyAction, KeyCode, KeyEvent, KeyNormalizer};
pub use report::{RawInputEvent, RawReport, ReportAccumulator, ReportStatus};
pub use set::{DeviceInputBatch, InputSet};

use atria_protocol::capability::{Capability, CapabilitySet};

/// Reports the event class this package normalizes without emulating deferred classes.
#[must_use]
pub const fn capabilities() -> CapabilitySet {
    CapabilitySet::default_grants().with(Capability::InputKeys)
}
