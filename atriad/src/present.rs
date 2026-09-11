//! Composing what clients committed, and giving their buffers back.

use std::collections::{BTreeMap, BTreeSet};

use atria_compositor::{CommitId, CompositorState, ConnectionId, SceneFault, Size};
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
    /// A buffer named memory the store no longer holds, or holds less of than it claims.
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
    /// The commit each staged buffer holds. Pixels are read once per commit, not per frame.
    staged: BTreeMap<BufferKey, CommitId>,
}

impl Presenter {
    pub fn new(size: Size, layout: PixelLayout) -> Result<Self, ValidationError> {
        Ok(Self {
            output: SoftwareOutput::new(size, layout)?,
            layout,
            buffers: BufferStore::new(),
            staged: BTreeMap::new(),
        })
    }

    /// Read every buffer a connection has committed into the staging store.
    pub fn stage(
        &mut self,
        state: &CompositorState,
        connection: ConnectionId,
        memory: &impl SharedMemorySource,
        buffer: ObjectId,
        commit: CommitId,
    ) -> Result<(), PresentFailure> {
        let key = BufferKey {
            connection,
            object_id: buffer,
        };
        if self.staged.get(&key) == Some(&commit) {
            return Ok(());
        }
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
        self.buffers.insert(key, staged);
        self.staged.insert(key, commit);
        Ok(())
    }

    /// Composite everything mapped and hand the frame to `sink`.
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

    /// Forget what is staged for anything not named here.
    fn forget_all_but(&mut self, live: &BTreeSet<BufferKey>) {
        self.staged.retain(|key, _| live.contains(key));
    }

    /// The frame most recently composed.
    #[must_use]
    pub fn frame(&self) -> &Frame {
        self.output.frame()
    }
}

/// Composite every client's content into one frame, then deliver what that produced to each.
///
/// A buffer is readable only through the session that owns it, so staging is per-session even
/// though the frame is not. A surface whose session has gone is skipped, not failed.
pub fn present_all<T: Transport>(
    presenter: &mut Presenter,
    state: &mut CompositorState,
    sessions: &mut [Session<T>],
    timestamp_ns: u64,
    sink: &mut impl FrameSink,
) -> Result<FrameReport, PresentFailure> {
    if let Err(fault) = state.scene_faults() {
        return Err(PresentFailure::Scene(fault));
    }

    let order: Vec<_> = state.stacking_order().to_vec();
    let mut live = BTreeSet::new();
    for surface in order {
        let Some((buffer, commit)) = state
            .surface_snapshot(surface.connection, surface.object_id)
            .map(|snapshot| (snapshot.buffer, snapshot.commit))
        else {
            continue;
        };
        live.insert(BufferKey {
            connection: surface.connection,
            object_id: buffer,
        });
        let Some(session) = sessions
            .iter()
            .find(|session| session.connection() == surface.connection)
        else {
            return Err(PresentFailure::Scene(SceneFault::ConnectionGone(surface)));
        };
        presenter.stage(state, surface.connection, session.memory(), buffer, commit)?;
    }
    presenter.forget_all_but(&live);

    let report = presenter.present(state, timestamp_ns, sink)?;
    for session in sessions.iter_mut() {
        let _ = session.deliver(state);
    }
    Ok(report)
}

/// Stage and present one client's committed content, then deliver whatever that produced.
pub fn present_for(
    presenter: &mut Presenter,
    state: &mut CompositorState,
    session: &mut Session<impl Transport>,
    buffer: ObjectId,
    commit: CommitId,
    timestamp_ns: u64,
    sink: &mut impl FrameSink,
) -> Result<FrameReport, PresentFailure> {
    presenter.stage(
        state,
        session.connection(),
        session.memory(),
        buffer,
        commit,
    )?;
    let report = presenter.present(state, timestamp_ns, sink)?;
    let _ = session.deliver(state);
    Ok(report)
}
