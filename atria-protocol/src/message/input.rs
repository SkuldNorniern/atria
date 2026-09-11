//! Payloads for a seat's pointer, keyboard and claimed chords.

use super::{EncodePayload, read_u64, write_u64};
use crate::ObjectId;
use crate::wire::{Decoder, Encoder};
use crate::{DecodeError, EncodeError};

/// The pointer arrived over a surface, at a point inside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerEnter {
    pub serial: u32,
    pub surface: ObjectId,
    pub x: i32,
    pub y: i32,
    pub epoch: u32,
}

impl PointerEnter {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            serial: decoder.read_u32()?,
            surface: ObjectId::from_raw(decoder.read_u32()?),
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for PointerEnter {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(20)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.surface.into_raw())?;
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)?;
        encoder.write_u32(self.epoch)
    }
}

/// The pointer is no longer over a surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerLeave {
    pub serial: u32,
    pub surface: ObjectId,
    pub epoch: u32,
}

impl PointerLeave {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a serial, a surface and an epoch.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            serial: decoder.read_u32()?,
            surface: ObjectId::from_raw(decoder.read_u32()?),
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for PointerLeave {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.surface.into_raw())?;
        encoder.write_u32(self.epoch)
    }
}

/// The pointer moved to a point inside the surface it is over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerMotion {
    pub time_ns: u64,
    pub x: i32,
    pub y: i32,
    pub epoch: u32,
}

impl PointerMotion {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a time, a point and an epoch.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let time_ns = read_u64(&mut decoder)?;
        let value = Self {
            time_ns,
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for PointerMotion {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(20)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.time_ns)?;
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)?;
        encoder.write_u32(self.epoch)
    }
}

/// A pointer button changed state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerButton {
    pub serial: u32,
    pub time_ns: u64,
    pub button: u32,
    pub state: u32,
    pub epoch: u32,
}

impl PointerButton {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let serial = decoder.read_u32()?;
        let time_ns = read_u64(&mut decoder)?;
        let value = Self {
            serial,
            time_ns,
            button: decoder.read_u32()?,
            state: decoder.read_u32()?,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for PointerButton {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(24)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        write_u64(encoder, self.time_ns)?;
        encoder.write_u32(self.button)?;
        encoder.write_u32(self.state)?;
        encoder.write_u32(self.epoch)
    }
}

/// A scroll or other continuous axis moved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerAxis {
    pub time_ns: u64,
    pub axis: u32,
    pub value: i32,
    pub epoch: u32,
}

impl PointerAxis {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a time, an axis, a value and an epoch.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let time_ns = read_u64(&mut decoder)?;
        let value = Self {
            time_ns,
            axis: decoder.read_u32()?,
            value: decoder.read_i32()?,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for PointerAxis {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(20)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.time_ns)?;
        encoder.write_u32(self.axis)?;
        encoder.write_i32(self.value)?;
        encoder.write_u32(self.epoch)
    }
}

/// What a person did to a window, as a shell is told about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShellInteraction {
    pub seat: u64,
    pub handle: u64,
    pub serial: u32,
    pub kind: u32,
    /// Where in the window it happened, in the window's own coordinates.
    ///
    /// A shell needs this to tell a press on a window's chrome from a press on its content, which
    /// is the difference between dragging the window and using the application.
    pub x: i32,
    pub y: i32,
}

impl ShellInteraction {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the four fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let seat = read_u64(&mut decoder)?;
        let handle = read_u64(&mut decoder)?;
        let value = Self {
            seat,
            handle,
            serial: decoder.read_u32()?,
            kind: decoder.read_u32()?,
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ShellInteraction {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(32)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.seat)?;
        write_u64(encoder, self.handle)?;
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.kind)?;
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)
    }
}

/// A seat and the window a shell is talking about on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeatHandle {
    pub seat: u64,
    pub handle: u64,
}

impl SeatHandle {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a seat and a handle.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let seat = read_u64(&mut decoder)?;
        let handle = read_u64(&mut decoder)?;
        decoder.finish()?;
        Ok(Self { seat, handle })
    }
}

impl EncodePayload for SeatHandle {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.seat)?;
        write_u64(encoder, self.handle)
    }
}

/// Where the pointer is, while a shell holds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeatPoint {
    pub seat: u64,
    pub x: i32,
    pub y: i32,
}

impl SeatPoint {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a seat and a point.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let seat = read_u64(&mut decoder)?;
        let value = Self {
            seat,
            x: decoder.read_i32()?,
            y: decoder.read_i32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for SeatPoint {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.seat)?;
        encoder.write_i32(self.x)?;
        encoder.write_i32(self.y)
    }
}

/// A seat on its own, for a message that names nothing else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeatName {
    pub seat: u64,
}

impl SeatName {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not one seat.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let seat = read_u64(&mut decoder)?;
        decoder.finish()?;
        Ok(Self { seat })
    }
}

impl EncodePayload for SeatName {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        write_u64(encoder, self.seat)
    }
}

/// A surface gained keyboard focus, with every key already held.
///
/// The held set travels with the focus. A client told only about later releases would believe
/// keys were up that are not, and act on a chord it never saw begin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardEnter<'a> {
    pub serial: u32,
    pub surface: ObjectId,
    pub modifiers: u32,
    pub epoch: u32,
    /// Physical positions, as HID usages.
    pub held: &'a [u32],
}

impl<'a> KeyboardEnter<'a> {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &'a [u8], out: &'a mut [u32]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let serial = decoder.read_u32()?;
        let surface = ObjectId::from_raw(decoder.read_u32()?);
        let modifiers = decoder.read_u32()?;
        let epoch = decoder.read_u32()?;
        let count = decoder.read_array_len()? as usize;
        if count > out.len() {
            return Err(DecodeError::SizeOverflow);
        }
        for slot in out.iter_mut().take(count) {
            *slot = decoder.read_u32()?;
        }
        decoder.finish()?;
        Ok(Self {
            serial,
            surface,
            modifiers,
            epoch,
            held: &out[..count],
        })
    }
}

impl EncodePayload for KeyboardEnter<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        20_usize
            .checked_add(
                self.held
                    .len()
                    .checked_mul(4)
                    .ok_or(EncodeError::SizeOverflow)?,
            )
            .ok_or(EncodeError::SizeOverflow)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.surface.into_raw())?;
        encoder.write_u32(self.modifiers)?;
        encoder.write_u32(self.epoch)?;
        let count = u32::try_from(self.held.len()).map_err(|_| EncodeError::SizeOverflow)?;
        encoder.write_array_len(count)?;
        for key in self.held {
            encoder.write_u32(*key)?;
        }
        Ok(())
    }
}

/// A surface lost keyboard focus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardLeave {
    pub serial: u32,
    pub surface: ObjectId,
    pub epoch: u32,
}

impl KeyboardLeave {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not a serial, a surface and an epoch.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            serial: decoder.read_u32()?,
            surface: ObjectId::from_raw(decoder.read_u32()?),
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for KeyboardLeave {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.surface.into_raw())?;
        encoder.write_u32(self.epoch)
    }
}

/// A key changed state. The key is a physical position, not a character.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardKey {
    pub serial: u32,
    pub time_ns: u64,
    /// A USB HID Keyboard/Keypad usage.
    pub key: u32,
    pub state: u32,
    pub epoch: u32,
}

impl KeyboardKey {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let serial = decoder.read_u32()?;
        let time_ns = read_u64(&mut decoder)?;
        let value = Self {
            serial,
            time_ns,
            key: decoder.read_u32()?,
            state: decoder.read_u32()?,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for KeyboardKey {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(24)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        write_u64(encoder, self.time_ns)?;
        encoder.write_u32(self.key)?;
        encoder.write_u32(self.state)?;
        encoder.write_u32(self.epoch)
    }
}

/// Which modifiers are held, latched and locked, and which group is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardModifiers {
    pub depressed: u32,
    pub latched: u32,
    pub locked: u32,
    pub group: u32,
    pub epoch: u32,
}

impl KeyboardModifiers {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            depressed: decoder.read_u32()?,
            latched: decoder.read_u32()?,
            locked: decoder.read_u32()?,
            group: decoder.read_u32()?,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for KeyboardModifiers {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(20)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.depressed)?;
        encoder.write_u32(self.latched)?;
        encoder.write_u32(self.locked)?;
        encoder.write_u32(self.group)?;
        encoder.write_u32(self.epoch)
    }
}

/// One number a message carries and names for itself: a serial, a purpose, a frame request.
///
/// Structurally the same as a global's name and deliberately not that type. Reusing a name for a
/// purpose makes every reader work out which is meant, and the wire is where a reader can least
/// afford to guess.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Number {
    pub value: u32,
}

impl Number {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not one number.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            value: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for Number {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.value)
    }
}

/// A chord a holder wants to be told about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutRegistration {
    /// The holder's own name for it, quoted back when it fires.
    pub shortcut: u32,
    pub seat: u64,
    /// A USB HID Keyboard/Keypad usage.
    pub trigger: u32,
    pub modifiers: u32,
    /// Whether the modifiers must match exactly, or merely all be held.
    pub mode: u32,
}

impl ShortcutRegistration {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let shortcut = decoder.read_u32()?;
        let seat = read_u64(&mut decoder)?;
        let value = Self {
            shortcut,
            seat,
            trigger: decoder.read_u32()?,
            modifiers: decoder.read_u32()?,
            mode: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ShortcutRegistration {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(24)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.shortcut)?;
        write_u64(encoder, self.seat)?;
        encoder.write_u32(self.trigger)?;
        encoder.write_u32(self.modifiers)?;
        encoder.write_u32(self.mode)
    }
}

/// A chord fired. Says which one, and nothing about anything else pressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutTriggered {
    pub shortcut: u32,
    pub seat: u64,
    pub serial: u32,
    pub time_ns: u64,
    pub epoch: u32,
}

impl ShortcutTriggered {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not the five fields the table names.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let shortcut = decoder.read_u32()?;
        let seat = read_u64(&mut decoder)?;
        let serial = decoder.read_u32()?;
        let time_ns = read_u64(&mut decoder)?;
        let value = Self {
            shortcut,
            seat,
            serial,
            time_ns,
            epoch: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ShortcutTriggered {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(28)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.shortcut)?;
        write_u64(encoder, self.seat)?;
        encoder.write_u32(self.serial)?;
        write_u64(encoder, self.time_ns)?;
        encoder.write_u32(self.epoch)
    }
}
