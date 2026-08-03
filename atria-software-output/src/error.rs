use std::io;
use std::{error, fmt};

use atria_compositor::{BufferTransport, CommitId, SurfaceKey};

use crate::{BufferKey, PixelLayout};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    UnsupportedTransport(BufferTransport),
    ZeroAreaSurface,
    ZeroBytesPerPixel,
    StrideSmallerThanWidth { stride: u32, width: u32 },
    StrideTooSmallForPixelRow { stride: u32, required: u64 },
    BufferTooSmallForDeclaredGeometry { available: u64, required: u64 },
    BackingStoreTooSmall { available: usize, declared: u64 },
    ArithmeticOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposeError {
    InvalidOutput(ValidationError),
    MissingSurfaceState(SurfaceKey),
    MissingSurfacePosition(SurfaceKey),
    MissingBuffer(BufferKey),
    MissingBufferDescriptor(BufferKey),
    BufferDescriptorMismatch(BufferKey),
    PixelLayoutMismatch {
        buffer: BufferKey,
        expected: PixelLayout,
        actual: PixelLayout,
    },
    BufferNotHeld {
        buffer: BufferKey,
        surface: SurfaceKey,
        commit: CommitId,
    },
    CommitNotReady(SurfaceKey),
    InvalidBuffer {
        buffer: BufferKey,
        error: ValidationError,
    },
    DamageRectangleOutsideSurface {
        surface: SurfaceKey,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    GeometryOverflow(SurfaceKey),
    StateChanged(SurfaceKey),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTransport(transport) => write!(
                formatter,
                "software output cannot import {transport:?} buffers; use SoftwareShm"
            ),
            Self::ZeroAreaSurface => {
                formatter.write_str("surface dimensions must both be greater than zero")
            }
            Self::ZeroBytesPerPixel => {
                formatter.write_str("pixel layout must use at least one byte per pixel")
            }
            Self::StrideSmallerThanWidth { stride, width } => write!(
                formatter,
                "buffer stride {stride} bytes is smaller than its width of {width} pixels"
            ),
            Self::StrideTooSmallForPixelRow { stride, required } => write!(
                formatter,
                "buffer stride {stride} bytes cannot hold the required {required}-byte pixel row"
            ),
            Self::BufferTooSmallForDeclaredGeometry {
                available,
                required,
            } => write!(
                formatter,
                "buffer declares {available} bytes, but its geometry requires at least {required} bytes"
            ),
            Self::BackingStoreTooSmall {
                available,
                declared,
            } => write!(
                formatter,
                "buffer backing store has {available} bytes, fewer than the declared {declared} bytes"
            ),
            Self::ArithmeticOverflow => formatter.write_str(
                "buffer or frame geometry is too large to represent safely on this platform",
            ),
        }
    }
}

impl error::Error for ValidationError {}

impl fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOutput(error) => write!(formatter, "output frame is invalid: {error}"),
            Self::MissingSurfaceState(surface) => write!(
                formatter,
                "surface {} on connection {} disappeared while composing",
                surface.object_id.into_raw(),
                surface.connection.0
            ),
            Self::MissingSurfacePosition(surface) => write!(
                formatter,
                "surface {} on connection {} has no scene position",
                surface.object_id.into_raw(),
                surface.connection.0
            ),
            Self::MissingBuffer(buffer) => write!(
                formatter,
                "software buffer {} on connection {} was not supplied to the output",
                buffer.object_id.into_raw(),
                buffer.connection.0
            ),
            Self::MissingBufferDescriptor(buffer) => write!(
                formatter,
                "buffer {} on connection {} has no compositor descriptor",
                buffer.object_id.into_raw(),
                buffer.connection.0
            ),
            Self::BufferDescriptorMismatch(buffer) => write!(
                formatter,
                "software buffer {} on connection {} does not match its compositor descriptor",
                buffer.object_id.into_raw(),
                buffer.connection.0
            ),
            Self::PixelLayoutMismatch {
                buffer,
                expected,
                actual,
            } => write!(
                formatter,
                "buffer {} on connection {} uses {} bytes per pixel, but the output requires {}",
                buffer.object_id.into_raw(),
                buffer.connection.0,
                actual.bytes_per_pixel(),
                expected.bytes_per_pixel()
            ),
            Self::BufferNotHeld {
                buffer,
                surface,
                commit,
            } => write!(
                formatter,
                "buffer {} on connection {} is not held for surface {} commit {}",
                buffer.object_id.into_raw(),
                buffer.connection.0,
                surface.object_id.into_raw(),
                commit.0
            ),
            Self::CommitNotReady(surface) => write!(
                formatter,
                "surface {} on connection {} has an unsignaled acquire fence",
                surface.object_id.into_raw(),
                surface.connection.0
            ),
            Self::InvalidBuffer { buffer, error } => write!(
                formatter,
                "buffer {} on connection {} is invalid: {error}",
                buffer.object_id.into_raw(),
                buffer.connection.0
            ),
            Self::DamageRectangleOutsideSurface {
                surface,
                x,
                y,
                width,
                height,
            } => write!(
                formatter,
                "damage rectangle ({x}, {y}, {width}x{height}) extends outside surface {} on connection {}",
                surface.object_id.into_raw(),
                surface.connection.0
            ),
            Self::GeometryOverflow(surface) => write!(
                formatter,
                "geometry for surface {} on connection {} exceeds representable coordinates",
                surface.object_id.into_raw(),
                surface.connection.0
            ),
            Self::StateChanged(surface) => write!(
                formatter,
                "surface {} on connection {} changed while its frame was being composed",
                surface.object_id.into_raw(),
                surface.connection.0
            ),
        }
    }
}

impl error::Error for ComposeError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::InvalidOutput(error) | Self::InvalidBuffer { error, .. } => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum SinkError {
    Io(io::Error),
}

impl From<io::Error> for SinkError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl fmt::Display for SinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "output sink I/O failed: {error}"),
        }
    }
}

impl error::Error for SinkError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
        }
    }
}
