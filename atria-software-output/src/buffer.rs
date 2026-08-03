use std::collections::BTreeMap;

use atria_compositor::{BufferDescriptor, BufferTransport, ConnectionId};
use atria_protocol::ObjectId;

use crate::{PixelLayout, ValidationError};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BufferKey {
    pub connection: ConnectionId,
    pub object_id: ObjectId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoftwareBuffer {
    descriptor: BufferDescriptor,
    layout: PixelLayout,
    bytes: Vec<u8>,
}

impl SoftwareBuffer {
    pub fn new(
        descriptor: BufferDescriptor,
        layout: PixelLayout,
        bytes: Vec<u8>,
    ) -> Result<Self, ValidationError> {
        validate_descriptor(descriptor, layout, bytes.len())?;
        Ok(Self {
            descriptor,
            layout,
            bytes,
        })
    }

    #[must_use]
    pub const fn descriptor(&self) -> BufferDescriptor {
        self.descriptor
    }

    #[must_use]
    pub const fn layout(&self) -> PixelLayout {
        self.layout
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, Default)]
pub struct BufferStore {
    buffers: BTreeMap<BufferKey, SoftwareBuffer>,
}

impl BufferStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: BufferKey, buffer: SoftwareBuffer) -> Option<SoftwareBuffer> {
        self.buffers.insert(key, buffer)
    }

    pub fn remove(&mut self, key: BufferKey) -> Option<SoftwareBuffer> {
        self.buffers.remove(&key)
    }

    #[must_use]
    pub fn get(&self, key: BufferKey) -> Option<&SoftwareBuffer> {
        self.buffers.get(&key)
    }
}

fn validate_descriptor(
    descriptor: BufferDescriptor,
    layout: PixelLayout,
    backing_len: usize,
) -> Result<(), ValidationError> {
    if descriptor.transport != BufferTransport::SoftwareShm {
        return Err(ValidationError::UnsupportedTransport(descriptor.transport));
    }
    let width = descriptor.size.width;
    let height = descriptor.size.height;
    if width == 0 || height == 0 {
        return Err(ValidationError::ZeroAreaSurface);
    }
    if descriptor.stride < width {
        return Err(ValidationError::StrideSmallerThanWidth {
            stride: descriptor.stride,
            width,
        });
    }
    let row_bytes = u64::from(width)
        .checked_mul(u64::from(layout.bytes_per_pixel()))
        .ok_or(ValidationError::ArithmeticOverflow)?;
    if u64::from(descriptor.stride) < row_bytes {
        return Err(ValidationError::StrideTooSmallForPixelRow {
            stride: descriptor.stride,
            required: row_bytes,
        });
    }
    let required = u64::from(height - 1)
        .checked_mul(u64::from(descriptor.stride))
        .and_then(|prefix| prefix.checked_add(row_bytes))
        .ok_or(ValidationError::ArithmeticOverflow)?;
    if descriptor.byte_len < required {
        return Err(ValidationError::BufferTooSmallForDeclaredGeometry {
            available: descriptor.byte_len,
            required,
        });
    }
    if u64::try_from(backing_len).map_or(true, |length| length < descriptor.byte_len) {
        return Err(ValidationError::BackingStoreTooSmall {
            available: backing_len,
            declared: descriptor.byte_len,
        });
    }
    Ok(())
}
