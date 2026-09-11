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
    /// The commit these bytes hold, once the frame carrying it has been presented and its buffer
    /// released. `None` while the bytes are newer than anything a client has been told about, so
    /// a frame that failed part way is read again rather than counted as already consumed.
    commit: Option<CommitId>,
    size: Size,
    bytes: Vec<u8>,
}

/// Where a surface was drawn, and how far up the stack. A change to either moves pixels without
/// the surface's content changing at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Placement {
    rect: Rect,
    depth: usize,
}

#[derive(Clone, Debug)]
pub struct SoftwareOutput {
    frame: Frame,
    /// Composed into, then swapped with `frame`. Two lasting allocations rather than one per
    /// frame, which at 1280x720 is 3.5 MB of churn per pointer movement.
    spare: Frame,
    surfaces: BTreeMap<SurfaceKey, SurfaceImage>,
    /// Where each surface was drawn in the frame before this one.
    placed: BTreeMap<SurfaceKey, Placement>,
    /// Rectangles covering every pixel of the last frame that differs from the one before it.
    ///
    /// A frame is cleared and rebuilt from its surfaces, so two frames can differ only where a
    /// surface was added, removed, moved, changed depth or changed content. Each of those
    /// contributes the rectangle it left and the one it took, and everywhere else both frames
    /// hold the same surfaces at the same depths over the same ground. Conservative by
    /// construction: never smaller than the difference, sometimes larger.
    damage: Vec<Rect>,
    composed_once: bool,
}

impl SoftwareOutput {
    pub fn new(size: Size, layout: PixelLayout) -> Result<Self, ValidationError> {
        Ok(Self {
            frame: Frame::new(size, layout)?,
            spare: Frame::new(size, layout)?,
            surfaces: BTreeMap::new(),
            placed: BTreeMap::new(),
            damage: Vec::new(),
            composed_once: false,
        })
    }

    #[must_use]
    pub const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// What the last composed frame changed, relative to the one before it.
    #[must_use]
    pub fn damage(&self) -> &[Rect] {
        &self.damage
    }

    /// The part of `size` placed at `origin` that lands on the output, or `None` for none of it.
    fn clipped(&self, origin_x: i64, origin_y: i64, size: Size) -> Option<Rect> {
        let left = origin_x.max(0);
        let top = origin_y.max(0);
        let right = origin_x
            .saturating_add(i64::from(size.width))
            .min(i64::from(self.frame.size().width));
        let bottom = origin_y
            .saturating_add(i64::from(size.height))
            .min(i64::from(self.frame.size().height));
        if left >= right || top >= bottom {
            return None;
        }
        Some(Rect {
            x: i32::try_from(left).ok()?,
            y: i32::try_from(top).ok()?,
            width: u32::try_from(right - left).ok()?,
            height: u32::try_from(bottom - top).ok()?,
        })
    }

    /// Record what this frame changed, and where every surface now sits.
    ///
    /// `origins` is every surface in stacking order; `redrawn` is those whose pixels were read
    /// again. A surface that kept its rectangle, its depth and its content cannot have changed a
    /// pixel, so it contributes nothing. Anything else contributes the rectangle it left and the
    /// one it took.
    fn note_damage(&mut self, origins: &[(SurfaceKey, i64, i64)], redrawn: &BTreeSet<SurfaceKey>) {
        let mut now = BTreeMap::new();
        for (depth, &(surface, origin_x, origin_y)) in origins.iter().enumerate() {
            let Some(size) = self.surfaces.get(&surface).map(|image| image.size) else {
                continue;
            };
            if let Some(rect) = self.clipped(origin_x, origin_y, size) {
                now.insert(surface, Placement { rect, depth });
            }
        }

        self.damage.clear();
        if self.composed_once {
            for (surface, place) in &now {
                match self.placed.get(surface) {
                    Some(before) if before == place && !redrawn.contains(surface) => {}
                    Some(before) => {
                        self.damage.push(before.rect);
                        self.damage.push(place.rect);
                    }
                    None => self.damage.push(place.rect),
                }
            }
            for (surface, before) in &self.placed {
                if !now.contains_key(surface) {
                    self.damage.push(before.rect);
                }
            }
        } else {
            // Nothing was on screen before the first frame, so all of it is new.
            self.composed_once = true;
            self.damage.push(Rect {
                x: 0,
                y: 0,
                width: self.frame.size().width,
                height: self.frame.size().height,
            });
        }
        self.placed = now;
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
        let layout = self.frame.layout();
        let mut active = BTreeSet::new();
        let mut consumed = Vec::new();
        let mut damage_count = 0_usize;
        let mut origins: Vec<(SurfaceKey, i64, i64)> = Vec::new();
        let mut redrawn = BTreeSet::new();

        for &surface in state.stacking_order() {
            active.insert(surface);
            let snapshot = state
                .surface_snapshot(surface.connection, surface.object_id)
                .cloned()
                .ok_or(ComposeError::MissingSurfaceState(surface))?;
            let Some(position) = state.surface_position(surface) else {
                return Err(ComposeError::MissingSurfacePosition(surface));
            };
            let (origin_x, origin_y) = checked_origin(surface, position, snapshot.offset)?;
            origins.push((surface, origin_x, origin_y));

            if self
                .surfaces
                .get(&surface)
                .is_some_and(|image| image.commit == Some(snapshot.commit))
            {
                continue;
            }
            redrawn.insert(surface);
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
            if buffer.layout() != layout {
                return Err(ComposeError::PixelLayoutMismatch {
                    buffer: buffer_key,
                    expected: layout,
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
            validate_for_layout(descriptor, layout, buffer.bytes().len()).map_err(|error| {
                ComposeError::InvalidBuffer {
                    buffer: buffer_key,
                    error,
                }
            })?;
            validate_damage(surface, descriptor.size, &snapshot.damage)?;

            let replace_all = self
                .surfaces
                .get(&surface)
                .is_none_or(|image| image.size != descriptor.size);
            let byte_len = tight_byte_len(descriptor.size, layout).map_err(|error| {
                ComposeError::InvalidBuffer {
                    buffer: buffer_key,
                    error,
                }
            })?;
            let tight = tight_stride(descriptor.size, layout)?;
            let whole = Rect {
                x: 0,
                y: 0,
                width: descriptor.size.width,
                height: descriptor.size.height,
            };
            if replace_all {
                // Moved in whole, so a failed copy leaves no size disagreeing with its bytes.
                let mut bytes = vec![0; byte_len];
                copy_rect(
                    buffer.bytes(),
                    descriptor.stride,
                    &mut bytes,
                    tight,
                    layout,
                    whole,
                )?;
                self.surfaces.insert(
                    surface,
                    SurfaceImage {
                        commit: None,
                        size: descriptor.size,
                        bytes,
                    },
                );
            } else {
                let image = self
                    .surfaces
                    .get_mut(&surface)
                    .ok_or(ComposeError::MissingSurfaceState(surface))?;
                for damage in &snapshot.damage {
                    let rect = match *damage {
                        Damage::Full => whole,
                        Damage::Rect(rect) => rect,
                    };
                    copy_rect(
                        buffer.bytes(),
                        descriptor.stride,
                        &mut image.bytes,
                        tight,
                        layout,
                        rect,
                    )?;
                }
                image.commit = None;
            }
            damage_count = damage_count.saturating_add(snapshot.damage.len());
            consumed.push((surface, snapshot.buffer, snapshot.commit));
        }

        self.surfaces.retain(|surface, _| active.contains(surface));
        // Into the kept frame, then swapped in.
        self.spare.clear();
        for &surface in state.stacking_order() {
            let snapshot = state
                .surface_snapshot(surface.connection, surface.object_id)
                .ok_or(ComposeError::MissingSurfaceState(surface))?;
            let position = state
                .surface_position(surface)
                .ok_or(ComposeError::MissingSurfacePosition(surface))?;
            let image = self
                .surfaces
                .get(&surface)
                .ok_or(ComposeError::MissingSurfaceState(surface))?;
            blit_surface(surface, image, position, snapshot.offset, &mut self.spare)?;
        }

        for (surface, buffer, _) in &consumed {
            state
                .present(*surface, timestamp_ns)
                .map_err(|_| ComposeError::StateChanged(*surface))?;
            state
                .release_buffer(surface.connection, *buffer)
                .map_err(|_| ComposeError::StateChanged(*surface))?;
        }
        // Only now is a commit accounted for. Until the release reaches the client, a frame that
        // failed leaves the pixels staged and the commit still owing.
        for (surface, _, commit) in &consumed {
            if let Some(image) = self.surfaces.get_mut(surface) {
                image.commit = Some(*commit);
            }
        }
        // Last, with the frame built and its commits accounted for. A frame that failed leaves
        // the recorded placement describing what is actually on screen.
        self.note_damage(&origins, &redrawn);

        let report = FrameReport {
            timestamp_ns,
            surfaces_composited: self.surfaces.len(),
            commits_consumed: consumed.len(),
            damage_rectangles_consumed: damage_count,
            bytes_written: self.spare.bytes().len(),
        };
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
    let overflow = || ComposeError::GeometryOverflow(surface);
    let (origin_x, origin_y) = checked_origin(surface, position, offset)?;
    let pixel_bytes = u64::from(frame.layout().bytes_per_pixel());
    let source_stride = u64::from(image.size.width)
        .checked_mul(pixel_bytes)
        .ok_or_else(overflow)?;
    let destination_stride = u64::from(frame.stride());

    // The visible part, in the surface's own coordinates. Clipped once, not once per pixel.
    let width = i64::from(image.size.width);
    let height = i64::from(image.size.height);
    let span = |origin: i64, extent: i64, limit: i64| {
        let first = origin.checked_neg()?.clamp(0, extent);
        let last = limit.checked_sub(origin)?.clamp(0, extent);
        Some((first, last))
    };
    let (x_first, x_last) =
        span(origin_x, width, i64::from(frame.size().width)).ok_or_else(overflow)?;
    let (y_first, y_last) =
        span(origin_y, height, i64::from(frame.size().height)).ok_or_else(overflow)?;
    if x_first >= x_last || y_first >= y_last {
        return Ok(());
    }

    let row_bytes = u64::try_from(x_last - x_first)
        .ok()
        .and_then(|columns| columns.checked_mul(pixel_bytes))
        .ok_or_else(overflow)?;
    let source_x = u64::try_from(x_first)
        .ok()
        .and_then(|x| x.checked_mul(pixel_bytes))
        .ok_or_else(overflow)?;
    let destination_x = origin_x
        .checked_add(x_first)
        .and_then(|x| u64::try_from(x).ok())
        .and_then(|x| x.checked_mul(pixel_bytes))
        .ok_or_else(overflow)?;

    for row in y_first..y_last {
        let source_start = u64::try_from(row)
            .ok()
            .and_then(|y| y.checked_mul(source_stride))
            .and_then(|offset| offset.checked_add(source_x))
            .ok_or_else(overflow)?;
        let destination_start = origin_y
            .checked_add(row)
            .and_then(|y| u64::try_from(y).ok())
            .and_then(|y| y.checked_mul(destination_stride))
            .and_then(|offset| offset.checked_add(destination_x))
            .ok_or_else(overflow)?;
        copy_range(
            &image.bytes,
            source_start,
            frame.bytes_mut(),
            destination_start,
            row_bytes,
        )?;
    }
    Ok(())
}
