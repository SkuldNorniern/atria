//! Payloads for text, and for the method that produces it.

use super::{EncodePayload, encoded_string_len};
use crate::ObjectId;
use crate::wire::{Decoder, Encoder};
use crate::{DecodeError, EncodeError};

/// Text a client may be sent in one message.
///
/// A commit is a word or a phrase, not a document. The bound is what stops an input method from
/// making the compositor's memory its own, and it is stated rather than discovered.
pub const MAX_TEXT_BYTES: usize = 4096;

/// What a text field is for. An input method must not compose into a password.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextPurpose {
    #[default]
    Normal,
    /// Never composed into, and never remembered.
    Password,
    Digits,
}

impl TextPurpose {
    /// The wire's number for this purpose, or nothing for a number it does not define.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Normal),
            1 => Some(Self::Password),
            2 => Some(Self::Digits),
            _ => None,
        }
    }

    #[must_use]
    pub const fn into_raw(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Password => 1,
            Self::Digits => 2,
        }
    }

    /// Whether an input method may compose into a field of this purpose.
    #[must_use]
    pub const fn admits_composition(self) -> bool {
        !matches!(self, Self::Password)
    }
}

/// Text being composed, with where the caret sits inside it.
///
/// The caret is a byte offset, and may be negative to mean "do not show one". Composed text is
/// replaced wholesale by the next preedit: it is a proposal, not an edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Preedit<'a> {
    /// The composition this belongs to. One that has ended is refused, not applied.
    pub composition: u32,
    pub text: &'a str,
    pub cursor_begin: i32,
    pub cursor_end: i32,
}

impl<'a> Preedit<'a> {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a string and two offsets, or when the text
    /// is longer than [`MAX_TEXT_BYTES`].
    pub fn decode(input: &'a [u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let composition = decoder.read_u32()?;
        let text = decoder.read_string()?;
        if text.len() > MAX_TEXT_BYTES {
            return Err(DecodeError::SizeOverflow);
        }
        let value = Self {
            composition,
            text,
            cursor_begin: decoder.read_i32()?,
            cursor_end: decoder.read_i32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for Preedit<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        encoded_string_len(self.text)?
            .checked_add(12)
            .ok_or(EncodeError::SizeOverflow)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.composition)?;
        encoder.write_string(self.text)?;
        encoder.write_i32(self.cursor_begin)?;
        encoder.write_i32(self.cursor_end)
    }
}

/// Text that is text: no longer a proposal, and the field should keep it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitText<'a> {
    /// The composition this belongs to.
    pub composition: u32,
    pub text: &'a str,
}

impl<'a> CommitText<'a> {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a string, or the text is too long.
    pub fn decode(input: &'a [u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let composition = decoder.read_u32()?;
        let text = decoder.read_string()?;
        if text.len() > MAX_TEXT_BYTES {
            return Err(DecodeError::SizeOverflow);
        }
        decoder.finish()?;
        Ok(Self { composition, text })
    }
}

impl EncodePayload for CommitText<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        encoded_string_len(self.text)?
            .checked_add(4)
            .ok_or(EncodeError::SizeOverflow)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.composition)?;
        encoder.write_string(self.text)
    }
}

/// Where the caret is on screen, so a candidate window is put beside it and not over it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl CursorArea {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a rectangle.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
            width: decoder.read_u32()?,
            height: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for CursorArea {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)?;
        encoder.write_u32(self.width)?;
        encoder.write_u32(self.height)
    }
}

/// An input method being told which field it is composing into, and what that field is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputMethodActivation {
    pub surface: ObjectId,
    pub purpose: u32,
    /// The composition being started. Everything the method says must quote it back.
    pub composition: u32,
}

impl InputMethodActivation {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a surface and a purpose.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            surface: ObjectId::from_raw(decoder.read_u32()?),
            purpose: decoder.read_u32()?,
            composition: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for InputMethodActivation {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.surface.into_raw())?;
        encoder.write_u32(self.purpose)?;
        encoder.write_u32(self.composition)
    }
}

/// Two numbers a method quotes back: the composition, and its own serial for the round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompositionRound {
    pub composition: u32,
    pub serial: u32,
}

impl CompositionRound {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not two numbers.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            composition: decoder.read_u32()?,
            serial: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for CompositionRound {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.composition)?;
        encoder.write_u32(self.serial)
    }
}
