//! Payloads for a display describing itself.

use super::{EncodePayload, read_u64, write_u64};
use crate::wire::{Decoder, Encoder};
use crate::{DecodeError, EncodeError};

/// Which display this is, for as long as it is the same display.
///
/// A `u128` on the wire as two `u64` halves. The identity outlives an unplug, so a client or a
/// shell can key remembered arrangement to it and have that survive a cable being moved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputIdentity {
    pub high: u64,
    pub low: u64,
}

impl OutputIdentity {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not exactly two 64-bit halves.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let high = read_u64(&mut decoder)?;
        let low = read_u64(&mut decoder)?;
        decoder.finish()?;
        Ok(Self { high, low })
    }
}

impl EncodePayload for OutputIdentity {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.high)?;
        write_u64(encoder, self.low)
    }
}

/// Where a display sits and what it physically is.
///
/// `identity_source` says whether the identity above is the panel's own or was derived from the
/// connector it is plugged into. A shell that remembers an arrangement needs to know which,
/// because two identical panels swapped between ports exchange positional identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub physical_width_millimetres: u32,
    pub physical_height_millimetres: u32,
    pub identity_source: u32,
}

impl OutputGeometry {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
            physical_width_millimetres: decoder.read_u32()?,
            physical_height_millimetres: decoder.read_u32()?,
            identity_source: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for OutputGeometry {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(20)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)?;
        encoder.write_u32(self.physical_width_millimetres)?;
        encoder.write_u32(self.physical_height_millimetres)?;
        encoder.write_u32(self.identity_source)
    }
}

/// The resolution and refresh the display is running.
///
/// Refresh is in millihertz so 59.94 Hz is exact rather than rounded to 60.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputMode {
    pub width: u32,
    pub height: u32,
    pub refresh_millihertz: u32,
}

impl OutputMode {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not three 32-bit fields.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            width: decoder.read_u32()?,
            height: decoder.read_u32()?,
            refresh_millihertz: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for OutputMode {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.width)?;
        encoder.write_u32(self.height)?;
        encoder.write_u32(self.refresh_millihertz)
    }
}

/// The display's scale, as an exact ratio.
///
/// A pair rather than a number: 1.5 is 3/2, and no integer expresses it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputScale {
    pub numerator: u32,
    pub denominator: u32,
}

impl OutputScale {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not two 32-bit terms.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            numerator: decoder.read_u32()?,
            denominator: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for OutputScale {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.numerator)?;
        encoder.write_u32(self.denominator)
    }
}
