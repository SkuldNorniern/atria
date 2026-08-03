//! Presents a moving test pattern through `/dev/dri/card0`.
//!
//! This requires DRM master access to real KMS hardware:
//! `cargo run -p atria-drm --example hardware-first-light`.
//!
//! Repeated presentation is the point: a single frame exercises mode setting only, while a
//! sequence exercises the flip path, its completion events, and the rule that a flip is never
//! queued while one is pending. `ARTERY_FRAMES` bounds the run for unattended use; without it
//! the pattern runs until the process is terminated, so a human can look at it.

use std::env::var;
use std::error::Error;
use std::fmt;

use atria_compositor::{
    BufferDescriptor, BufferTransport, ClientRequest, CompositorState, ConnectionLimits,
    NegotiationError, Point, Rect, Size, StateError, SurfaceKey,
};
use atria_drm::{DeviceConfig, DrmError, DrmSink};
use atria_protocol::ObjectId;
use atria_software_output::{
    BufferKey, BufferStore, ComposeError, PixelLayout, SoftwareBuffer, SoftwareOutput,
    ValidationError, capabilities as software_capabilities,
};

#[derive(Debug)]
enum HardwareFirstLightError {
    Drm(DrmError),
    Layout(ValidationError),
    Negotiation(NegotiationError),
    State(StateError),
    Compose(ComposeError),
}

impl fmt::Display for HardwareFirstLightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Drm(error) => write!(formatter, "DRM first light failed: {error}"),
            Self::Layout(error) => write!(formatter, "test-pattern layout is invalid: {error}"),
            Self::Negotiation(error) => write!(formatter, "capability negotiation failed: {error}"),
            Self::State(error) => write!(formatter, "compositor state rejected the frame: {error}"),
            Self::Compose(error) => {
                write!(formatter, "compositing the test pattern failed: {error}")
            }
        }
    }
}

impl Error for HardwareFirstLightError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Drm(error) => Some(error),
            Self::Layout(error) => Some(error),
            Self::Negotiation(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Compose(error) => Some(error),
        }
    }
}

impl From<DrmError> for HardwareFirstLightError {
    fn from(error: DrmError) -> Self {
        Self::Drm(error)
    }
}

impl From<ValidationError> for HardwareFirstLightError {
    fn from(error: ValidationError) -> Self {
        Self::Layout(error)
    }
}

impl From<NegotiationError> for HardwareFirstLightError {
    fn from(error: NegotiationError) -> Self {
        Self::Negotiation(error)
    }
}

impl From<StateError> for HardwareFirstLightError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<ComposeError> for HardwareFirstLightError {
    fn from(error: ComposeError) -> Self {
        Self::Compose(error)
    }
}

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn test_pattern(size: Size, phase: u32) -> Result<Vec<u8>, DrmError> {
    let pixel_count = u64::from(size.width)
        .checked_mul(u64::from(size.height))
        .ok_or(DrmError::ArithmeticOverflow)?;
    let pixel_count = usize::try_from(pixel_count).map_err(|_| DrmError::ArithmeticOverflow)?;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or(DrmError::ArithmeticOverflow)?;
    let mut bytes = Vec::with_capacity(capacity);
    for y in 0..size.height {
        for x in 0..size.width {
            let red = ((u64::from(x) * 255) / u64::from(size.width)) as u8;
            let green = ((u64::from(y) * 255) / u64::from(size.height)) as u8;
            // Scroll the checkerboard horizontally rather than inverting it. Inverting the
            // whole field every few frames reads as a strobe, which is indistinguishable
            // from a fault; motion shows that frames are advancing and that they are not
            // tearing.
            let blue = if (((x + phase * 2) / 64) + (y / 64)) % 2 == 0 {
                48
            } else {
                192
            };
            // XRGB8888 is B, G, R, unused in little-endian byte order.
            bytes.extend_from_slice(&[blue, green, red, 0]);
        }
    }
    Ok(bytes)
}

fn main() -> Result<(), HardwareFirstLightError> {
    let layout = PixelLayout::new(4)?;
    let mut sink = DrmSink::open(DeviceConfig::default(), layout)?;
    let mode = sink.mode();
    let size = Size {
        width: u32::from(mode.width),
        height: u32::from(mode.height),
    };
    let stride = size
        .width
        .checked_mul(4)
        .ok_or(DrmError::ArithmeticOverflow)?;
    let byte_len = u64::from(stride)
        .checked_mul(u64::from(size.height))
        .ok_or(DrmError::ArithmeticOverflow)?;
    let descriptor = BufferDescriptor {
        transport: BufferTransport::SoftwareShm,
        size,
        stride,
        byte_len,
    };

    let capabilities = software_capabilities();
    let mut state = CompositorState::new(capabilities, ConnectionLimits::default());
    let connection = state.connect(capabilities, capabilities)?;
    state.create_session(connection, id(256), None, true)?;
    state.dispatch(
        connection,
        ClientRequest::CreateSurface {
            session: id(256),
            new_id: id(257),
        },
    )?;
    let surface = SurfaceKey {
        connection,
        object_id: id(257),
    };
    state.place_surface(surface, Point::default())?;
    state.dispatch(
        connection,
        ClientRequest::ImportBuffer {
            new_id: id(300),
            descriptor,
        },
    )?;
    let mut buffers = BufferStore::new();
    let _ = buffers.insert(
        BufferKey {
            connection,
            object_id: id(300),
        },
        SoftwareBuffer::new(descriptor, layout, test_pattern(size, 0)?)?,
    );
    let mut output = SoftwareOutput::new(size, layout)?;
    let frame_limit = match var("ARTERY_FRAMES") {
        Ok(value) => value.parse::<u64>().ok(),
        Err(_) => None,
    };

    let buffer_key = BufferKey {
        connection,
        object_id: id(300),
    };
    let mut phase = 0u32;
    let mut presented = 0u64;
    loop {
        let _ = buffers.insert(
            buffer_key,
            SoftwareBuffer::new(descriptor, layout, test_pattern(size, phase)?)?,
        );
        // Each frame is a full attach/damage/commit cycle: a commit consumes the attached
        // buffer, so damage alone does not make a new frame.
        state.dispatch(
            connection,
            ClientRequest::Attach {
                surface: id(257),
                buffer: id(300),
                offset: Point::default(),
                acquire_fence: None,
            },
        )?;
        state.dispatch(
            connection,
            ClientRequest::Damage {
                surface: id(257),
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: size.width,
                    height: size.height,
                },
            },
        )?;
        state.dispatch(connection, ClientRequest::Commit { surface: id(257) })?;

        let report = output.compose(&mut state, &buffers, presented)?;
        sink.present(output.frame(), report)?;
        // Waiting for the completion before the next present is the backpressure rule: the
        // sink refuses a queued flip while one is in flight, so the loop cannot outrun the
        // display.
        sink.complete_flip()?;
        // No explicit release: presenting the next commit supersedes the previous one and
        // returns the buffer to the client, so the buffer is Available again by the time the
        // next attach runs. Releasing here would be a second release of an available buffer.

        presented += 1;
        phase = phase.wrapping_add(1);
        if frame_limit.is_some_and(|limit| presented >= limit) {
            println!("presented {presented} frames");
            return Ok(());
        }
    }
}
