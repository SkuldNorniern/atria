//! Composites one frame and writes it as a PPM image.
//!
//! This exists to prove the stack produces pixels end to end — protocol state, buffer
//! validation, composition, and a sink — with no GPU, no display server, and no
//! operating system support beyond writing a file. It is the software path the boot
//! presenter also requires (`plan/atria/boot-presenter.md`).
//!
//! Run with: cargo run --example first-light

use atria_compositor::{
    BufferDescriptor, BufferTransport, ClientRequest, CompositorState, ConnectionLimits,
    NegotiationError, Point, Rect, ServerLimits, Size, StateError, SurfaceKey,
};
use atria_protocol::ObjectId;
use atria_software_output::{
    BufferKey, BufferStore, Frame, FrameReport, FrameSink, PixelLayout, PresentError, SinkError,
    SoftwareBuffer, SoftwareOutput, ValidationError, capabilities,
};
use core::fmt::{self, Display, Formatter};
use std::error::Error;
use std::fs::File;
use std::fs::metadata;
use std::io::Error as IoError;
use std::io::Write;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 200;
const BYTES_PER_PIXEL: u32 = 4;
const OUTPUT_PATH: &str = "first-light.ppm";

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

/// A surface filled with a flat colour, laid out exactly as the descriptor claims.
fn solid_surface(width: u32, height: u32, colour: [u8; 4]) -> (BufferDescriptor, Vec<u8>) {
    let stride = width * BYTES_PER_PIXEL;
    let descriptor = BufferDescriptor {
        transport: BufferTransport::SoftwareShm,
        size: Size { width, height },
        stride,
        byte_len: u64::from(stride) * u64::from(height),
    };
    let mut bytes = Vec::with_capacity((stride * height) as usize);
    for _ in 0..(width * height) {
        bytes.extend_from_slice(&colour);
    }
    (descriptor, bytes)
}

struct PpmSink {
    file: File,
}

impl PpmSink {
    fn create(path: &str) -> Result<Self, SinkError> {
        Ok(Self {
            file: File::create(path)?,
        })
    }
}

impl FrameSink for PpmSink {
    fn present(&mut self, frame: &Frame, _report: FrameReport) -> Result<(), SinkError> {
        let size = frame.size();
        self.file
            .write_all(format!("P6\n{} {}\n255\n", size.width, size.height).as_bytes())?;
        for pixel in frame
            .bytes()
            .chunks_exact(usize::from(frame.layout().bytes_per_pixel()))
        {
            self.file.write_all(&pixel[..3])?;
        }
        self.file.flush()?;
        Ok(())
    }
}

/// Everything this example can fail at, named.
///
/// `Box<dyn Error>` would allocate and erase the type for no benefit here: the failure
/// set is small, closed, and known at compile time. An enum keeps every failure mode
/// visible in the signature, which is the point of enum-based errors — a reader sees what
/// can go wrong without running it, and adding a case is a compile error at every match.
#[derive(Debug)]
enum FirstLightError {
    Layout(ValidationError),
    Negotiation(NegotiationError),
    Request(StateError),
    Present(PresentError),
    Sink(SinkError),
    Io(IoError),
}

impl Display for FirstLightError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Layout(error) => write!(f, "pixel layout or buffer is invalid: {error}"),
            Self::Negotiation(error) => write!(f, "capability negotiation failed: {error}"),
            Self::Request(error) => write!(f, "the compositor rejected a request: {error}"),
            Self::Present(error) => write!(f, "presenting the frame failed: {error}"),
            Self::Sink(error) => write!(f, "the output sink failed: {error}"),
            Self::Io(error) => write!(f, "reading back the written file failed: {error}"),
        }
    }
}

impl Error for FirstLightError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Layout(error) => Some(error),
            Self::Negotiation(error) => Some(error),
            Self::Request(error) => Some(error),
            Self::Present(error) => Some(error),
            Self::Sink(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<IoError> for FirstLightError {
    fn from(error: IoError) -> Self {
        Self::Io(error)
    }
}

impl From<ValidationError> for FirstLightError {
    fn from(error: ValidationError) -> Self {
        Self::Layout(error)
    }
}

impl From<NegotiationError> for FirstLightError {
    fn from(error: NegotiationError) -> Self {
        Self::Negotiation(error)
    }
}

impl From<StateError> for FirstLightError {
    fn from(error: StateError) -> Self {
        Self::Request(error)
    }
}

impl From<PresentError> for FirstLightError {
    fn from(error: PresentError) -> Self {
        Self::Present(error)
    }
}

impl From<SinkError> for FirstLightError {
    fn from(error: SinkError) -> Self {
        Self::Sink(error)
    }
}

fn main() -> Result<(), FirstLightError> {
    let layout = PixelLayout::new(BYTES_PER_PIXEL as u8)?;
    let software_profile = capabilities();
    let mut state = CompositorState::new(
        software_profile,
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let connection = state.connect(software_profile, software_profile)?;
    state.create_session(connection, id(256), None, true)?;

    let mut buffers = BufferStore::default();

    // Two overlapping surfaces, so the frame proves stacking order rather than a single
    // blit. The second is placed down and right of the first.
    for (index, (surface_id, buffer_id, origin, colour)) in [
        (257_u32, 300_u32, Point { x: 20, y: 20 }, [40, 90, 160, 255]),
        (258, 301, Point { x: 120, y: 80 }, [200, 120, 40, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        state.dispatch(
            connection,
            ClientRequest::CreateSurface {
                session: id(256),
                new_id: id(surface_id),
            },
        )?;
        state.place_surface(
            SurfaceKey {
                connection,
                object_id: id(surface_id),
            },
            origin,
        )?;

        let (descriptor, bytes) = solid_surface(160, 100, colour);
        state.dispatch(
            connection,
            ClientRequest::ImportBuffer {
                new_id: id(buffer_id),
                descriptor,
            },
        )?;
        buffers.insert(
            BufferKey {
                connection,
                object_id: id(buffer_id),
            },
            SoftwareBuffer::new(descriptor, layout, bytes)?,
        );
        state.dispatch(
            connection,
            ClientRequest::Attach {
                surface: id(surface_id),
                buffer: id(buffer_id),
                offset: Point::default(),
                // The software profile does not advertise explicit fences. Per ADR-0012,
                // absent capabilities are negotiated rather than emulated.
                acquire_fence: None,
            },
        )?;
        state.dispatch(
            connection,
            ClientRequest::Damage {
                surface: id(surface_id),
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 160,
                    height: 100,
                },
            },
        )?;
        state.dispatch(
            connection,
            ClientRequest::Commit {
                surface: id(surface_id),
            },
        )?;
        println!("surface {} committed ({} of 2)", surface_id, index + 1);
    }

    let mut output = SoftwareOutput::new(
        Size {
            width: WIDTH,
            height: HEIGHT,
        },
        layout,
    )?;
    let mut sink = PpmSink::create(OUTPUT_PATH)?;
    let report = output.present_to(&mut state, &buffers, 0, &mut sink)?;

    println!(
        "FrameReport: timestamp_ns={}, surfaces_composited={}, commits_consumed={}, damage_rectangles_consumed={}, bytes_written={}",
        report.timestamp_ns,
        report.surfaces_composited,
        report.commits_consumed,
        report.damage_rectangles_consumed,
        report.bytes_written
    );
    println!(
        "wrote {OUTPUT_PATH} ({}x{}, {} bytes)",
        WIDTH,
        HEIGHT,
        metadata(OUTPUT_PATH)?.len()
    );
    Ok(())
}
