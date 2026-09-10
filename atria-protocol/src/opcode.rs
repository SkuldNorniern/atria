//! Interface operations and the opcode assignments present in the draft.
//!
//! An opcode is local to its interface. There is no global range table: the same number means
//! different things on different interfaces, and only the object being addressed says which
//! interface is in play. `display.get_registry` and `buffer.release_with_fence` are both
//! `0x0001` and are told apart by the object, not by the number.

/// A wire opcode, meaningful only alongside the interface it was sent to.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Opcode(u16);

impl Opcode {
    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn into_raw(self) -> u16 {
        self.0
    }
}

impl From<u16> for Opcode {
    fn from(value: u16) -> Self {
        Self::from_raw(value)
    }
}

impl From<Opcode> for u16 {
    fn from(value: Opcode) -> Self {
        value.into_raw()
    }
}
