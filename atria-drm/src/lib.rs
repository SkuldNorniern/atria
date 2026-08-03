//! Direct DRM/KMS scanout for software-composited Atria frames.

mod backend;
mod error;
mod logic;
mod sink;

pub use backend::{Connector, DeviceConfig, DrmDevice, Encoder, ResourceSnapshot};
pub use error::DrmError;
pub use logic::{
    BufferGeometry, DrmFormat, Mode, ModeSelection, buffer_geometry, drm_format, next_buffer_index,
    select_mode,
};
pub use sink::DrmSink;

use atria_protocol::capability::{Capability, CapabilitySet};

/// Reports the scanout facilities discovered on a DRM device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrmCapabilities {
    dumb_buffers: bool,
    atomic_commit: bool,
}

impl DrmCapabilities {
    #[must_use]
    pub const fn new(dumb_buffers: bool, atomic_commit: bool) -> Self {
        Self {
            dumb_buffers,
            atomic_commit,
        }
    }

    #[must_use]
    pub const fn dumb_buffers(self) -> bool {
        self.dumb_buffers
    }

    #[must_use]
    pub const fn atomic_commit(self) -> bool {
        self.atomic_commit
    }

    /// Page flip is reported only as the real fallback when atomic commit is absent.
    #[must_use]
    pub const fn capability_set(self) -> CapabilitySet {
        let mut capabilities = CapabilitySet::default_grants();
        if self.dumb_buffers {
            capabilities = capabilities.with(Capability::DrmDumbBuffer);
        }
        if self.atomic_commit {
            capabilities.with(Capability::DrmAtomicCommit)
        } else {
            capabilities.with(Capability::DrmPageFlip)
        }
    }
}
