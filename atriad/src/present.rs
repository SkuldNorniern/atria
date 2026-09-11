//! Composing what clients committed, and giving their buffers back.
//!
//! The half of the chain that closes it: a client attaches and commits, this reads the pixels out
//! of the memory it handed over, composites them, and the compositor releases the buffer. Until
//! the release reaches the client it cannot draw the next frame into that memory, so this is what
//! makes a second frame possible rather than a nicety.

use atria_compositor::{CompositorState, ConnectionId, SceneFault, Size};
use atria_protocol::ObjectId;
use atria_software_output::{
    BufferKey, BufferStore, Frame, FrameReport, FrameSink, PixelLayout, PresentError,
    SoftwareBuffer, SoftwareOutput, ValidationError,
};
use atria_transport::{SharedMemorySource, Transport};

use crate::session::Session;

/// Why a frame could not be presented.
#[derive(Debug)]
pub enum PresentFailure {
    /// A buffer named memory the store no longer holds, or holds less of than the buffer claims.
    ///
    /// A client that shrinks a pool under a buffer it already committed reaches this. Refusing
    /// the frame is the whole point of reading rather than mapping: the alternative is a fault
    /// inside the compositor.
    Unreadable,
    /// The bytes do not describe the buffer they came with.
    Invalid(ValidationError),
    /// Composition or the sink refused.
    Present(PresentError),
    /// The scene disagrees with the objects it names, which no client request can cause.
    Scene(SceneFault),
}

/// One output, and the pixels staged for it.
pub struct Presenter {
    output: SoftwareOutput,
    layout: PixelLayout,
    buffers: BufferStore,
}

impl Presenter {
    /// # Errors
    ///
    /// Returns [`ValidationError`] when the size and layout do not describe a usable frame.
    pub fn new(size: Size, layout: PixelLayout) -> Result<Self, ValidationError> {
        Ok(Self {
            output: SoftwareOutput::new(size, layout)?,
            layout,
            buffers: BufferStore::new(),
        })
    }

    /// Read every buffer a connection has committed into the staging store.
    ///
    /// Done once per frame rather than at attach: a client may attach and then commit something
    /// else, and only what it committed is what this frame shows.
    ///
    /// # Errors
    ///
    /// Returns [`PresentFailure::Unreadable`] when a buffer's memory is gone or short.
    pub fn stage(
        &mut self,
        state: &CompositorState,
        connection: ConnectionId,
        memory: &impl SharedMemorySource,
        buffer: ObjectId,
    ) -> Result<(), PresentFailure> {
        let source = state
            .buffer_source(connection, buffer)
            .ok_or(PresentFailure::Unreadable)?;
        let len =
            usize::try_from(source.descriptor.byte_len).map_err(|_| PresentFailure::Unreadable)?;
        let bytes = memory
            .read(source.memory, source.offset, len)
            .map_err(|_| PresentFailure::Unreadable)?;
        let staged = SoftwareBuffer::new(source.descriptor, self.layout, bytes)
            .map_err(PresentFailure::Invalid)?;
        self.buffers.insert(
            BufferKey {
                connection,
                object_id: buffer,
            },
            staged,
        );
        Ok(())
    }

    /// Composite everything mapped and hand the frame to `sink`.
    ///
    /// # Errors
    ///
    /// Returns [`PresentFailure::Present`] when composition or the sink refuses.
    pub fn present(
        &mut self,
        state: &mut CompositorState,
        timestamp_ns: u64,
        sink: &mut impl FrameSink,
    ) -> Result<FrameReport, PresentFailure> {
        self.output
            .present_to(state, &self.buffers, timestamp_ns, sink)
            .map_err(PresentFailure::Present)
    }

    /// The frame most recently composed.
    #[must_use]
    pub fn frame(&self) -> &Frame {
        self.output.frame()
    }
}

/// Composite every client's content into one frame, then deliver what that produced to each.
///
/// The scene's stacking order is compositor-wide, so this walks it once and reads each surface's
/// pixels through the session that owns them. A buffer belongs to the client that handed over the
/// memory behind it, and no other session can read it — which is why staging is per-session even
/// though the frame is not.
///
/// A surface whose session has gone is skipped rather than failing the frame. The other clients
/// are still drawing, and one departure is not everyone's.
///
/// # Errors
///
/// Returns [`PresentFailure`] when a buffer cannot be read or the frame cannot be composed.
pub fn present_all<T: Transport>(
    presenter: &mut Presenter,
    state: &mut CompositorState,
    sessions: &mut [Session<T>],
    timestamp_ns: u64,
    sink: &mut impl FrameSink,
) -> Result<FrameReport, PresentFailure> {
    // Checked before composing, because a frame that cannot be built says only that it cannot be
    // built. This says which surface disagrees with the objects behind it, and how.
    if let Err(fault) = state.scene_faults() {
        return Err(PresentFailure::Scene(fault));
    }

    let order: Vec<_> = state.stacking_order().to_vec();
    for surface in order {
        let Some(buffer) = state
            .surface_snapshot(surface.connection, surface.object_id)
            .map(|snapshot| snapshot.buffer)
        else {
            continue;
        };
        let Some(session) = sessions
            .iter()
            .find(|session| session.connection() == surface.connection)
        else {
            // The scene check above already proved the connection exists, so reaching here means
            // the server holds a connection it has no session for.
            return Err(PresentFailure::Scene(SceneFault::ConnectionGone(surface)));
        };
        presenter.stage(state, surface.connection, session.memory(), buffer)?;
    }

    let report = presenter.present(state, timestamp_ns, sink)?;
    for session in sessions.iter_mut() {
        let _ = session.deliver(state);
    }
    Ok(report)
}

/// Stage and present one client's committed content, then deliver whatever that produced.
///
/// # Errors
///
/// Returns [`PresentFailure`] when the frame could not be composed.
pub fn present_for(
    presenter: &mut Presenter,
    state: &mut CompositorState,
    session: &mut Session<impl Transport>,
    buffer: ObjectId,
    timestamp_ns: u64,
    sink: &mut impl FrameSink,
) -> Result<FrameReport, PresentFailure> {
    presenter.stage(state, session.connection(), session.memory(), buffer)?;
    let report = presenter.present(state, timestamp_ns, sink)?;
    // The release the compositor decided during presentation is an event like any other, and the
    // client cannot reuse its memory until it arrives.
    let _ = session.deliver(state);
    Ok(report)
}
