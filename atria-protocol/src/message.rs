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

/// A shortcut, by the name its holder gave it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutName {
    pub shortcut: u32,
}

impl ShortcutName {
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the payload is not one name.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut decoder = Decoder::new(input);
        let value = Self {
            shortcut: decoder.read_u32()?,
        };
        decoder.finish()?;
        Ok(value)
    }
}

impl EncodePayload for ShortcutName {
    fn encoded_len(&self) -> Result<usize, EncodeError> {
        Ok(4)
    }

    fn encode(&self, encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
        encoder.write_u32(self.shortcut)
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
