//! Payloads a shell and the compositor exchange about windows.

use super::{EncodePayload, encoded_string_len, read_u64, write_u64};
use crate::wire::{Decoder, Encoder};
use crate::{DecodeError, EncodeError};

/// A window a shell is told about, by the handle it will use to name it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellHandle {
    pub handle: u64,
}

impl ShellHandle {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not one 64-bit handle.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let handle = read_u64(&mut decoder)?;
        decoder.finish()?;
        Ok(Self { handle })
    }
}

impl EncodePayload for ShellHandle {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.handle)
    }
}

/// A shell asking a window to adopt a size and a set of states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellConfigure {
    pub handle: u64,
    pub width: u32,
    pub height: u32,
    pub state: u32,
}

impl ShellConfigure {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the four fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let handle = read_u64(&mut decoder)?;
        let value = Self {
            handle,
            width: decoder.read_u32()?,
            height: decoder.read_u32()?,
            state: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ShellConfigure {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(20)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.handle)?;
        encoder.write_u32(self.width)?;
        encoder.write_u32(self.height)?;
        encoder.write_u32(self.state)
    }
}

/// A shell putting a window somewhere.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellPlace {
    pub handle: u64,
    pub x: i32,
    pub y: i32,
}

impl ShellPlace {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a handle and a position.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let handle = read_u64(&mut decoder)?;
        let value = Self {
            handle,
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ShellPlace {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.handle)?;
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)
    }
}

/// One window in what a shell is told, by handle and title.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellToplevel<'a> {
    pub handle: u64,
    pub title: &'a str,
}

impl<'a> ShellToplevel<'a> {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a handle and a title.
    pub fn decode(input: &'a [u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let handle = read_u64(&mut decoder)?;
        let title = decoder.read_string()?;
        decoder.finish()?;
        Ok(Self { handle, title })
    }
}

impl EncodePayload for ShellToplevel<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        8_usize
            .checked_add(encoded_string_len(self.title)?)
            .ok_or(EncodeError::SizeOverflow)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.handle)?;
        encoder.write_string(self.title)
    }
}
