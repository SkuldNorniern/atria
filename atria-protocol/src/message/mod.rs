//! Payloads whose field layouts are explicitly specified by §§10, 12, and 17.

use crate::error::ErrorCode;
use crate::wire::{Decoder, Encoder, HEADER_SIZE, HandleIndex, HandleKind, Header, padded_size};
use crate::{DecodeError, EncodeError, ObjectId, Opcode};

/// An encodable protocol payload.
pub trait EncodePayload {
    fn encoded_len(&self) -> Result<usize, EncodeError>;
    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError>;
}

/// Encodes a typed payload and its header into a complete packet.
pub fn encode_message(
    object_id: ObjectId,
    opcode: Opcode,
    sequence_num: u32,
    payload: &impl EncodePayload,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let payload_len = payload.encoded_len()?;
    let header = Header::for_payload(object_id, opcode, sequence_num, payload_len)?;
    let message_len = usize::from(header.message_size);
    if output.len() < message_len {
        return Err(EncodeError::BufferTooSmall {
            needed: message_len,
            available: output.len(),
        });
    }
    header.encode(output)?;
    let available = output.len();
    let payload_output =
        output
            .get_mut(HEADER_SIZE..message_len)
            .ok_or(EncodeError::BufferTooSmall {
                needed: message_len,
                available,
            })?;
    let mut encoder = Encoder::new(payload_output);
    payload.encode(&mut encoder)?;
    if encoder.position() != payload_len {
        return Err(EncodeError::EncodedLengthMismatch {
            declared: payload_len,
            actual: encoder.position(),
        });
    }
    Ok(message_len)
}

/// Payload of `connect_request(min_version, max_version)` from §12.
///
/// The draft does not assign this handshake a target object or opcode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectRequest {
    pub min_version: u32,
    pub max_version: u32,
}

impl ConnectRequest {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            min_version: decoder.read_u32()?,
            max_version: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ConnectRequest {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.min_version)?;
        encoder.write_u32(self.max_version)
    }
}

/// Payload of `connect_ack(negotiated_version)` from §12.
///
/// The draft does not assign this handshake a target object or opcode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectAck {
    pub negotiated_version: u32,
}

impl ConnectAck {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            negotiated_version: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ConnectAck {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.negotiated_version)
    }
}

/// `display.get_registry` payload assigned in wire example 1.
///
/// The example uses `new_id = 2`, although §4 says client-allocated objects start at 256.
/// Decoding preserves the ID; enforcing object allocation and liveness belongs to the
/// connection's object table, which this wire-only package intentionally does not invent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GetRegistry {
    pub new_id: ObjectId,
}

impl GetRegistry {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            new_id: ObjectId::from_raw(decoder.read_u32()?),
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for GetRegistry {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.new_id.into_raw())
    }
}

/// `registry.global` payload assigned in wire example 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistryGlobal<'a> {
    pub name: u32,
    pub interface: &'a str,
    pub version: u32,
}

impl<'a> RegistryGlobal<'a> {
    pub fn decode(input: &'a [u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let name = decoder.read_u32()?;
        let interface = decoder.read_string()?;
        let version = decoder.read_u32()?;
        decoder.finish()?;
        Ok(Self {
            name,
            interface,
            version,
        })
    }
}

impl EncodePayload for RegistryGlobal<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        8_usize
            .checked_add(encoded_string_len(self.interface)?)
            .ok_or(EncodeError::SizeOverflow)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.name)?;
        encoder.write_string(self.interface)?;
        encoder.write_u32(self.version)
    }
}

/// `atria_compositor.create_surface` and any other message whose payload is one new identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewId {
    pub new_id: ObjectId,
}

impl NewId {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            new_id: ObjectId::from_raw(decoder.read_u32()?),
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for NewId {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.new_id.into_raw())
    }
}

/// `atria_shm_pool.create_buffer` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateBuffer {
    pub new_id: ObjectId,
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
}

impl CreateBuffer {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            new_id: ObjectId::from_raw(decoder.read_u32()?),
            offset: decoder.read_u32()?,
            width: decoder.read_u32()?,
            height: decoder.read_u32()?,
            stride: decoder.read_u32()?,
            format: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for CreateBuffer {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(24)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.new_id.into_raw())?;
        encoder.write_u32(self.offset)?;
        encoder.write_u32(self.width)?;
        encoder.write_u32(self.height)?;
        encoder.write_u32(self.stride)?;
        encoder.write_u32(self.format)
    }
}

/// `atria_surface.attach` payload. The buffer is null to unmap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Attach {
    pub buffer: ObjectId,
    pub x_offset: i32,
    pub y_offset: i32,
}

impl Attach {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            buffer: ObjectId::from_raw(decoder.read_u32()?),
            x_offset: decoder.read_i32()?,
            y_offset: decoder.read_i32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for Attach {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.buffer.into_raw())?;
        encoder.write_i32(self.x_offset)?;
        encoder.write_i32(self.y_offset)
    }
}

/// `atria_surface.damage_buffer` payload, in buffer pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DamageBuffer {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DamageBuffer {
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

impl EncodePayload for DamageBuffer {
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

/// `atria_surface.commit` payload. `configure_serial` is zero when answering no configure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Commit {
    pub commit_id: u64,
    pub configure_serial: u32,
}

impl Commit {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let low = u64::from(decoder.read_u32()?);
        let high = u64::from(decoder.read_u32()?);
        let value = Self {
            commit_id: low | (high << 32),
            configure_serial: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for Commit {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.commit_id as u32)?;
        encoder.write_u32((self.commit_id >> 32) as u32)?;
        encoder.write_u32(self.configure_serial)
    }
}

/// `atria_registry.global_remove` payload, and any other message carrying one global name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalName {
    pub name: u32,
}

impl GlobalName {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            name: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for GlobalName {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.name)
    }
}

/// `atria_registry.bind` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bind {
    pub name: u32,
    pub version: u32,
    pub new_id: ObjectId,
}

impl Bind {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            name: decoder.read_u32()?,
            version: decoder.read_u32()?,
            new_id: ObjectId::from_raw(decoder.read_u32()?),
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for Bind {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.name)?;
        encoder.write_u32(self.version)?;
        encoder.write_u32(self.new_id.into_raw())
    }
}

/// Bytes a title may occupy.
///
/// A title is client-supplied text the compositor retains for as long as the window exists, so it
/// is a resource a client can grow. Bounded here rather than by whatever the message ceiling
/// allows, because a 64 KiB window title is not a title.
pub const MAX_TITLE_BYTES: usize = 256;

/// `atria_shell.get_toplevel` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GetToplevel {
    pub surface: ObjectId,
    pub new_id: ObjectId,
}

impl GetToplevel {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            surface: ObjectId::from_raw(decoder.read_u32()?),
            new_id: ObjectId::from_raw(decoder.read_u32()?),
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for GetToplevel {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.surface.into_raw())?;
        encoder.write_u32(self.new_id.into_raw())
    }
}

/// `atria_toplevel.set_title` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetTitle<'a> {
    pub title: &'a str,
}

impl<'a> SetTitle<'a> {
    pub fn decode(input: &'a [u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let title = decoder.read_string()?;
        decoder.finish()?;
        Ok(Self { title })
    }
}

impl EncodePayload for SetTitle<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4 + padded_size(self.title.len()).ok_or(EncodeError::SizeOverflow)?)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_string(self.title)
    }
}

/// `atria_toplevel.set_min_size` and `set_max_size` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SizeHint {
    pub width: u32,
    pub height: u32,
}

impl SizeHint {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            width: decoder.read_u32()?,
            height: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for SizeHint {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(8)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.width)?;
        encoder.write_u32(self.height)
    }
}

/// `atria_toplevel.configure` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Configure {
    pub serial: u32,
    pub width: u32,
    pub height: u32,
    pub state: u32,
}

impl Configure {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            serial: decoder.read_u32()?,
            width: decoder.read_u32()?,
            height: decoder.read_u32()?,
            state: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for Configure {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.width)?;
        encoder.write_u32(self.height)?;
        encoder.write_u32(self.state)
    }
}

/// `atria_surface.frame_done` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameDone {
    pub serial: u32,
    pub timestamp_ns: u64,
}

impl FrameDone {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let serial = decoder.read_u32()?;
        let low = u64::from(decoder.read_u32()?);
        let high = u64::from(decoder.read_u32()?);
        decoder.finish()?;
        Ok(Self {
            serial,
            timestamp_ns: low | (high << 32),
        })
    }
}

impl EncodePayload for FrameDone {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.serial)?;
        encoder.write_u32(self.timestamp_ns as u32)?;
        encoder.write_u32((self.timestamp_ns >> 32) as u32)
    }
}

/// `atria_shm.create_pool` payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreatePool {
    pub new_id: ObjectId,
    pub memory: HandleIndex,
    /// Bytes of the resource this pool covers. Checked against the resource at resolution.
    pub size: u32,
}

impl CreatePool {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            new_id: ObjectId::from_raw(decoder.read_u32()?),
            memory: decoder.read_handle(HandleKind::SharedMemory)?,
            size: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for CreatePool {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(12)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.new_id.into_raw())?;
        encoder.write_handle(self.memory)?;
        encoder.write_u32(self.size)
    }
}

/// `surface.attach_with_fence` payload assigned in wire example 2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachWithFence {
    pub buffer_id: ObjectId,
    pub fence: HandleIndex,
    pub x_offset: i32,
    pub y_offset: i32,
}

impl AttachWithFence {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            buffer_id: ObjectId::from_raw(decoder.read_u32()?),
            fence: decoder.read_handle(HandleKind::Fence)?,
            x_offset: decoder.read_i32()?,
            y_offset: decoder.read_i32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for AttachWithFence {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(16)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.buffer_id.into_raw())?;
        encoder.write_handle(self.fence)?;
        encoder.write_i32(self.x_offset)?;
        encoder.write_i32(self.y_offset)
    }
}

/// `buffer.release_with_fence` payload assigned in wire example 2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseWithFence {
    pub fence: HandleIndex,
}

impl ReleaseWithFence {
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            fence: decoder.read_handle(HandleKind::Fence)?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ReleaseWithFence {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_handle(self.fence)
    }
}

/// `display.error(object_id, code, message_string)` payload from §10.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayError<'a> {
    pub object_id: ObjectId,
    pub code: ErrorCode,
    pub message: &'a str,
}

impl<'a> DisplayError<'a> {
    pub fn decode(input: &'a [u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let object_id = ObjectId::from_raw(decoder.read_u32()?);
        let code = ErrorCode::from_raw(decoder.read_u32()?);
        let message = decoder.read_string()?;
        decoder.finish()?;
        Ok(Self {
            object_id,
            code,
            message,
        })
    }
}

impl EncodePayload for DisplayError<'_> {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        8_usize
            .checked_add(encoded_string_len(self.message)?)
            .ok_or(EncodeError::SizeOverflow)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.object_id.into_raw())?;
        encoder.write_u32(self.code.into_raw())?;
        encoder.write_string(self.message)
    }
}

fn encoded_string_len(value: &str) -> Result<usize, EncodeError> {
    let padded = padded_size(value.len()).ok_or(EncodeError::SizeOverflow)?;
    4_usize.checked_add(padded).ok_or(EncodeError::SizeOverflow)
}

pub mod input;
pub mod output;
pub mod shell;
pub mod text;

pub use input::{
    KeyboardEnter, KeyboardKey, KeyboardLeave, KeyboardModifiers, Number, PointerAxis,
    PointerButton, PointerEnter, PointerLeave, PointerMotion, SeatHandle, SeatName, SeatPoint,
    ShellInteraction, ShortcutRegistration, ShortcutTriggered,
};
pub use output::{OutputGeometry, OutputIdentity, OutputMode, OutputScale};
pub use shell::{ShellConfigure, ShellHandle, ShellPlace, ShellToplevel};
pub use text::{
    CommitText, CompositionRound, CursorArea, InputMethodActivation, MAX_TEXT_BYTES, Preedit,
    TextPurpose,
};

/// Reading a 64-bit value as the low half then the high, matching how the wire carries one.
fn read_u64(decoder: &mut Decoder<'_>) -> Result<u64, DecodeError> {
    let low = u64::from(decoder.read_u32()?);
    let high = u64::from(decoder.read_u32()?);
    Ok(low | (high << 32))
}

fn write_u64(encoder: &mut Encoder<'_>, value: u64) -> Result<(), EncodeError> {
    encoder.write_u32(value as u32)?;
    encoder.write_u32((value >> 32) as u32)
}
