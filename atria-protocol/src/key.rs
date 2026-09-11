//! Which key was pressed, in a vocabulary that outlives any one platform.
//!
//! A key is a position on a keyboard, not a character. What a position produces depends on a
//! layout, and a layout is not the compositor's to know — text arrives by its own route, and a
//! client that wants characters asks for characters rather than guessing from these.
//!
//! The numbers are USB HID usages from the Keyboard/Keypad page, because that is what keyboards
//! themselves report and it is the same on every platform Artery will run on. Linux evdev codes
//! would have been easier here and wrong everywhere else.

/// A physical key, as a USB HID Keyboard/Keypad usage.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhysicalKey(pub u32);

impl PhysicalKey {
    /// Preserves a usage received from the wire or from a device.
    #[must_use]
    pub const fn from_usage(usage: u32) -> Self {
        Self(usage)
    }

    /// The usage this key reports.
    #[must_use]
    pub const fn usage(self) -> u32 {
        self.0
    }

    /// Whether this key is one of the modifiers, which are contiguous in the HID page.
    #[must_use]
    pub const fn is_modifier(self) -> bool {
        self.0 >= usage::LEFT_CONTROL && self.0 <= usage::RIGHT_META
    }
}

/// The usages this system names. Not the whole page: the ones something refers to by name.
pub mod usage {
    pub const A: u32 = 0x04;
    pub const Q: u32 = 0x14;
    pub const W: u32 = 0x1a;
    pub const Z: u32 = 0x1d;
    pub const ONE: u32 = 0x1e;
    pub const ENTER: u32 = 0x28;
    pub const ESCAPE: u32 = 0x29;
    pub const BACKSPACE: u32 = 0x2a;
    pub const TAB: u32 = 0x2b;
    pub const SPACE: u32 = 0x2c;
    pub const F1: u32 = 0x3a;
    pub const RIGHT: u32 = 0x4f;
    pub const LEFT: u32 = 0x50;
    pub const DOWN: u32 = 0x51;
    pub const UP: u32 = 0x52;
    pub const LEFT_CONTROL: u32 = 0xe0;
    pub const LEFT_SHIFT: u32 = 0xe1;
    pub const LEFT_ALT: u32 = 0xe2;
    pub const LEFT_META: u32 = 0xe3;
    pub const RIGHT_CONTROL: u32 = 0xe4;
    pub const RIGHT_SHIFT: u32 = 0xe5;
    pub const RIGHT_ALT: u32 = 0xe6;
    pub const RIGHT_META: u32 = 0xe7;
}

/// Which modifiers are held, as a bitmask.
///
/// Separate from the keys themselves because a client asking "is control down" should not have to
/// track eight key positions and two sides of each.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const CONTROL: u32 = 1 << 0;
    pub const SHIFT: u32 = 1 << 1;
    pub const ALT: u32 = 1 << 2;
    pub const META: u32 = 1 << 3;

    /// The bit a key contributes, if it contributes one.
    #[must_use]
    pub const fn of(key: PhysicalKey) -> Option<u32> {
        match key.0 {
            usage::LEFT_CONTROL | usage::RIGHT_CONTROL => Some(Self::CONTROL),
            usage::LEFT_SHIFT | usage::RIGHT_SHIFT => Some(Self::SHIFT),
            usage::LEFT_ALT | usage::RIGHT_ALT => Some(Self::ALT),
            usage::LEFT_META | usage::RIGHT_META => Some(Self::META),
            _ => None,
        }
    }

    #[must_use]
    pub const fn contains(self, modifier: u32) -> bool {
        self.0 & modifier != 0
    }

    #[must_use]
    pub const fn with(self, modifier: u32) -> Self {
        Self(self.0 | modifier)
    }

    #[must_use]
    pub const fn without(self, modifier: u32) -> Self {
        Self(self.0 & !modifier)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}
