#![forbid(unsafe_code)]

//! CPU-only composition and output sinks for Atria.
//!
//! The protocol draft requires format validation but does not enumerate pixel formats. This
//! crate therefore performs no format conversion or channel interpretation. A validated
//! platform adapter configures one packed pixel size, and source and output bytes are copied
//! opaquely. Adding named formats here before the protocol defines them would create a second
//! protocol by accident.

mod buffer;
mod error;
mod frame;
mod output;
mod sink;

pub use atria_compositor::Rect;
pub use buffer::{BufferKey, BufferStore, SoftwareBuffer};
pub use error::{ComposeError, SinkError, ValidationError};
pub use frame::{Frame, PixelLayout};
pub use output::{FrameReport, PresentError, SoftwareOutput};
pub use sink::{FileSink, FrameSink, HeadlessSink, Presented};

use atria_protocol::capability::{Capability, CapabilitySet};

/// Semantic capabilities implemented by this backend.
///
/// In particular, GPU import, explicit fences, and revocable seat tokens are absent rather
/// than emulated.
#[must_use]
pub const fn capabilities() -> CapabilitySet {
    CapabilitySet::default_grants().with(Capability::SoftwareShm)
}
