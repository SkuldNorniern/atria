//! Codec failures and protocol-reported error categories.

use core::error::Error;
use core::fmt;

use crate::interface::{Interface, MessageKind};
use crate::opcode::Opcode;

/// A failure to encode a value into a caller-provided buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    /// A size calculation overflowed `usize` or the wire integer used to carry it.
    SizeOverflow,
    /// The resulting message would exceed the protocol maximum.
    MessageTooLarge { size: usize, maximum: usize },
    /// A message or payload size was not a multiple of four.
    MisalignedSize { size: usize },
    /// The destination buffer was too short.
    BufferTooSmall { needed: usize, available: usize },
    /// A payload implementation wrote a different length than it declared.
    EncodedLengthMismatch { declared: usize, actual: usize },
}

/// A failure to decode untrusted wire bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// More bytes were required to decode the current value.
    Truncated { needed: usize, available: usize },
    /// The declared message size is smaller than the fixed header.
    MessageTooSmall { size: usize, minimum: usize },
    /// The declared message size exceeds the protocol maximum.
    MessageTooLarge { size: usize, maximum: usize },
    /// A message or field ended at a non-four-byte boundary.
    MisalignedSize { size: usize },
    /// A checked size calculation overflowed.
    SizeOverflow,
    /// A complete packet contained bytes beyond its declared message size.
    TrailingBytes { declared: usize, actual: usize },
    /// A string was not valid UTF-8.
    InvalidUtf8,
    /// A field expected a handle placeholder but received another value.
    InvalidHandlePlaceholder { value: u32 },
    /// No specified operation has this opcode for the interface and message kind.
    UnknownOpcode {
        interface: Interface,
        kind: MessageKind,
        opcode: Opcode,
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeOverflow => formatter.write_str("encoded size cannot be represented"),
            Self::MessageTooLarge { size, maximum } => write!(
                formatter,
                "encoded message is {size} bytes, exceeding the {maximum}-byte protocol maximum"
            ),
            Self::MisalignedSize { size } => write!(
                formatter,
                "encoded size {size} is not aligned to the required four-byte boundary"
            ),
            Self::BufferTooSmall { needed, available } => write!(
                formatter,
                "encoding needs {needed} bytes, but the destination has {available} bytes"
            ),
            Self::EncodedLengthMismatch { declared, actual } => write!(
                formatter,
                "payload declared {declared} encoded bytes but wrote {actual} bytes"
            ),
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, available } => write!(
                formatter,
                "message needs {needed} bytes to decode, but only {available} bytes are available"
            ),
            Self::MessageTooSmall { size, minimum } => write!(
                formatter,
                "declared message size {size} is smaller than the {minimum}-byte header"
            ),
            Self::MessageTooLarge { size, maximum } => write!(
                formatter,
                "declared message size {size} exceeds the {maximum}-byte protocol maximum"
            ),
            Self::MisalignedSize { size } => write!(
                formatter,
                "decoded size {size} is not aligned to the required four-byte boundary"
            ),
            Self::SizeOverflow => formatter.write_str("decoded size cannot be represented"),
            Self::TrailingBytes { declared, actual } => write!(
                formatter,
                "packet contains {actual} bytes, but the message declares only {declared} bytes"
            ),
            Self::InvalidUtf8 => formatter.write_str("message string is not valid UTF-8"),
            Self::InvalidHandlePlaceholder { value } => write!(
                formatter,
                "handle field contains {value:#010x} instead of the required placeholder"
            ),
            Self::UnknownOpcode {
                interface,
                kind,
                opcode,
            } => write!(
                formatter,
                "opcode {:#06x} is not defined for {interface:?} {kind:?} messages",
                opcode.into_raw()
            ),
        }
    }
}

impl Error for EncodeError {}

impl Error for DecodeError {}

/// Error category defined by draft §10.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCategory {
    Protocol,
    Object,
    Resource,
    Compositor,
}

impl ErrorCategory {
    /// Whether the connection survives an error in this category.
    ///
    /// Draft §10 explicitly calls object and resource errors recoverable. It gives compositor
    /// faults a restart procedure rather than classifying them with this boolean.
    #[must_use]
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::Object | Self::Resource)
    }
}

/// A numeric code carried by `display.error`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct ErrorCode(u32);

impl ErrorCode {
    /// The only individual code assigned by the draft, in wire example 3.
    pub const INVALID_OBJECT: Self = Self(0x0001);

    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn into_raw(self) -> u32 {
        self.0
    }

    /// Classifies an assigned category range. Values outside §10's ranges remain unclassified.
    #[must_use]
    pub const fn category(self) -> Option<ErrorCategory> {
        match self.0 {
            0x0001..=0x00ff => Some(ErrorCategory::Protocol),
            0x0100..=0x01ff => Some(ErrorCategory::Object),
            0x0200..=0x02ff => Some(ErrorCategory::Resource),
            0x0300..=0x03ff => Some(ErrorCategory::Compositor),
            _ => None,
        }
    }
}
