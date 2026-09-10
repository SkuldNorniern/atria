//! Little-endian framing and allocation-free primitive codecs from draft §5.

use core::str;

use crate::{DecodeError, EncodeError, ObjectId, Opcode};

pub const HEADER_SIZE: usize = 12;
pub const MAX_MESSAGE_SIZE: usize = 65_532;
pub const MAX_PAYLOAD_SIZE: usize = MAX_MESSAGE_SIZE - HEADER_SIZE;
pub const HANDLE_PLACEHOLDER_BASE: u32 = 0xffff_ff00;
/// Slots a message's handle array can name.
///
/// A slot index is a `u8` and the reserved placeholder range runs to the top of the `u32` space.
/// The assertion below is what keeps those two facts one fact: widening either without the other
/// would leave placeholders that decode to a slot no array can hold, or slots no placeholder can
/// name.
pub const MAX_HANDLES_PER_MESSAGE: usize = u8::MAX as usize + 1;
const _: () =
    assert!(HANDLE_PLACEHOLDER_BASE as usize + MAX_HANDLES_PER_MESSAGE - 1 == u32::MAX as usize);

/// What a handle slot must name.
///
/// The kind is not on the wire. It is a property of the field being decoded — the fence slot of
/// `surface.attach_with_fence` is a fence wherever that message appears — so encoding it would
/// only invite a client to disagree with the message it sent. It travels in [`HandleIndex`] so
/// that the transport, which is the only layer holding the actual object, can refuse a slot whose
/// contents are the wrong kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleKind {
    SharedMemory,
    GpuImage,
    Fence,
}

/// A slot in the message's out-of-band handle array, and what must be in it.
///
/// The protocol does not say what a handle *is*. A Unix transport resolves a slot to a file
/// descriptor received by `SCM_RIGHTS`; an Artery transport resolves it to a capability handle
/// moved through a channel. Naming either here would make the wire format unspeakable on the
/// other system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleIndex {
    index: u8,
    kind: HandleKind,
}

impl HandleIndex {
    #[must_use]
    pub const fn new(index: u8, kind: HandleKind) -> Self {
        Self { index, kind }
    }

    /// Which slot of the message's handle array this names.
    #[must_use]
    pub const fn slot(self) -> u8 {
        self.index
    }

    /// What the transport must find in that slot.
    #[must_use]
    pub const fn kind(self) -> HandleKind {
        self.kind
    }

    #[must_use]
    pub const fn placeholder(self) -> u32 {
        HANDLE_PLACEHOLDER_BASE + self.index as u32
    }

    pub const fn from_placeholder(value: u32, kind: HandleKind) -> Result<Self, DecodeError> {
        if value >= HANDLE_PLACEHOLDER_BASE {
            Ok(Self {
                index: (value - HANDLE_PLACEHOLDER_BASE) as u8,
                kind,
            })
        } else {
            Err(DecodeError::InvalidHandlePlaceholder { value })
        }
    }
}

/// The fixed twelve-byte message header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub object_id: ObjectId,
    pub opcode: Opcode,
    pub message_size: u16,
    pub sequence_num: u32,
}

impl Header {
    pub fn for_payload(
        object_id: ObjectId,
        opcode: Opcode,
        sequence_num: u32,
        payload_size: usize,
    ) -> Result<Self, EncodeError> {
        let message_size = HEADER_SIZE
            .checked_add(payload_size)
            .ok_or(EncodeError::SizeOverflow)?;
        if message_size > MAX_MESSAGE_SIZE {
            return Err(EncodeError::MessageTooLarge {
                size: message_size,
                maximum: MAX_MESSAGE_SIZE,
            });
        }
        if message_size % 4 != 0 {
            return Err(EncodeError::MisalignedSize { size: message_size });
        }
        let message_size = u16::try_from(message_size).map_err(|_| EncodeError::SizeOverflow)?;
        Ok(Self {
            object_id,
            opcode,
            message_size,
            sequence_num,
        })
    }

    pub fn encode(self, output: &mut [u8]) -> Result<(), EncodeError> {
        if output.len() < HEADER_SIZE {
            return Err(EncodeError::BufferTooSmall {
                needed: HEADER_SIZE,
                available: output.len(),
            });
        }
        write_at(output, 0, &self.object_id.into_raw().to_le_bytes())?;
        write_at(output, 4, &self.opcode.into_raw().to_le_bytes())?;
        write_at(output, 6, &self.message_size.to_le_bytes())?;
        write_at(output, 8, &self.sequence_num.to_le_bytes())
    }

    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        if input.len() < HEADER_SIZE {
            return Err(DecodeError::Truncated {
                needed: HEADER_SIZE,
                available: input.len(),
            });
        }
        Ok(Self {
            object_id: ObjectId::from_raw(read_u32_at(input, 0)?),
            opcode: Opcode::from_raw(read_u16_at(input, 4)?),
            message_size: read_u16_at(input, 6)?,
            sequence_num: read_u32_at(input, 8)?,
        })
    }
}

/// A frame borrowing its payload from one complete `SOCK_SEQPACKET` packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame<'a> {
    pub header: Header,
    pub payload: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn decode(packet: &'a [u8]) -> Result<Self, DecodeError> {
        let header = Header::decode(packet)?;
        let declared = usize::from(header.message_size);
        if declared < HEADER_SIZE {
            return Err(DecodeError::MessageTooSmall {
                size: declared,
                minimum: HEADER_SIZE,
            });
        }
        if declared > MAX_MESSAGE_SIZE {
            return Err(DecodeError::MessageTooLarge {
                size: declared,
                maximum: MAX_MESSAGE_SIZE,
            });
        }
        if declared % 4 != 0 {
            return Err(DecodeError::MisalignedSize { size: declared });
        }
        if packet.len() < declared {
            return Err(DecodeError::Truncated {
                needed: declared,
                available: packet.len(),
            });
        }
        if packet.len() > declared {
            return Err(DecodeError::TrailingBytes {
                declared,
                actual: packet.len(),
            });
        }
        let payload = packet
            .get(HEADER_SIZE..declared)
            .ok_or(DecodeError::Truncated {
                needed: declared,
                available: packet.len(),
            })?;
        Ok(Self { header, payload })
    }
}

/// Encodes one already-aligned payload into a complete packet.
pub fn encode_frame(
    object_id: ObjectId,
    opcode: Opcode,
    sequence_num: u32,
    payload: &[u8],
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let header = Header::for_payload(object_id, opcode, sequence_num, payload.len())?;
    let size = usize::from(header.message_size);
    if output.len() < size {
        return Err(EncodeError::BufferTooSmall {
            needed: size,
            available: output.len(),
        });
    }
    header.encode(output)?;
    write_at(output, HEADER_SIZE, payload)?;
    Ok(size)
}

/// Primitive payload writer over a caller-owned buffer.
pub struct Encoder<'a> {
    output: &'a mut [u8],
    position: usize,
}

impl<'a> Encoder<'a> {
    #[must_use]
    pub fn new(output: &'a mut [u8]) -> Self {
        Self {
            output,
            position: 0,
        }
    }

    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    pub fn write_u32(&mut self, value: u32) -> Result<(), EncodeError> {
        self.write(&value.to_le_bytes())
    }

    pub fn write_i32(&mut self, value: i32) -> Result<(), EncodeError> {
        self.write(&value.to_le_bytes())
    }

    pub fn write_handle(&mut self, handle: HandleIndex) -> Result<(), EncodeError> {
        self.write_u32(handle.placeholder())
    }

    /// Writes a u32 element count. Element encoding is interface-specific and the draft does
    /// not define a universal array element width, so callers encode the elements afterward.
    pub fn write_array_len(&mut self, count: u32) -> Result<(), EncodeError> {
        self.write_u32(count)
    }

    pub fn write_string(&mut self, value: &str) -> Result<(), EncodeError> {
        let length = u32::try_from(value.len()).map_err(|_| EncodeError::SizeOverflow)?;
        let padded = padded_size(value.len()).ok_or(EncodeError::SizeOverflow)?;
        self.write_u32(length)?;
        self.write(value.as_bytes())?;
        let padding = padded - value.len();
        // §5 does not prescribe padding-byte values. The encoder chooses zero for stable output;
        // the decoder intentionally does not reject other values.
        const ZEROES: [u8; 3] = [0; 3];
        self.write(&ZEROES[..padding])
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        let end = self
            .position
            .checked_add(bytes.len())
            .ok_or(EncodeError::SizeOverflow)?;
        let available = self.output.len();
        let destination =
            self.output
                .get_mut(self.position..end)
                .ok_or(EncodeError::BufferTooSmall {
                    needed: end,
                    available,
                })?;
        destination.copy_from_slice(bytes);
        self.position = end;
        Ok(())
    }
}

/// Primitive payload reader over untrusted bytes.
pub struct Decoder<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Decoder<'a> {
    #[must_use]
    pub const fn new(input: &'a [u8]) -> Self {
        Self { input, position: 0 }
    }

    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.read(4)?;
        let array = <[u8; 4]>::try_from(bytes).map_err(|_| DecodeError::Truncated {
            needed: 4,
            available: bytes.len(),
        })?;
        Ok(u32::from_le_bytes(array))
    }

    pub fn read_i32(&mut self) -> Result<i32, DecodeError> {
        let bytes = self.read(4)?;
        let array = <[u8; 4]>::try_from(bytes).map_err(|_| DecodeError::Truncated {
            needed: 4,
            available: bytes.len(),
        })?;
        Ok(i32::from_le_bytes(array))
    }

    /// Reads a handle slot, tagged with the kind this field is defined to carry.
    pub fn read_handle(&mut self, kind: HandleKind) -> Result<HandleIndex, DecodeError> {
        HandleIndex::from_placeholder(self.read_u32()?, kind)
    }

    pub fn read_array_len(&mut self) -> Result<u32, DecodeError> {
        self.read_u32()
    }

    pub fn read_string(&mut self) -> Result<&'a str, DecodeError> {
        let length_u32 = self.read_u32()?;
        let length = usize::try_from(length_u32).map_err(|_| DecodeError::SizeOverflow)?;
        let padded = padded_size(length).ok_or(DecodeError::SizeOverflow)?;
        let bytes = self.read(padded)?;
        let value = bytes.get(..length).ok_or(DecodeError::Truncated {
            needed: length,
            available: bytes.len(),
        })?;
        str::from_utf8(value).map_err(|_| DecodeError::InvalidUtf8)
    }

    pub fn finish(self) -> Result<(), DecodeError> {
        if self.position == self.input.len() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes {
                declared: self.position,
                actual: self.input.len(),
            })
        }
    }

    fn read(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(DecodeError::SizeOverflow)?;
        let value = self
            .input
            .get(self.position..end)
            .ok_or(DecodeError::Truncated {
                needed: end,
                available: self.input.len(),
            })?;
        self.position = end;
        Ok(value)
    }
}

#[must_use]
pub const fn padded_size(length: usize) -> Option<usize> {
    let remainder = length % 4;
    let padding = if remainder == 0 { 0 } else { 4 - remainder };
    length.checked_add(padding)
}

fn read_u16_at(input: &[u8], offset: usize) -> Result<u16, DecodeError> {
    let end = offset.checked_add(2).ok_or(DecodeError::SizeOverflow)?;
    let bytes = input.get(offset..end).ok_or(DecodeError::Truncated {
        needed: end,
        available: input.len(),
    })?;
    let array = <[u8; 2]>::try_from(bytes).map_err(|_| DecodeError::Truncated {
        needed: end,
        available: input.len(),
    })?;
    Ok(u16::from_le_bytes(array))
}

fn read_u32_at(input: &[u8], offset: usize) -> Result<u32, DecodeError> {
    let end = offset.checked_add(4).ok_or(DecodeError::SizeOverflow)?;
    let bytes = input.get(offset..end).ok_or(DecodeError::Truncated {
        needed: end,
        available: input.len(),
    })?;
    let array = <[u8; 4]>::try_from(bytes).map_err(|_| DecodeError::Truncated {
        needed: end,
        available: input.len(),
    })?;
    Ok(u32::from_le_bytes(array))
}

fn write_at(output: &mut [u8], offset: usize, bytes: &[u8]) -> Result<(), EncodeError> {
    let end = offset
        .checked_add(bytes.len())
        .ok_or(EncodeError::SizeOverflow)?;
    let available = output.len();
    let destination = output
        .get_mut(offset..end)
        .ok_or(EncodeError::BufferTooSmall {
            needed: end,
            available,
        })?;
    destination.copy_from_slice(bytes);
    Ok(())
}
