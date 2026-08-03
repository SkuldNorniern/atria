//! Semantic capability negotiation from ADR-0012.

/// A capability name defined by draft §6 or ADR-0012.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Capability {
    SurfaceCreate,
    BufferImport,
    ExtendedInput,
    Screencopy,
    PrivilegedSession,
    SoftwareShm,
    GpuPrime,
    ExplicitGpuFence,
    RevocableSeat,
    DrmDumbBuffer,
    DrmAtomicCommit,
    DrmPageFlip,
}

impl Capability {
    const fn mask(self) -> u16 {
        1_u16 << (self as u8)
    }

    /// Returns the spelling used by the specification or ADR.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SurfaceCreate => "surface_create",
            Self::BufferImport => "buffer_import",
            Self::ExtendedInput => "extended_input",
            Self::Screencopy => "screencopy",
            Self::PrivilegedSession => "privileged_session",
            Self::SoftwareShm => "software_shm",
            Self::GpuPrime => "gpu_prime",
            Self::ExplicitGpuFence => "explicit_gpu_fence",
            Self::RevocableSeat => "revocable_seat",
            Self::DrmDumbBuffer => "drm_dumb_buffer",
            Self::DrmAtomicCommit => "drm_atomic_commit",
            Self::DrmPageFlip => "drm_page_flip",
        }
    }
}

/// An allocation-free semantic capability set.
///
/// This bitset is an in-memory API, not a wire representation. ADR-0012 requires a versioned
/// capability set but does not assign numeric capability IDs or define its payload encoding.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The default grants stated by draft §6.
    #[must_use]
    pub const fn default_grants() -> Self {
        Self(Capability::SurfaceCreate.mask() | Capability::BufferImport.mask())
    }

    #[must_use]
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & capability.mask() != 0
    }

    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.mask())
    }

    #[must_use]
    pub const fn without(self, capability: Capability) -> Self {
        Self(self.0 & !capability.mask())
    }

    /// Capabilities that both peers advertise.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether every required capability is present.
    #[must_use]
    pub const fn satisfies(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}
