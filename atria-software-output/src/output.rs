use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::mem::swap;

use atria_compositor::{
    BufferDescriptor, BufferState, BufferTransport, CommitId, CompositorState, Damage, Point, Rect,
    Size, SurfaceKey,
};

use crate::{
    BufferKey, BufferStore, ComposeError, Frame, FrameSink, PixelLayout, SinkError, ValidationError,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameReport {
    pub timestamp_ns: u64,
    pub surfaces_composited: usize,
    pub commits_consumed: usize,
    pub damage_rectangles_consumed: usize,
    pub bytes_written: usize,
}

#[derive(Clone, Debug)]
struct SurfaceImage {
    commit: CommitId,
    size: Size,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SoftwareOutput {
    frame: Frame,
    /// Composed into, then swapped with `frame`. Two lasting allocations rather than one per
    /// frame, which at 1280x720 is 3.5 MB of churn per pointer movement.
    spare: Frame,
    surfaces: BTreeMap<SurfaceKey, SurfaceImage>,
}

impl SoftwareOutput {
    pub fn new(size: Size, layout: PixelLayout) -> Result<Self, ValidationError> {
        Ok(Self {
            frame: Frame::new(size, layout)?,
            spare: Frame::new(size, layout)?,
            surfaces: BTreeMap::new(),
        })
    }

    #[must_use]
    pub const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Composites one complete frame and releases each newly consumed client buffer.
    ///
    /// Client bytes are first copied into compositor-owned surface images. The corresponding
    /// buffer release occurs only after all reads have finished, and later frames use only the
    /// owned copy. This keeps ADR-0005's ownership boundary even without process isolation.
    pub fn compose(
        &mut self,
        state: &mut CompositorState,
        buffers: &BufferStore,
        timestamp_ns: u64,
    ) -> Result<FrameReport, ComposeError> {
        let mut staged_surfaces = self.surfaces.clone();
        let mut active = BTreeSet::new();
        let mut consumed = Vec::new();
        let mut damage_count = 0_usize;

        for &surface in state.stacking_order() {
            active.insert(surface);
            let snapshot = state
                .surface_snapshot(surface.connection, surface.object_id)
                .cloned()
                .ok_or(ComposeError::MissingSurfaceState(surface))?;
            let Some(position) = state.surface_position(surface) else {
                return Err(ComposeError::MissingSurfacePosition(surface));
            };
            checked_origin(surface, position, snapshot.offset)?;

            if staged_surfaces
                .get(&surface)
                .is_some_and(|image| image.commit == snapshot.commit)
            {
                continue;
            }
            if !state
                .commit_ready(surface)
                .map_err(|_| ComposeError::StateChanged(surface))?
            {
                return Err(ComposeError::CommitNotReady(surface));
            }
            let buffer_key = BufferKey {
                connection: surface.connection,
                object_id: snapshot.buffer,
            };
            let descriptor = state
                .buffer_descriptor(surface.connection, snapshot.buffer)
                .ok_or(ComposeError::MissingBufferDescriptor(buffer_key))?;
            let buffer = buffers
                .get(buffer_key)
                .ok_or(ComposeError::MissingBuffer(buffer_key))?;
            if buffer.descriptor() != descriptor {
                return Err(ComposeError::BufferDescriptorMismatch(buffer_key));
            }
            if buffer.layout() != self.frame.layout() {
                return Err(ComposeError::PixelLayoutMismatch {
                    buffer: buffer_key,
                    expected: self.frame.layout(),
                    actual: buffer.layout(),
                });
            }
            match state.buffer_state(surface.connection, snapshot.buffer) {
                Some(BufferState::CompositorHeld {
                    surface: held_surface,
                    commit,
                }) if held_surface == surface && commit == snapshot.commit => {}
                _ => {
                    return Err(ComposeError::BufferNotHeld {
                        buffer: buffer_key,
                        surface,
                        commit: snapshot.commit,
                    });
                }
            }
            validate_for_layout(descriptor, self.frame.layout(), buffer.bytes().len()).map_err(
                |error| ComposeError::InvalidBuffer {
                    buffer: buffer_key,
                    error,
                },
            )?;
            validate_damage(surface, descriptor.size, &snapshot.damage)?;

            let replace_all = staged_surfaces
                .get(&surface)
                .is_none_or(|image| image.size != descriptor.size);
            let byte_len =
                tight_byte_len(descriptor.size, self.frame.layout()).map_err(|error| {
                    ComposeError::InvalidBuffer {
                        buffer: buffer_key,
                        error,
                    }
                })?;
            let image = staged_surfaces
                .entry(surface)
                .or_insert_with(|| SurfaceImage {
                    commit: snapshot.commit,
                    size: descriptor.size,
                    bytes: vec![0; byte_len],
                });
            if replace_all {
                image.size = descriptor.size;
                image.bytes.resize(byte_len, 0);
            }
            if replace_all {
                copy_rect(
                    buffer.bytes(),
                    descriptor.stride,
                    &mut image.bytes,
                    tight_stride(descriptor.size, self.frame.layout())?,
                    self.frame.layout(),
                    Rect {
                        x: 0,
                        y: 0,
                        width: descriptor.size.width,
                        height: descriptor.size.height,
                    },
                )?;
            } else {
                for damage in &snapshot.damage {
                    let rect = match *damage {
                        Damage::Full => Rect {
                            x: 0,
                            y: 0,
                            width: descriptor.size.width,
                            height: descriptor.size.height,
                        },
                        Damage::Rect(rect) => rect,
                    };
                    copy_rect(
                        buffer.bytes(),
                        descriptor.stride,
                        &mut image.bytes,
                        tight_stride(descriptor.size, self.frame.layout())?,
                        self.frame.layout(),
                        rect,
                    )?;
                }
            }
            image.commit = snapshot.commit;
            damage_count = damage_count.saturating_add(snapshot.damage.len());
            consumed.push((surface, snapshot.buffer));
        }

        staged_surfaces.retain(|surface, _| active.contains(surface));
        // Into the kept frame, then swapped in.
        self.spare.clear();
        for &surface in state.stacking_order() {
            let snapshot = state
                .surface_snapshot(surface.connection, surface.object_id)
                .ok_or(ComposeError::MissingSurfaceState(surface))?;
            let position = state
                .surface_position(surface)
                .ok_or(ComposeError::MissingSurfacePosition(surface))?;
            let image = staged_surfaces
                .get(&surface)
                .ok_or(ComposeError::MissingSurfaceState(surface))?;
            blit_surface(surface, image, position, snapshot.offset, &mut self.spare)?;
        }

        for (surface, buffer) in &consumed {
            state
                .present(*surface, timestamp_ns)
                .map_err(|_| ComposeError::StateChanged(*surface))?;
            state
                .release_buffer(surface.connection, *buffer)
                .map_err(|_| ComposeError::StateChanged(*surface))?;
        }

        let report = FrameReport {
            timestamp_ns,
            surfaces_composited: staged_surfaces.len(),
            commits_consumed: consumed.len(),
            damage_rectangles_consumed: damage_count,
            bytes_written: self.spare.bytes().len(),
        };
        self.surfaces = staged_surfaces;
        swap(&mut self.frame, &mut self.spare);
        Ok(report)
    }

    pub fn present_to(
        &mut self,
        state: &mut CompositorState,
        buffers: &BufferStore,
        timestamp_ns: u64,
        sink: &mut impl FrameSink,
    ) -> Result<FrameReport, PresentError> {
        let report = self
            .compose(state, buffers, timestamp_ns)
            .map_err(PresentError::Compose)?;
        sink.present(&self.frame, report)
            .map_err(PresentError::Sink)?;
        Ok(report)
    }
}

#[derive(Debug)]
pub enum PresentError {
    Compose(ComposeError),
    Sink(SinkError),
}

impl fmt::Display for PresentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compose(error) => write!(formatter, "frame composition failed: {error}"),
            Self::Sink(error) => write!(formatter, "frame presentation failed: {error}"),
        }
    }
}

impl Error for PresentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Compose(error) => Some(error),
            Self::Sink(error) => Some(error),
        }
    }
}

fn validate_for_layout(
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
    let row = u64::from(width)
        .checked_mul(u64::from(layout.bytes_per_pixel()))
        .ok_or(ValidationError::ArithmeticOverflow)?;
    if u64::from(descriptor.stride) < row {
        return Err(ValidationError::StrideTooSmallForPixelRow {
            stride: descriptor.stride,
            required: row,
        });
    }
    let required = u64::from(height - 1)
        .checked_mul(u64::from(descriptor.stride))
        .and_then(|prefix| prefix.checked_add(row))
        .ok_or(ValidationError::ArithmeticOverflow)?;
    if descriptor.byte_len < required {
        return Err(ValidationError::BufferTooSmallForDeclaredGeometry {
            available: descriptor.byte_len,
            required,
        });
    }
    if u64::try_from(backing_len).map_or(true, |len| len < descriptor.byte_len) {
        return Err(ValidationError::BackingStoreTooSmall {
            available: backing_len,
            declared: descriptor.byte_len,
        });
    }
    Ok(())
}

fn validate_damage(surface: SurfaceKey, size: Size, damage: &[Damage]) -> Result<(), ComposeError> {
    for damage in damage {
        let Damage::Rect(rect) = damage else {
            continue;
        };
        let valid = rect.x >= 0
            && rect.y >= 0
            && u64::try_from(rect.x)
                .ok()
                .and_then(|x| x.checked_add(u64::from(rect.width)))
                .is_some_and(|right| right <= u64::from(size.width))
            && u64::try_from(rect.y)
                .ok()
                .and_then(|y| y.checked_add(u64::from(rect.height)))
                .is_some_and(|bottom| bottom <= u64::from(size.height));
        if !valid {
            return Err(ComposeError::DamageRectangleOutsideSurface {
                surface,
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            });
        }
    }
    Ok(())
}

fn tight_stride(size: Size, layout: PixelLayout) -> Result<u32, ComposeError> {
    u32::try_from(
        u64::from(size.width)
            .checked_mul(u64::from(layout.bytes_per_pixel()))
            .ok_or(ComposeError::InvalidOutput(
                ValidationError::ArithmeticOverflow,
            ))?,
    )
    .map_err(|_| ComposeError::InvalidOutput(ValidationError::ArithmeticOverflow))
}

fn tight_byte_len(size: Size, layout: PixelLayout) -> Result<usize, ValidationError> {
    let stride = u64::from(size.width)
        .checked_mul(u64::from(layout.bytes_per_pixel()))
        .ok_or(ValidationError::ArithmeticOverflow)?;
    let length = usize::try_from(
        stride
            .checked_mul(u64::from(size.height))
            .ok_or(ValidationError::ArithmeticOverflow)?,
    )
    .map_err(|_| ValidationError::ArithmeticOverflow)?;
    if length > isize::MAX as usize {
        return Err(ValidationError::ArithmeticOverflow);
    }
    Ok(length)
}

fn copy_rect(
    source: &[u8],
    source_stride: u32,
    destination: &mut [u8],
    destination_stride: u32,
    layout: PixelLayout,
    rect: Rect,
) -> Result<(), ComposeError> {
    let x = u64::try_from(rect.x)
        .map_err(|_| ComposeError::InvalidOutput(ValidationError::ArithmeticOverflow))?;
    let y = u64::try_from(rect.y)
        .map_err(|_| ComposeError::InvalidOutput(ValidationError::ArithmeticOverflow))?;
    let pixel_bytes = u64::from(layout.bytes_per_pixel());
    let row_bytes =
        u64::from(rect.width)
            .checked_mul(pixel_bytes)
            .ok_or(ComposeError::InvalidOutput(
                ValidationError::ArithmeticOverflow,
            ))?;
    for row in 0..u64::from(rect.height) {
        let source_start = y
            .checked_add(row)
            .and_then(|line| line.checked_mul(u64::from(source_stride)))
            .and_then(|offset| {
                x.checked_mul(pixel_bytes)
                    .and_then(|x| offset.checked_add(x))
            })
            .ok_or(ComposeError::InvalidOutput(
                ValidationError::ArithmeticOverflow,
            ))?;
        let destination_start = y
            .checked_add(row)
            .and_then(|line| line.checked_mul(u64::from(destination_stride)))
            .and_then(|offset| {
                x.checked_mul(pixel_bytes)
                    .and_then(|x| offset.checked_add(x))
            })
            .ok_or(ComposeError::InvalidOutput(
                ValidationError::ArithmeticOverflow,
            ))?;
        copy_range(
            source,
            source_start,
            destination,
            destination_start,
            row_bytes,
        )?;
    }
    Ok(())
}

fn copy_range(
    source: &[u8],
    source_start: u64,
    destination: &mut [u8],
    destination_start: u64,
    length: u64,
) -> Result<(), ComposeError> {
    let source_end = source_start
        .checked_add(length)
        .ok_or(ComposeError::InvalidOutput(
            ValidationError::ArithmeticOverflow,
        ))?;
    let destination_end =
        destination_start
            .checked_add(length)
            .ok_or(ComposeError::InvalidOutput(
                ValidationError::ArithmeticOverflow,
            ))?;
    let source_range = usize::try_from(source_start)
        .ok()
        .zip(usize::try_from(source_end).ok());
    let destination_range = usize::try_from(destination_start)
        .ok()
        .zip(usize::try_from(destination_end).ok());
    let (Some((source_start, source_end)), Some((destination_start, destination_end))) =
        (source_range, destination_range)
    else {
        return Err(ComposeError::InvalidOutput(
            ValidationError::ArithmeticOverflow,
        ));
    };
    let source = source
        .get(source_start..source_end)
        .ok_or(ComposeError::InvalidOutput(
            ValidationError::BufferTooSmallForDeclaredGeometry {
                available: source.len() as u64,
                required: source_end as u64,
            },
        ))?;
    let destination = destination
        .get_mut(destination_start..destination_end)
        .ok_or(ComposeError::InvalidOutput(
            ValidationError::ArithmeticOverflow,
        ))?;
    destination.copy_from_slice(source);
    Ok(())
}

fn checked_origin(
    surface: SurfaceKey,
    position: Point,
    offset: Point,
) -> Result<(i64, i64), ComposeError> {
    let x = i64::from(position.x)
        .checked_add(i64::from(offset.x))
        .ok_or(ComposeError::GeometryOverflow(surface))?;
    let y = i64::from(position.y)
        .checked_add(i64::from(offset.y))
        .ok_or(ComposeError::GeometryOverflow(surface))?;
    Ok((x, y))
}

fn blit_surface(
    surface: SurfaceKey,
    image: &SurfaceImage,
    position: Point,
    offset: Point,
    frame: &mut Frame,
) -> Result<(), ComposeError> {
    let (origin_x, origin_y) = checked_origin(surface, position, offset)?;
    let output_width = i64::from(frame.size().width);
    let output_height = i64::from(frame.size().height);
    let pixel_bytes = u64::from(frame.layout().bytes_per_pixel());
    let source_stride = u64::from(image.size.width)
        .checked_mul(pixel_bytes)
        .ok_or(ComposeError::GeometryOverflow(surface))?;
    let destination_stride = u64::from(frame.stride());
    for source_y in 0..image.size.height {
        let destination_y = origin_y
            .checked_add(i64::from(source_y))
            .ok_or(ComposeError::GeometryOverflow(surface))?;
        if destination_y < 0 || destination_y >= output_height {
            continue;
        }
        for source_x in 0..image.size.width {
            let destination_x = origin_x
                .checked_add(i64::from(source_x))
                .ok_or(ComposeError::GeometryOverflow(surface))?;
            if destination_x < 0 || destination_x >= output_width {
                continue;
            }
            let source_start = u64::from(source_y)
                .checked_mul(source_stride)
                .and_then(|offset| {
                    u64::from(source_x)
                        .checked_mul(pixel_bytes)
                        .and_then(|x| offset.checked_add(x))
                })
                .ok_or(ComposeError::GeometryOverflow(surface))?;
            let destination_start = u64::try_from(destination_y)
                .ok()
                .and_then(|y| y.checked_mul(destination_stride))
                .and_then(|offset| {
                    u64::try_from(destination_x)
                        .ok()
                        .and_then(|x| x.checked_mul(pixel_bytes))
                        .and_then(|x| offset.checked_add(x))
                })
                .ok_or(ComposeError::GeometryOverflow(surface))?;
            copy_range(
                &image.bytes,
                source_start,
                frame.bytes_mut(),
                destination_start,
                pixel_bytes,
            )?;
        }
    }
    Ok(())
}
