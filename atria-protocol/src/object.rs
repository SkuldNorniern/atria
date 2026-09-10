//! Connection-scoped protocol object identifiers.
//!
//! Zero is the null identifier, one is the display, and everything above is allocated by the
//! client. There is no reserved band between them: a block of identifiers set aside for
//! protocol-defined singletons only pays off if singletons keep being added, and every object
//! after the display is created by a request that names its own identifier anyway.

/// First identifier a client may allocate.
pub const FIRST_CLIENT_ALLOCATED_ID: u32 = 2;

/// A connection-scoped protocol object identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ObjectId(u32);

impl ObjectId {
    /// The display singleton used by the draft's wire examples.
    pub const DISPLAY: Self = Self(1);

    /// Preserves an identifier received from the wire.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the wire representation.
    #[must_use]
    pub const fn into_raw(self) -> u32 {
        self.0
    }

    /// Whether this identifier names nothing.
    #[must_use]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Whether a client may allocate this identifier.
    #[must_use]
    pub const fn is_client_allocatable(self) -> bool {
        self.0 >= FIRST_CLIENT_ALLOCATED_ID
    }
}

impl From<u32> for ObjectId {
    fn from(value: u32) -> Self {
        Self::from_raw(value)
    }
}

impl From<ObjectId> for u32 {
    fn from(value: ObjectId) -> Self {
        value.into_raw()
    }
}
