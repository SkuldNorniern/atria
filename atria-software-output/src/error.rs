use std::io;

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

#[derive(Debug)]
pub enum SinkError {
    Io(io::Error),
}

impl From<io::Error> for SinkError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "output sink I/O failed: {error}"),
        }
    }
}

impl std::error::Error for SinkError {}
