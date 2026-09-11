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
    InputKeys,
    /// Arrange windows: place, raise, focus, configure, close. The shell role's own authority.
    ShellControl,
    /// Change what a display does — mode, scale, arrangement, virtual outputs.
    OutputControl,
    /// Observe input outside the holder's own routing target.
    ///
    /// Not what an ordinary global shortcut needs: a shell registers the chord it wants and is
    /// told when it fires, without seeing anything else. This is for the cases that genuinely
    /// need everything — input diagnostics, accessibility technologies, explicit automation.
    GlobalInputObservation,
    /// Synthesise input. Accessibility and automation need it; nothing else should have it.
    InputInjection,
    /// Hold the screen against every other client, and be the only thing drawing on it.
    LockScreen,
    /// Claim a key chord, and be told when it is pressed.
    ///
    /// Far less than observing input: the holder learns that the chord it asked for happened, and
    /// nothing about anything else. A shell needs this; it does not need to watch what is typed
    /// into a password field to discover that Super+Q was pressed.
    ShortcutControl,
}

impl Capability {
    /// The bit this capability occupies in [`CapabilitySet`].
    ///
    /// The width was chosen while the set had no wire form at all and could still be picked
    /// freely; the authority the shell role needs — shell control, output control, capture,
    /// global input observation, input injection, session control, lock screen — did not fit the
    /// sixteen bits this used to have. The bound is checked here rather than beside the list,
    /// because this shift is the only place outgrowing it would do damage, and a capability past
    /// the sixty-fourth would otherwise shift straight out of the set and read as absent.
    const fn mask(self) -> u64 {
        let index = self as u32;
        assert!(
            index < u64::BITS,
            "capability index outside the set's width"
        );
        1_u64 << index
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
            Self::InputKeys => "input_keys",
            Self::ShellControl => "shell_control",
            Self::OutputControl => "output_control",
            Self::GlobalInputObservation => "global_input_observation",
            Self::InputInjection => "input_injection",
            Self::LockScreen => "lock_screen",
            Self::ShortcutControl => "shortcut_control",
        }
    }
}

/// An allocation-free semantic capability set.
///
/// This bitset is an in-memory API, not a wire representation. ADR-0012 requires a versioned
/// capability set but does not assign numeric capability IDs or define its payload encoding.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet(u64);

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
