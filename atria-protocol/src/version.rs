//! Protocol version ranges and overlap detection.

/// Current protocol version from draft §12.
pub const CURRENT_VERSION: u32 = 1;
/// Oldest interface/protocol version the draft currently requires.
pub const MINIMUM_SUPPORTED_VERSION: u32 = 1;

/// An inclusive version range offered by a peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionRange {
    pub minimum: u32,
    pub maximum: u32,
}

impl VersionRange {
    /// Creates a range when its bounds are ordered.
    #[must_use]
    pub const fn new(minimum: u32, maximum: u32) -> Option<Self> {
        if minimum <= maximum {
            Some(Self { minimum, maximum })
        } else {
            None
        }
    }

    /// Computes the mutually supported range.
    ///
    /// Draft §12 does not say whether the client or compositor selects a version within an
    /// overlap, nor whether the lowest or highest is preferred, so this API returns the full
    /// overlap instead of inventing a selection rule.
    #[must_use]
    pub const fn overlap(self, other: Self) -> Option<Self> {
        let minimum = if self.minimum > other.minimum {
            self.minimum
        } else {
            other.minimum
        };
        let maximum = if self.maximum < other.maximum {
            self.maximum
        } else {
            other.maximum
        };
        Self::new(minimum, maximum)
    }
}
