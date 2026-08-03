//! Connection-scoped protocol object identifiers.

/// First identifier a client may allocate for a non-singleton object.
pub const FIRST_CLIENT_ALLOCATED_ID: u32 = 256;

/// A connection-scoped protocol object identifier.
///
/// The draft reserves `1..=255` for protocol-defined singletons and starts client allocation
/// at 256. It does not define the meaning of zero, so this type preserves zero rather than
/// rejecting it or assigning it a meaning.
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

    /// Whether this identifier lies in the protocol-reserved singleton range.
    #[must_use]
    pub const fn is_reserved(self) -> bool {
        self.0 >= 1 && self.0 < FIRST_CLIENT_ALLOCATED_ID
    }

    /// Whether clients may allocate this identifier according to draft §4.
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
