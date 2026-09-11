use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::error::Error;
use core::fmt;
use core::mem::take;

use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::error::ErrorCategory;

use crate::error::StateError;
use atria_protocol::interface::toplevel_state;

use crate::model::{
    BufferDescriptor, BufferState, BufferTransport, ClientRequest, CommitId, ConnectionId, Damage,
    Event, EventKind, FocusEvent, ObjectKind, Point, Rect, SeatCapabilities, SeatSnapshot,
    SessionSnapshot, Size, SurfaceKey, SurfaceRole, SurfaceSnapshot, TitleText,
    pixel_format_is_known,
};
use atria_protocol::interface::Interface;

use crate::binding::interface_of;
use alloc::string::String;

use crate::output::{IdentitySource, OutputIdentity, OutputInfo, OutputSet, TopologyDelta};
use crate::registry::{ObjectRegistry, Teardown};
use crate::resolve::SharedMemory;
use crate::shell::{ShellError, ToplevelHandle, ToplevelRecord};

/// Bounds on the compositor as a whole, rather than on any one connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerLimits {
    pub max_connections: usize,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            // One connection is one channel on Artery, and the kernel bounds channel pairs at 64.
            // A compositor that accepted more would be promising a transport it cannot be given.
            max_connections: 64,
        }
    }
}

/// Bounds on one connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionLimits {
    pub max_objects: usize,
    pub max_surfaces: usize,
    pub max_buffers: usize,
    pub max_imported_handles: usize,
    pub max_damage_rects_per_commit: usize,
    /// Events one connection may have waiting between drains. A connection that reaches it is
    /// not reading, and is disconnected rather than allowed to grow the compositor's memory.
    pub max_pending_events: usize,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_objects: 1_024,
            max_surfaces: 256,
            max_buffers: 64,
            // Draft §11 defines 64 as the default per-client fd quota. Each imported buffer
            // represents one already-validated handle at this layer.
            max_imported_handles: 64,
            max_damage_rects_per_commit: 256,
            max_pending_events: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NegotiationError {
    UnsupportedCapabilities {
        required: CapabilitySet,
        available: CapabilitySet,
    },
    ConnectionIdExhausted,
    /// The compositor already holds [`ServerLimits::max_connections`] connections.
    ConnectionLimitReached {
        limit: usize,
    },
}

impl fmt::Display for NegotiationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCapabilities {
                required,
                available,
            } => write!(
                formatter,
                "connection requires capabilities {required:?}, but only {available:?} are mutually available"
            ),
            Self::ConnectionLimitReached { limit } => write!(
                formatter,
                "the compositor already holds its limit of {limit} connections"
            ),
            Self::ConnectionIdExhausted => formatter
                .write_str("cannot create a connection because all connection IDs are exhausted"),
        }
    }
}

impl Error for NegotiationError {}

#[derive(Clone, Debug)]
enum Object {
    Display,
    Registry,
    Toplevel(ToplevelState),
    /// A global the client bound. The compositor's resource behind it is not this object, so
    /// destroying it releases the reference and nothing else.
    Global,
    ShmPool(SharedMemory),
    Seat(SeatState),
    Session(SessionState),
    Surface(SurfaceState),
    Buffer(BufferObject),
    Fence(FenceState),
    InputStream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SeatState {
    capabilities: SeatCapabilities,
    active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionState {
    seat: Option<ObjectId>,
    active: bool,
}

/// What a commit will do to the surface's content.
///
/// Three outcomes, named rather than inferred from an `Option`. A commit that attaches nothing is
/// not the same as a commit that attaches nothing *on purpose*: the first keeps the content the
/// surface already has, the second takes it away. An `Option` cannot tell them apart, so it made
/// state-only commits unrepresentable and the code rejected them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PendingAttachment {
    /// No attach since the last commit. The surface keeps whatever it is already showing.
    #[default]
    Unchanged,
    /// A buffer to show from this commit onwards.
    Set(ObjectId, Point),
    /// A null attach. The surface stops showing anything and leaves the scene.
    Detach,
}

#[derive(Clone, Debug, Default)]
struct PendingSurface {
    attachment: PendingAttachment,
    acquire_fence: Option<ObjectId>,
    damage: Vec<Damage>,
    refresh_range: Option<(u32, u32)>,
}

#[derive(Clone, Debug)]
struct CurrentSurface {
    snapshot: SurfaceSnapshot,
    frame_callback: bool,
    /// The serial the client asked with, echoed back when the frame is presented.
    frame_serial: u32,
}

#[derive(Clone, Debug)]
struct SurfaceState {
    session: ObjectId,
    role: Option<SurfaceRole>,
    pending: PendingSurface,
    current: Option<CurrentSurface>,
    frame_requested: bool,
    /// The serial the client asked with, echoed back when the frame is presented.
    frame_serial: u32,
    consecutive_deadline_misses: u8,
}

#[derive(Clone, Copy, Debug)]
struct BufferObject {
    descriptor: BufferDescriptor,
    state: BufferState,
    /// Where the pixels are, for a buffer carved out of a pool.
    ///
    /// `None` for one imported as an inert descriptor by a backend that already resolved it.
    /// Recorded so presentation can find the bytes without the compositor having mapped them.
    source: Option<BufferSource>,
}

/// A surface given window semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ToplevelState {
    surface: ObjectId,
    title: TitleText,
    minimum: Option<Size>,
    maximum: Option<Size>,
    /// The serial of the most recent configure, so a commit answering it can be recognised.
    last_configure: u32,
    /// The size the compositor last asked for, retained so a shell taking over is told what the
    /// window was already asked to be rather than having to guess or re-ask.
    configured: Option<Size>,
    /// How a shell refers to this window.
    handle: ToplevelHandle,
}

/// Where a buffer's pixels live: a region of memory a client handed over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferSource {
    pub memory: SharedMemory,
    pub offset: u32,
    pub descriptor: BufferDescriptor,
}

#[derive(Clone, Copy, Debug, Default)]
struct FenceState {
    signaled: bool,
}

#[derive(Clone, Debug)]
struct Connection {
    /// Events queued for this connection since the last drain, bounding what one client that
    /// stops reading can make the compositor hold.
    pending_events: usize,
    /// The session this connection's surfaces belong to.
    ///
    /// A connection belongs to one session: the wire creates a surface through the compositor
    /// global, which names no session, so the connection has to be what says which one. A client
    /// does not choose a session per surface, and never did — the field on a surface only ever
    /// held whatever its creator passed.
    session: Option<ObjectId>,
    available_capabilities: CapabilitySet,
    capabilities: CapabilitySet,
    registry: ObjectRegistry<Object>,
}

#[derive(Clone, Debug, Default)]
struct Scene {
    positions: BTreeMap<SurfaceKey, Point>,
    /// Bottom to top.
    stack: Vec<SurfaceKey>,
}

/// A global the registry advertises: what it offers, and up to which version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Global {
    interface: Interface,
    kind: ObjectKind,
    version: u32,
    /// Which display this global offers, when it offers one.
    ///
    /// A display is one global each rather than one global listing displays, so a display
    /// arriving or leaving is a global arriving or leaving — the mechanism clients already have
    /// for learning what exists.
    output: Option<OutputIdentity>,
}

/// The protocol state machine between decoded messages and a display/input backend.
#[derive(Clone, Debug)]
pub struct CompositorState {
    server_capabilities: CapabilitySet,
    /// Globals by name. A name is stable for as long as the global is advertised, and is never
    /// reused after it is withdrawn — a client may still have one in flight.
    globals: BTreeMap<u32, Global>,
    next_global: u32,
    outputs: OutputSet,
    next_configure: u32,
    /// Windows by the handle a shell names them with. Compositor-wide, so it outlives any one
    /// connection — including a shell's.
    toplevels: BTreeMap<ToplevelHandle, (ConnectionId, ObjectId)>,
    next_handle: u64,
    server_limits: ServerLimits,
    limits: ConnectionLimits,
    next_connection: u64,
    next_commit: u64,
    connections: BTreeMap<ConnectionId, Connection>,
    scene: Scene,
    keyboard_focus: Option<SurfaceKey>,
    pointer_target: Option<SurfaceKey>,
    events: Vec<Event>,
}

impl CompositorState {
    #[must_use]
    pub fn new(
        server_capabilities: CapabilitySet,
        server_limits: ServerLimits,
        limits: ConnectionLimits,
    ) -> Self {
        Self {
            server_capabilities: server_capabilities
                .with(Capability::SurfaceCreate)
                .with(Capability::BufferImport),
            globals: BTreeMap::new(),
            next_global: 1,
            outputs: OutputSet::new(),
            next_configure: 1,
            toplevels: BTreeMap::new(),
            next_handle: 1,
            server_limits,
            limits,
            next_connection: 1,
            next_commit: 1,
            connections: BTreeMap::new(),
            scene: Scene::default(),
            keyboard_focus: None,
            pointer_target: None,
            events: Vec::new(),
        }
    }

    pub fn connect(
        &mut self,
        client_capabilities: CapabilitySet,
        required: CapabilitySet,
    ) -> Result<ConnectionId, NegotiationError> {
        let available = self
            .server_capabilities
            .intersection(client_capabilities)
            .with(Capability::SurfaceCreate)
            .with(Capability::BufferImport);
        if !available.satisfies(required) {
            return Err(NegotiationError::UnsupportedCapabilities {
                required,
                available,
            });
        }
        if self.connections.len() >= self.server_limits.max_connections {
            return Err(NegotiationError::ConnectionLimitReached {
                limit: self.server_limits.max_connections,
            });
        }
        if self.next_connection == u64::MAX {
            return Err(NegotiationError::ConnectionIdExhausted);
        }
        let id = ConnectionId(self.next_connection);
        self.next_connection += 1;
        let mut capabilities = available.intersection(CapabilitySet::default_grants());
        // ADR-0012's backend semantics are advertised facts, not policy privileges.
        for capability in [
            Capability::SoftwareShm,
            Capability::GpuPrime,
            Capability::ExplicitGpuFence,
            Capability::RevocableSeat,
        ] {
            if available.contains(capability) {
                capabilities = capabilities.with(capability);
            }
        }
        self.connections.insert(
            id,
            Connection {
                pending_events: 0,
                session: None,
                available_capabilities: available,
                capabilities,
                registry: ObjectRegistry::new(Object::Display),
            },
        );
        Ok(id)
    }

    #[must_use]
    pub fn is_connected(&self, connection: ConnectionId) -> bool {
        self.connections.contains_key(&connection)
    }

    #[must_use]
    pub fn capabilities(&self, connection: ConnectionId) -> Option<CapabilitySet> {
        self.connections
            .get(&connection)
            .map(|value| value.capabilities)
    }

    /// Records a policy authority's grant. This method does not make the policy decision.
    pub fn grant_capability(
        &mut self,
        connection: ConnectionId,
        capability: Capability,
    ) -> Result<(), StateError> {
        let available = self.connection(connection)?.available_capabilities;
        if !available.contains(capability) {
            return Err(StateError::UnsupportedCapability {
                object_id: ObjectId::DISPLAY,
                capability,
            });
        }
        let client = self.connection_mut(connection)?;
        client.capabilities = client.capabilities.with(capability);
        Ok(())
    }

    /// Revocation is observable before any affected object is terminated, as required by §6.
    pub fn revoke_capability(
        &mut self,
        connection: ConnectionId,
        capability: Capability,
    ) -> Result<Vec<(ObjectId, ObjectKind)>, StateError> {
        let Some(client) = self.connections.get_mut(&connection) else {
            return Err(StateError::ConnectionClosed);
        };
        client.capabilities = client.capabilities.without(capability);
        let ids: Vec<_> = client.registry.ids_reverse_creation().collect();
        self.push_event(
            connection,
            ObjectId::DISPLAY,
            EventKind::CapabilityRevoked(capability),
        );
        let mut destroyed = Vec::new();
        for id in ids {
            if self.object_requires(connection, id, capability) {
                self.destroy_object(connection, id, &mut destroyed);
            }
        }
        Ok(destroyed)
    }

    /// Adds an authenticated seat after platform/policy code has validated its authority.
    pub fn create_seat(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
        capabilities: SeatCapabilities,
        active: bool,
    ) -> Result<(), StateError> {
        self.allocate_client(
            connection,
            id,
            ObjectKind::Seat,
            Object::Seat(SeatState {
                capabilities,
                active,
            }),
        )
    }

    /// Adds an authenticated session. Token validation is deliberately outside this crate.
    pub fn create_session(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
        seat: Option<ObjectId>,
        active: bool,
    ) -> Result<(), StateError> {
        if let Some(seat_id) = seat {
            self.expect_kind(connection, seat_id, ObjectKind::Seat)?;
        }
        // One session per connection. A second would leave the compositor guessing which one a
        // surface created through the compositor global belongs to, and guessing is what the
        // wire having no session argument is meant to remove.
        if self.connection(connection)?.session.is_some() {
            return Err(StateError::InvalidState { object_id: id });
        }
        self.allocate_client(
            connection,
            id,
            ObjectKind::Session,
            Object::Session(SessionState { seat, active }),
        )?;
        self.connection_mut(connection)?.session = Some(id);
        Ok(())
    }

    /// The session this connection's surfaces belong to, if one has been established.
    #[must_use]
    pub fn session_of(&self, connection: ConnectionId) -> Option<ObjectId> {
        self.connections.get(&connection)?.session
    }

    pub fn create_fence(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
    ) -> Result<(), StateError> {
        self.require_capability(connection, id, Capability::ExplicitGpuFence)?;
        self.allocate_client(
            connection,
            id,
            ObjectKind::Fence,
            Object::Fence(FenceState::default()),
        )
    }

    /// Creates a compositor-owned, client-readable input stream after policy grants it.
    pub fn create_input_stream(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
    ) -> Result<(), StateError> {
        self.require_capability(connection, id, Capability::ExtendedInput)?;
        self.allocate_client(connection, id, ObjectKind::InputStream, Object::InputStream)
    }

    pub fn dispatch(
        &mut self,
        connection: ConnectionId,
        request: ClientRequest,
    ) -> Result<(), StateError> {
        if !self.connections.contains_key(&connection) {
            return Err(StateError::ConnectionClosed);
        }
        let result = self.dispatch_checked(connection, request);
        if let Err(error) = result {
            self.push_event(connection, error.object_id(), EventKind::Error(error));
            match error.category() {
                ErrorCategory::Protocol => {
                    let _ = self.close_connection(connection);
                }
                ErrorCategory::Object => {
                    let mut ignored = Vec::new();
                    self.destroy_object(connection, error.object_id(), &mut ignored);
                }
                ErrorCategory::Resource | ErrorCategory::Compositor => {}
            }
        }
        result
    }

    fn dispatch_checked(
        &mut self,
        connection: ConnectionId,
        request: ClientRequest,
    ) -> Result<(), StateError> {
        match request {
            ClientRequest::CreateRegistry { new_id } => {
                self.allocate_client(connection, new_id, ObjectKind::Registry, Object::Registry)?;
                // A registry that advertises nothing is a client that can reach nothing, so the
                // announcements are part of creating it rather than a later step.
                self.announce_globals(connection);
                Ok(())
            }
            ClientRequest::Bind {
                name,
                version,
                new_id,
            } => self.bind_global(connection, name, version, new_id),
            ClientRequest::GetToplevel { surface, new_id } => {
                self.get_toplevel(connection, surface, new_id)
            }
            ClientRequest::SetTitle { toplevel, title } => {
                self.toplevel_mut(connection, toplevel)?.title = title;
                Ok(())
            }
            ClientRequest::SetMinSize { toplevel, size } => {
                self.toplevel_mut(connection, toplevel)?.minimum = Some(size);
                Ok(())
            }
            ClientRequest::SetMaxSize { toplevel, size } => {
                self.toplevel_mut(connection, toplevel)?.maximum = Some(size);
                Ok(())
            }
            ClientRequest::CreatePool { new_id, memory } => {
                self.require_capability(connection, new_id, Capability::BufferImport)?;
                self.check_kind_quota(connection, ObjectKind::ShmPool)?;
                self.allocate_client(
                    connection,
                    new_id,
                    ObjectKind::ShmPool,
                    Object::ShmPool(memory),
                )
            }
            ClientRequest::CreateSurface { new_id } => {
                self.require_capability(connection, new_id, Capability::SurfaceCreate)?;
                let session = self
                    .session_of(connection)
                    .ok_or(StateError::InvalidState { object_id: new_id })?;
                self.expect_active_session(connection, session)?;
                self.check_kind_quota(connection, ObjectKind::Surface)?;
                self.allocate_client(
                    connection,
                    new_id,
                    ObjectKind::Surface,
                    Object::Surface(SurfaceState {
                        session,
                        role: None,
                        pending: PendingSurface::default(),
                        current: None,
                        frame_requested: false,
                        frame_serial: 0,
                        consecutive_deadline_misses: 0,
                    }),
                )
            }
            ClientRequest::CreateBuffer {
                new_id,
                pool,
                offset,
                size,
                stride,
                format,
            } => self.create_buffer(connection, new_id, pool, offset, size, stride, format),
            ClientRequest::ImportBuffer { new_id, descriptor } => {
                self.require_capability(connection, new_id, Capability::BufferImport)?;
                self.require_capability(connection, new_id, descriptor.transport.capability())?;
                if !descriptor.is_structurally_valid() {
                    return Err(StateError::InvalidState { object_id: new_id });
                }
                self.check_kind_quota(connection, ObjectKind::Buffer)?;
                self.allocate_client(
                    connection,
                    new_id,
                    ObjectKind::Buffer,
                    Object::Buffer(BufferObject {
                        descriptor,
                        state: BufferState::Available,
                        source: None,
                    }),
                )
            }
            ClientRequest::Attach {
                surface,
                buffer,
                offset,
                acquire_fence,
            } => self.attach(connection, surface, buffer, offset, acquire_fence),
            ClientRequest::Damage { surface, rect } => self.damage(connection, surface, rect),
            ClientRequest::Commit { surface } => self.commit(connection, surface),
            // A shell's requests carry a compositor-wide handle, and every one re-checks the
            // grant. Checking once at bind would leave a revoked shell still arranging windows.
            ClientRequest::ShellConfigure {
                control,
                handle,
                size,
                state,
            } => self
                .shell_configure(connection, handle, size, state)
                .map(|_| ())
                .map_err(|error| shell_refusal(error, control)),
            ClientRequest::ShellPlace {
                control,
                handle,
                position,
            } => self
                .shell_place(connection, handle, position)
                .map_err(|error| shell_refusal(error, control)),
            ClientRequest::ShellRaise { control, handle } => self
                .shell_raise(connection, handle)
                .map_err(|error| shell_refusal(error, control)),
            ClientRequest::ShellFocus { control, handle } => self
                .shell_focus(connection, handle)
                .map_err(|error| shell_refusal(error, control)),
            ClientRequest::ShellClose { control, handle } => self
                .shell_close(connection, handle)
                .map_err(|error| shell_refusal(error, control)),
            ClientRequest::SetRole { surface, role } => self.set_role(connection, surface, role),
            ClientRequest::RequestFrame { surface, serial } => {
                self.request_frame(connection, surface, serial)
            }
            ClientRequest::SetRefreshRange {
                surface,
                min_hz,
                max_hz,
            } => self.set_refresh_range(connection, surface, min_hz, max_hz),
            ClientRequest::Destroy { object } => {
                if object == ObjectId::DISPLAY {
                    return Err(StateError::InvalidState { object_id: object });
                }
                self.expect_any(connection, object)?;
                if self
                    .buffer(connection, object)
                    .is_ok_and(|buffer| buffer.state != BufferState::Available)
                {
                    // §7 forbids destruction until release. Section 10 still requires an
                    // object error to terminate the offending buffer object.
                    return Err(StateError::InvalidState { object_id: object });
                }
                let mut destroyed = Vec::new();
                self.destroy_object(connection, object, &mut destroyed);
                Ok(())
            }
            ClientRequest::Unknown { object, opcode } => {
                self.expect_any(connection, object)?;
                Err(StateError::UnknownOpcode {
                    object_id: object,
                    opcode,
                })
            }
        }
    }

    fn attach(
        &mut self,
        connection: ConnectionId,
        surface: ObjectId,
        buffer: ObjectId,
        offset: Point,
        acquire_fence: Option<ObjectId>,
    ) -> Result<(), StateError> {
        self.expect_kind(connection, surface, ObjectKind::Surface)?;
        let key = SurfaceKey {
            connection,
            object_id: surface,
        };
        // §7 says attach MUST be followed by commit and does not define replacement of an
        // uncommitted attachment. Rejecting a second attach avoids ambiguous release behavior.
        if self.surface(connection, surface)?.pending.attachment != PendingAttachment::Unchanged {
            return Err(StateError::InvalidState { object_id: surface });
        }

        // A null buffer is the request to show nothing. There is no buffer to fence against, so a
        // fence alongside it would have nothing to wait on.
        if buffer.is_null() {
            if acquire_fence.is_some() {
                return Err(StateError::InvalidState { object_id: surface });
            }
            self.surface_mut(connection, surface)?.pending.attachment = PendingAttachment::Detach;
            return Ok(());
        }

        self.expect_kind(connection, buffer, ObjectKind::Buffer)?;
        if let Some(fence) = acquire_fence {
            self.require_capability(connection, surface, Capability::ExplicitGpuFence)?;
            self.expect_kind(connection, fence, ObjectKind::Fence)?;
        }
        if self.buffer(connection, buffer)?.state != BufferState::Available {
            return Err(StateError::InvalidState { object_id: buffer });
        }
        self.set_buffer_state(connection, buffer, BufferState::Pending { surface: key })?;
        let state = self.surface_mut(connection, surface)?;
        state.pending.attachment = PendingAttachment::Set(buffer, offset);
        state.pending.acquire_fence = acquire_fence;
        Ok(())
    }

    fn damage(
        &mut self,
        connection: ConnectionId,
        surface: ObjectId,
        rect: Rect,
    ) -> Result<(), StateError> {
        if rect.width == 0 || rect.height == 0 {
            return Err(StateError::InvalidState { object_id: surface });
        }
        let limit = self.limits.max_damage_rects_per_commit;
        let state = self.surface_mut(connection, surface)?;
        if state.pending.damage.len() >= limit {
            return Err(StateError::QuotaExceeded { object_id: surface });
        }
        state.pending.damage.push(Damage::Rect(rect));
        Ok(())
    }

    fn commit(&mut self, connection: ConnectionId, surface: ObjectId) -> Result<(), StateError> {
        if self.next_commit == u64::MAX {
            return Err(StateError::QuotaExceeded { object_id: surface });
        }
        let key = SurfaceKey {
            connection,
            object_id: surface,
        };

        // A commit that attaches nothing keeps the content the surface already has. That is what
        // lets a client change damage, refresh range or a role's state without redrawing, and it
        // is why the attachment is a three-way choice rather than a missing value.
        let carried = match self.surface(connection, surface)?.pending.attachment {
            PendingAttachment::Set(buffer, offset) => Some((buffer, offset)),
            PendingAttachment::Unchanged => self
                .surface_snapshot(connection, surface)
                .map(|snapshot| (snapshot.buffer, snapshot.offset)),
            PendingAttachment::Detach => None,
        };

        if self.surface(connection, surface)?.pending.attachment == PendingAttachment::Detach {
            self.detach(key)?;
        }

        let Some((buffer, offset)) = carried else {
            // Nothing to show: either the surface was detached, or it has never had content and
            // this commit did not give it any. Both are ordinary states, not errors.
            let state = self.surface_mut(connection, surface)?;
            state.pending = PendingSurface::default();
            return Ok(());
        };

        let (damage, refresh_range, frame_callback, frame_serial, role, acquire_fence) = {
            let state = self.surface_mut(connection, surface)?;
            state.pending.attachment = PendingAttachment::Unchanged;
            let damage = if state.pending.damage.is_empty() {
                alloc::vec![Damage::Full]
            } else {
                take(&mut state.pending.damage)
            };
            let refresh_range = state.pending.refresh_range.take().or_else(|| {
                state
                    .current
                    .as_ref()
                    .and_then(|value| value.snapshot.refresh_range)
            });
            let acquire_fence = state.pending.acquire_fence.take();
            let callback = take(&mut state.frame_requested);
            let frame_serial = state.frame_serial;
            (
                damage,
                refresh_range,
                callback,
                frame_serial,
                state.role,
                acquire_fence,
            )
        };
        let commit = CommitId(self.next_commit);
        self.next_commit += 1;
        self.set_buffer_state(
            connection,
            buffer,
            BufferState::CompositorHeld {
                surface: key,
                commit,
            },
        )?;
        let state = self.surface_mut(connection, surface)?;
        state.current = Some(CurrentSurface {
            snapshot: SurfaceSnapshot {
                role,
                buffer,
                offset,
                damage,
                commit,
                refresh_range,
                acquire_fence,
            },
            frame_callback,
            frame_serial,
        });
        self.map(key);
        Ok(())
    }

    /// Take a surface's content away and remove it from the scene.
    ///
    /// The buffer the surface was holding goes back to the client, because nothing is showing it
    /// any more and a buffer the compositor holds forever is a buffer the client can never reuse.
    /// The surface's position is kept: a client that detaches and attaches again is the same
    /// window, and a shell that placed it should not have to place it twice.
    fn detach(&mut self, surface: SurfaceKey) -> Result<(), StateError> {
        let Some(held) = self
            .surface_snapshot(surface.connection, surface.object_id)
            .map(|snapshot| snapshot.buffer)
        else {
            return Ok(());
        };
        self.set_buffer_state(surface.connection, held, BufferState::Available)?;
        self.push_event(surface.connection, held, EventKind::BufferRelease);
        self.surface_mut(surface.connection, surface.object_id)?
            .current = None;
        self.scene.stack.retain(|entry| *entry != surface);
        Ok(())
    }

    /// Put a surface that now has content into the scene, if it is not there already.
    ///
    /// Content is what makes a surface part of the scene. Nothing else can do it: with no shell
    /// attached there is no component whose job is placing windows, and a compositor that showed
    /// nothing until one arrived would not compose without a shell. The origin is where a surface
    /// starts, and a shell that cares moves it.
    fn map(&mut self, surface: SurfaceKey) {
        if self.scene.stack.contains(&surface) {
            return;
        }
        self.scene.positions.entry(surface).or_default();
        self.scene.stack.push(surface);
    }

    fn set_role(
        &mut self,
        connection: ConnectionId,
        surface: ObjectId,
        role: SurfaceRole,
    ) -> Result<(), StateError> {
        let state = self.surface_mut(connection, surface)?;
        match state.role {
            None => {
                state.role = Some(role);
                Ok(())
            }
            Some(existing) if existing == role => Ok(()),
            Some(_) => Err(StateError::InvalidState { object_id: surface }),
        }
    }

    fn request_frame(
        &mut self,
        connection: ConnectionId,
        surface: ObjectId,
        serial: u32,
    ) -> Result<(), StateError> {
        let state = self.surface_mut(connection, surface)?;
        if state.frame_requested {
            return Err(StateError::InvalidState { object_id: surface });
        }
        state.frame_requested = true;
        state.frame_serial = serial;
        Ok(())
    }

    fn set_refresh_range(
        &mut self,
        connection: ConnectionId,
        surface: ObjectId,
        min_hz: u32,
        max_hz: u32,
    ) -> Result<(), StateError> {
        if min_hz == 0 || min_hz > max_hz {
            return Err(StateError::InvalidState { object_id: surface });
        }
        self.surface_mut(connection, surface)?.pending.refresh_range = Some((min_hz, max_hz));
        Ok(())
    }

    /// Marks the current commit presented and releases older buffers for this surface.
    pub fn present(
        &mut self,
        surface: SurfaceKey,
        timestamp_ns: u64,
    ) -> Result<CommitId, StateError> {
        let current = self
            .surface(surface.connection, surface.object_id)?
            .current
            .as_ref()
            .ok_or(StateError::InvalidState {
                object_id: surface.object_id,
            })?;
        let commit = current.snapshot.commit;
        let callback = current.frame_callback;
        let frame_serial = current.frame_serial;
        if let Some(fence) = current.snapshot.acquire_fence {
            let Object::Fence(fence_state) =
                self.object(surface.connection, fence, ObjectKind::Fence)?
            else {
                return Err(StateError::InvalidState { object_id: fence });
            };
            if !fence_state.signaled {
                return Err(StateError::InvalidState {
                    object_id: surface.object_id,
                });
            }
        }
        let ids: Vec<_> = self
            .connection(surface.connection)?
            .registry
            .ids()
            .collect();
        for id in ids {
            let release = matches!(
                self.buffer(surface.connection, id).map(|value| value.state),
                Ok(BufferState::CompositorHeld { surface: held_surface, commit: held_commit })
                    if held_surface == surface && held_commit < commit
            );
            if release {
                self.set_buffer_state(surface.connection, id, BufferState::Available)?;
                self.push_event(surface.connection, id, EventKind::BufferRelease);
            }
        }
        if callback {
            self.push_event(
                surface.connection,
                surface.object_id,
                EventKind::FrameDone {
                    serial: frame_serial,
                    timestamp_ns,
                },
            );
            if let Some(current) = self
                .surface_mut(surface.connection, surface.object_id)?
                .current
                .as_mut()
            {
                current.frame_callback = false;
            }
        }
        self.surface_mut(surface.connection, surface.object_id)?
            .consecutive_deadline_misses = 0;
        Ok(commit)
    }

    /// Signals an acquire or release fence. Platform waiting remains a backend concern.
    pub fn signal_fence(
        &mut self,
        connection: ConnectionId,
        fence: ObjectId,
    ) -> Result<(), StateError> {
        let object = self.object_mut(connection, fence, ObjectKind::Fence)?;
        let Object::Fence(state) = object else {
            return Err(StateError::InvalidState { object_id: fence });
        };
        state.signaled = true;
        Ok(())
    }

    pub fn commit_ready(&self, surface: SurfaceKey) -> Result<bool, StateError> {
        let current = self
            .surface(surface.connection, surface.object_id)?
            .current
            .as_ref()
            .ok_or(StateError::InvalidState {
                object_id: surface.object_id,
            })?;
        match current.snapshot.acquire_fence {
            None => Ok(true),
            Some(fence) => {
                let Object::Fence(state) =
                    self.object(surface.connection, fence, ObjectKind::Fence)?
                else {
                    return Err(StateError::InvalidState { object_id: fence });
                };
                Ok(state.signaled)
            }
        }
    }

    /// Releases a held buffer when scanout or composition is complete.
    pub fn release_buffer(
        &mut self,
        connection: ConnectionId,
        buffer: ObjectId,
    ) -> Result<(), StateError> {
        if !matches!(
            self.buffer(connection, buffer)?.state,
            BufferState::CompositorHeld { .. }
        ) {
            return Err(StateError::InvalidState { object_id: buffer });
        }
        self.set_buffer_state(connection, buffer, BufferState::Available)?;
        self.push_event(connection, buffer, EventKind::BufferRelease);
        Ok(())
    }

    pub fn release_buffer_with_fence(
        &mut self,
        connection: ConnectionId,
        buffer: ObjectId,
        fence: ObjectId,
    ) -> Result<(), StateError> {
        self.require_capability(connection, buffer, Capability::ExplicitGpuFence)?;
        self.expect_kind(connection, fence, ObjectKind::Fence)?;
        if !matches!(
            self.buffer(connection, buffer)?.state,
            BufferState::CompositorHeld { .. }
        ) {
            return Err(StateError::InvalidState { object_id: buffer });
        }
        self.set_buffer_state(connection, buffer, BufferState::Available)?;
        self.push_event(
            connection,
            buffer,
            EventKind::BufferReleaseWithFence { fence },
        );
        Ok(())
    }

    pub fn set_frame_deadline(
        &mut self,
        surface: SurfaceKey,
        deadline_ns: u64,
        refresh_interval_ns: u64,
    ) -> Result<(), StateError> {
        self.surface(surface.connection, surface.object_id)?;
        if refresh_interval_ns == 0 {
            return Err(StateError::InvalidState {
                object_id: surface.object_id,
            });
        }
        self.push_event(
            surface.connection,
            surface.object_id,
            EventKind::FrameDeadline {
                deadline_ns,
                refresh_interval_ns,
            },
        );
        Ok(())
    }

    pub fn record_deadline_miss(&mut self, surface: SurfaceKey) -> Result<(), StateError> {
        let state = self.surface_mut(surface.connection, surface.object_id)?;
        state.consecutive_deadline_misses = state.consecutive_deadline_misses.saturating_add(1);
        if state.consecutive_deadline_misses == 2 {
            self.push_event(surface.connection, surface.object_id, EventKind::FrameLate);
        }
        Ok(())
    }

    pub fn place_surface(
        &mut self,
        surface: SurfaceKey,
        position: Point,
    ) -> Result<(), StateError> {
        self.surface(surface.connection, surface.object_id)?;
        self.scene.positions.insert(surface, position);
        if !self.scene.stack.contains(&surface) {
            self.scene.stack.push(surface);
        }
        Ok(())
    }

    pub fn raise_surface(&mut self, surface: SurfaceKey) -> Result<(), StateError> {
        self.surface(surface.connection, surface.object_id)?;
        self.scene.stack.retain(|value| *value != surface);
        self.scene.stack.push(surface);
        Ok(())
    }

    #[must_use]
    pub fn stacking_order(&self) -> &[SurfaceKey] {
        &self.scene.stack
    }

    #[must_use]
    pub fn surface_position(&self, surface: SurfaceKey) -> Option<Point> {
        self.scene.positions.get(&surface).copied()
    }

    pub fn update_pointer_target(&mut self, point: Point) -> Option<SurfaceKey> {
        self.pointer_target = self.scene.stack.iter().rev().copied().find(|surface| {
            let Some(position) = self.scene.positions.get(surface) else {
                return false;
            };
            let Ok(state) = self.surface(surface.connection, surface.object_id) else {
                return false;
            };
            let Some(current) = &state.current else {
                return false;
            };
            let Ok(buffer) = self.buffer(surface.connection, current.snapshot.buffer) else {
                return false;
            };
            let Some(x) = position.x.checked_add(current.snapshot.offset.x) else {
                return false;
            };
            let Some(y) = position.y.checked_add(current.snapshot.offset.y) else {
                return false;
            };
            Rect {
                x,
                y,
                width: buffer.descriptor.size.width,
                height: buffer.descriptor.size.height,
            }
            .contains(point)
        });
        self.pointer_target
    }

    #[must_use]
    pub const fn pointer_target(&self) -> Option<SurfaceKey> {
        self.pointer_target
    }

    #[must_use]
    pub const fn keyboard_target(&self) -> Option<SurfaceKey> {
        self.keyboard_focus
    }

    /// Returns one atomic ordered batch: leave is always before enter.
    pub fn set_keyboard_focus(
        &mut self,
        new_focus: Option<SurfaceKey>,
    ) -> Result<Vec<FocusEvent>, StateError> {
        if let Some(surface) = new_focus {
            self.focusable(surface)?;
        }
        if self.keyboard_focus == new_focus {
            return Ok(Vec::new());
        }
        let mut events = Vec::with_capacity(2);
        if let Some(old) = self.keyboard_focus {
            events.push(FocusEvent::KeyboardLeave(old));
            self.push_event(old.connection, old.object_id, EventKind::KeyboardLeave);
        }
        if let Some(new) = new_focus {
            events.push(FocusEvent::KeyboardEnter(new));
            self.push_event(new.connection, new.object_id, EventKind::KeyboardEnter);
        }
        self.keyboard_focus = new_focus;
        Ok(events)
    }

    pub fn set_session_active(
        &mut self,
        connection: ConnectionId,
        session: ObjectId,
        active: bool,
    ) -> Result<Vec<FocusEvent>, StateError> {
        let object = self.object_mut(connection, session, ObjectKind::Session)?;
        let Object::Session(state) = object else {
            return Err(StateError::InvalidState { object_id: session });
        };
        state.active = active;
        let mut focus_events = Vec::new();
        if !active {
            if self.keyboard_focus.is_some_and(|key| {
                self.surface(key.connection, key.object_id)
                    .is_ok_and(|surface| key.connection == connection && surface.session == session)
            }) {
                if let Some(old) = self.keyboard_focus {
                    focus_events.push(FocusEvent::KeyboardLeave(old));
                    self.push_event(old.connection, old.object_id, EventKind::KeyboardLeave);
                }
                self.keyboard_focus = None;
            }
            if self.pointer_target.is_some_and(|key| {
                self.surface(key.connection, key.object_id)
                    .is_ok_and(|surface| key.connection == connection && surface.session == session)
            }) {
                self.pointer_target = None;
            }
        }
        Ok(focus_events)
    }

    pub fn set_seat_active(
        &mut self,
        connection: ConnectionId,
        seat: ObjectId,
        active: bool,
    ) -> Result<(), StateError> {
        let object = self.object_mut(connection, seat, ObjectKind::Seat)?;
        let Object::Seat(state) = object else {
            return Err(StateError::InvalidState { object_id: seat });
        };
        state.active = active;
        Ok(())
    }

    pub fn set_seat_capabilities(
        &mut self,
        connection: ConnectionId,
        seat: ObjectId,
        capabilities: SeatCapabilities,
    ) -> Result<(), StateError> {
        let object = self.object_mut(connection, seat, ObjectKind::Seat)?;
        let Object::Seat(state) = object else {
            return Err(StateError::InvalidState { object_id: seat });
        };
        state.capabilities = capabilities;
        Ok(())
    }

    #[must_use]
    pub fn surface_snapshot(
        &self,
        connection: ConnectionId,
        surface: ObjectId,
    ) -> Option<&SurfaceSnapshot> {
        self.surface(connection, surface)
            .ok()?
            .current
            .as_ref()
            .map(|value| &value.snapshot)
    }

    /// Advertise a global, returning the name the registry will give it.
    ///
    /// Which globals exist is the compositor's decision rather than a request, and it is made
    /// once: a name is stable while the global is advertised, and is never reused after it is
    /// withdrawn, because a client may still have that name in flight.
    pub fn advertise_global(&mut self, kind: ObjectKind, version: u32) -> Option<u32> {
        let interface = interface_of(kind)?;
        let name = self.next_global;
        self.next_global = self.next_global.checked_add(1)?;
        self.globals.insert(
            name,
            Global {
                interface,
                kind,
                version,
                output: None,
            },
        );
        Some(name)
    }

    /// The displays this compositor is composing for.
    #[must_use]
    pub const fn outputs(&self) -> &OutputSet {
        &self.outputs
    }

    /// Apply one whole topology change, and make the globals match it.
    ///
    /// A display that arrived or returned gains a global; one that departed loses its own. Every
    /// connected client is told, because a client that bound a display which has gone is holding
    /// an object describing something that is no longer there.
    ///
    /// Returns what the change did, for a shell that has to rearrange around it.
    pub fn apply_topology(
        &mut self,
        present: &[(OutputIdentity, IdentitySource, OutputInfo)],
        version: u32,
    ) -> TopologyDelta {
        let delta = self.outputs.apply(present);

        for identity in delta.arrived.iter().chain(&delta.returned) {
            if self.global_for_output(*identity).is_some() {
                continue;
            }
            let Some(name) = self.advertise_global(ObjectKind::Output, version) else {
                continue;
            };
            if let Some(global) = self.globals.get_mut(&name) {
                global.output = Some(*identity);
            }
            self.announce_global(name);
        }

        for identity in &delta.departed {
            if let Some(name) = self.global_for_output(*identity) {
                self.withdraw_global(name);
            }
        }

        delta
    }

    /// Place a display in the scene, and tell anyone who bound it where it now is.
    pub fn place_output(&mut self, identity: OutputIdentity, position: Point) -> bool {
        if !self.outputs.place(identity, position) {
            return false;
        }
        let Some(name) = self.global_for_output(identity) else {
            return true;
        };
        self.redescribe_output(name);
        true
    }

    /// Describe a display again to every connection that bound it.
    fn redescribe_output(&mut self, name: u32) {
        let bound: Vec<_> = self
            .connections
            .iter()
            .flat_map(|(connection, client)| {
                client
                    .registry
                    .ids()
                    .filter(|id| client.registry.kind_of(*id) == Some(ObjectKind::Output))
                    .map(move |id| (*connection, id))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (connection, object) in bound {
            self.describe_output(connection, object, name);
        }
    }

    /// The global offering a display, if one is advertised.
    fn global_for_output(&self, identity: OutputIdentity) -> Option<u32> {
        self.globals
            .iter()
            .find(|(_, global)| global.output == Some(identity))
            .map(|(name, _)| *name)
    }

    /// Tell a connection everything about the display an object it just bound names.
    ///
    /// Five events and then `done`. A display's properties are separate events because they
    /// change independently, and `done` is what says the description is whole — a client that
    /// acted on each as it arrived would act on a display half-described.
    fn describe_output(&mut self, connection: ConnectionId, object: ObjectId, name: u32) {
        let Some(identity) = self.globals.get(&name).and_then(|global| global.output) else {
            return;
        };
        let Some(info) = self.outputs.info(identity) else {
            return;
        };
        let source = self
            .outputs
            .source(identity)
            .unwrap_or(IdentitySource::Position);
        let position = self
            .outputs
            .area(identity)
            .map_or(Point::default(), |area| Point {
                x: area.x,
                y: area.y,
            });

        self.push_event(
            connection,
            object,
            EventKind::OutputIdentity {
                identity: identity.0,
            },
        );
        self.push_event(
            connection,
            object,
            EventKind::OutputGeometry {
                position,
                physical_millimetres: info.physical_millimetres,
                identity_source: source,
            },
        );
        self.push_event(
            connection,
            object,
            EventKind::OutputMode {
                size: info.size,
                refresh_millihertz: info.refresh_millihertz,
            },
        );
        self.push_event(
            connection,
            object,
            EventKind::OutputScale {
                numerator: info.scale_numerator,
                denominator: info.scale_denominator,
            },
        );
        self.push_event(connection, object, EventKind::OutputDone);
    }

    /// Tell a shell the world as it stands, then say the telling is over.
    ///
    /// A shell attaching to a compositor that has been running finds windows already there. It is
    /// told about each by handle, and then told the snapshot is complete — after which the same
    /// events mean "this just happened". Without that boundary a shell would have to enumerate
    /// the world and receive changes to it at the same time, and race the compositor for its own
    /// starting picture.
    ///
    /// This is what makes a replacement shell possible at all: it is the difference between
    /// restarting the shell and restarting the session.
    fn snapshot_for_shell(&mut self, connection: ConnectionId, control: ObjectId) {
        for record in self.shell_toplevels() {
            self.push_event(
                connection,
                control,
                EventKind::ShellToplevel {
                    handle: record.handle,
                    title: record.title,
                },
            );
        }
        // The shell is told which window holds focus, not which surface: a surface is a client's
        // own object and the shell has never seen its identifier.
        let focused = self.keyboard_focus.and_then(|surface| {
            self.shell_toplevels().into_iter().find_map(|record| {
                (record.connection == surface.connection
                    && self.toplevel_surface(record.connection, record.object_id)
                        == Some(surface.object_id))
                .then_some(record.handle)
            })
        });
        self.push_event(
            connection,
            control,
            EventKind::ShellFocusChanged {
                handle: focused.unwrap_or(ToplevelHandle(0)),
            },
        );
        self.push_event(connection, control, EventKind::ShellSnapshotDone);
    }

    /// Tell every connection one global exists. Used when one appears after clients have bound.
    fn announce_global(&mut self, name: u32) {
        let Some(global) = self.globals.get(&name).copied() else {
            return;
        };
        let connections: Vec<_> = self.connections.keys().copied().collect();
        for connection in connections {
            self.push_event(
                connection,
                ObjectId::DISPLAY,
                EventKind::Global {
                    name,
                    interface: global.interface,
                    version: global.version,
                },
            );
        }
    }

    /// Withdraw a global. Objects already bound from it become inert rather than invalid.
    pub fn withdraw_global(&mut self, name: u32) -> bool {
        if self.globals.remove(&name).is_none() {
            return false;
        }
        let connections: Vec<_> = self.connections.keys().copied().collect();
        for connection in connections {
            self.push_event(
                connection,
                ObjectId::DISPLAY,
                EventKind::GlobalRemove { name },
            );
        }
        true
    }

    /// Every global the registry advertises, as the events a fresh registry receives.
    fn announce_globals(&mut self, connection: ConnectionId) {
        let announcements: Vec<_> = self
            .globals
            .iter()
            .map(|(name, global)| (*name, global.interface, global.version))
            .collect();
        for (name, interface, version) in announcements {
            self.push_event(
                connection,
                ObjectId::DISPLAY,
                EventKind::Global {
                    name,
                    interface,
                    version,
                },
            );
        }
    }

    /// Take a global at an identifier the client chose.
    fn bind_global(
        &mut self,
        connection: ConnectionId,
        name: u32,
        version: u32,
        new_id: ObjectId,
    ) -> Result<(), StateError> {
        let global = *self
            .globals
            .get(&name)
            .ok_or(StateError::InvalidState { object_id: new_id })?;
        // §12.1: a client binds a version the compositor advertised. Binding higher would have it
        // sending operations the compositor does not implement, and there is no version to
        // negotiate down to afterwards — the bound version is fixed for the object's life.
        if version == 0 || version > global.version {
            return Err(StateError::InvalidState { object_id: new_id });
        }
        // The authority to arrange every window is not something binding confers. Refusing here
        // as well as on each request means an ungranted program never holds the object at all.
        if global.kind == ObjectKind::ShellControl
            && !self
                .capabilities(connection)
                .is_some_and(|held| held.contains(Capability::ShellControl))
        {
            return Err(StateError::UnsupportedCapability {
                object_id: new_id,
                capability: Capability::ShellControl,
            });
        }
        self.allocate_client(connection, new_id, global.kind, Object::Global)?;
        // A bound display describes itself immediately. A client that had to ask would have a
        // window on an output whose size it does not yet know.
        if global.kind == ObjectKind::Output {
            self.describe_output(connection, new_id, name);
        }
        if global.kind == ObjectKind::ShellControl {
            self.snapshot_for_shell(connection, new_id);
        }
        Ok(())
    }

    /// The memory a pool covers, as resolution validated it.
    ///
    /// Read rather than mapped: a buffer carved from this pool is checked against this size, and
    /// the backend behind the resource decides when anything is mapped.
    #[must_use]
    pub fn pool_memory(&self, connection: ConnectionId, id: ObjectId) -> Option<SharedMemory> {
        let entry = self.connections.get(&connection)?.registry.entry(id).ok()?;
        match &entry.value {
            Object::ShmPool(memory) => Some(*memory),
            _ => None,
        }
    }

    /// What kind of object `id` is, for a caller binding a frame addressed to it.
    #[must_use]
    pub fn object_kind(&self, connection: ConnectionId, id: ObjectId) -> Option<ObjectKind> {
        self.connections.get(&connection)?.registry.kind_of(id)
    }

    pub fn surface_role(&self, connection: ConnectionId, surface: ObjectId) -> Option<SurfaceRole> {
        self.surface(connection, surface).ok()?.role
    }

    #[must_use]
    pub fn session_snapshot(
        &self,
        connection: ConnectionId,
        session: ObjectId,
    ) -> Option<SessionSnapshot> {
        let Object::Session(state) = self.object(connection, session, ObjectKind::Session).ok()?
        else {
            return None;
        };
        Some(SessionSnapshot {
            seat: state.seat,
            active: state.active,
        })
    }

    #[must_use]
    pub fn seat_snapshot(&self, connection: ConnectionId, seat: ObjectId) -> Option<SeatSnapshot> {
        let Object::Seat(state) = self.object(connection, seat, ObjectKind::Seat).ok()? else {
            return None;
        };
        Some(SeatSnapshot {
            capabilities: state.capabilities,
            active: state.active,
        })
    }

    #[must_use]
    pub fn buffer_state(&self, connection: ConnectionId, buffer: ObjectId) -> Option<BufferState> {
        self.buffer(connection, buffer)
            .ok()
            .map(|value| value.state)
    }

    /// Returns validated metadata for a live imported buffer.
    #[must_use]
    pub fn buffer_descriptor(
        &self,
        connection: ConnectionId,
        buffer: ObjectId,
    ) -> Option<BufferDescriptor> {
        self.buffer(connection, buffer)
            .ok()
            .map(|value| value.descriptor)
    }

    /// Queue one event for a connection, or disconnect a connection that has stopped reading.
    ///
    /// The outgoing queue is shared, so a client that never drains would otherwise grow memory
    /// every other connection has to live beside. Reaching the bound is not a protocol violation
    /// — the client sent nothing wrong — so it produces a resource error and a close rather than
    /// being treated as a bad message. The error itself bypasses the bound, because a client
    /// being disconnected has to be told why.
    fn push_event(&mut self, connection: ConnectionId, object_id: ObjectId, kind: EventKind) {
        let Some(client) = self.connections.get_mut(&connection) else {
            // Nothing to deliver to. The only events addressed to a departed connection are the
            // ones its own teardown would produce, and `close_connection` returns those instead.
            return;
        };
        if client.pending_events >= self.limits.max_pending_events {
            let queued = client.pending_events;
            self.events.push(Event {
                connection,
                object_id: ObjectId::DISPLAY,
                kind: EventKind::Error(StateError::EventQueueOverflow { queued }),
            });
            let _ = self.close_connection(connection);
            return;
        }
        client.pending_events += 1;
        self.events.push(Event {
            connection,
            object_id,
            kind,
        });
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        for client in self.connections.values_mut() {
            client.pending_events = 0;
        }
        take(&mut self.events)
    }

    pub fn close_connection(&mut self, connection: ConnectionId) -> Teardown {
        let Some(mut client) = self.connections.remove(&connection) else {
            return Teardown::default();
        };
        let ids: Vec<_> = client.registry.ids_reverse_creation().collect();
        let mut destroyed = Vec::new();
        for id in ids {
            if let Some(entry) = client.registry.remove(id) {
                destroyed.push((id, entry.kind));
            }
        }
        // A connection going takes its windows with it, so the handles a shell held for them are
        // retired. A shell asking about one afterwards is told the window is gone rather than
        // reaching whatever next occupied the number.
        self.toplevels.retain(|_, (owner, _)| *owner != connection);
        self.scene
            .positions
            .retain(|key, _| key.connection != connection);
        self.scene.stack.retain(|key| key.connection != connection);
        if self
            .keyboard_focus
            .is_some_and(|key| key.connection == connection)
        {
            self.keyboard_focus = None;
        }
        if self
            .pointer_target
            .is_some_and(|key| key.connection == connection)
        {
            self.pointer_target = None;
        }
        Teardown { destroyed }
    }

    fn destroy_object(
        &mut self,
        connection: ConnectionId,
        object_id: ObjectId,
        destroyed: &mut Vec<(ObjectId, ObjectKind)>,
    ) {
        let Ok(kind) = self
            .connection(connection)
            .and_then(|c| c.registry.kind(object_id))
        else {
            return;
        };
        if kind == ObjectKind::Session {
            let children: Vec<_> = self
                .connection(connection)
                .map(|client| {
                    client
                        .registry
                        .ids_reverse_creation()
                        .filter(|id| {
                            self.surface(connection, *id)
                                .is_ok_and(|surface| surface.session == object_id)
                        })
                        .collect()
                })
                .unwrap_or_default();
            for child in children {
                self.destroy_object(connection, child, destroyed);
            }
        }
        if kind == ObjectKind::Seat {
            let sessions: Vec<_> = self
                .connection(connection)
                .map(|client| {
                    client
                        .registry
                        .ids_reverse_creation()
                        .filter(|id| {
                            matches!(
                                self.object(connection, *id, ObjectKind::Session),
                                Ok(Object::Session(SessionState { seat: Some(seat), .. }))
                                    if *seat == object_id
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            for session in sessions {
                self.destroy_object(connection, session, destroyed);
            }
        }
        if kind == ObjectKind::Surface {
            let key = SurfaceKey {
                connection,
                object_id,
            };
            let buffer_ids: Vec<_> = self
                .connection(connection)
                .map(|client| client.registry.ids().collect())
                .unwrap_or_default();
            for buffer_id in buffer_ids {
                let related = self.buffer(connection, buffer_id).is_ok_and(|buffer| {
                    matches!(
                        buffer.state,
                        BufferState::Pending { surface } | BufferState::CompositorHeld { surface, .. }
                            if surface == key
                    )
                });
                if related {
                    let _ = self.set_buffer_state(connection, buffer_id, BufferState::Available);
                    self.push_event(connection, buffer_id, EventKind::BufferRelease);
                }
            }
            self.scene.positions.remove(&key);
            self.scene.stack.retain(|value| *value != key);
            if self.keyboard_focus == Some(key) {
                self.push_event(connection, object_id, EventKind::KeyboardLeave);
                self.keyboard_focus = None;
            }
            if self.pointer_target == Some(key) {
                self.pointer_target = None;
            }
        }
        if kind == ObjectKind::Buffer {
            if let Some(client) = self.connections.get_mut(&connection) {
                for entry in client.registry.live.values_mut() {
                    if let Object::Surface(surface) = &mut entry.value {
                        if matches!(
                            surface.pending.attachment,
                            PendingAttachment::Set(id, _) if id == object_id
                        ) {
                            surface.pending.attachment = PendingAttachment::Unchanged;
                        }
                        if surface
                            .current
                            .as_ref()
                            .is_some_and(|current| current.snapshot.buffer == object_id)
                        {
                            surface.current = None;
                        }
                    }
                }
            }
        }
        if kind == ObjectKind::Fence {
            let dependents: Vec<_> = self
                .connection(connection)
                .map(|client| {
                    client
                        .registry
                        .ids_reverse_creation()
                        .filter(|id| {
                            self.surface(connection, *id).is_ok_and(|surface| {
                                surface.pending.acquire_fence == Some(object_id)
                                    || surface.current.as_ref().is_some_and(|current| {
                                        current.snapshot.acquire_fence == Some(object_id)
                                    })
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Fence destruction while referenced is unspecified. Keep synchronization sound
            // by terminating dependent surfaces instead of silently dropping the fence.
            for dependent in dependents {
                self.destroy_object(connection, dependent, destroyed);
            }
        }
        // Retired before the object goes, and never reissued: a shell's request already in flight
        // must not land on a different window than the one it named.
        if kind == ObjectKind::Toplevel
            && let Some(handle) = self.toplevel_handle(connection, object_id)
        {
            self.toplevels.remove(&handle);
        }
        if let Some(client) = self.connections.get_mut(&connection) {
            if client.registry.remove(object_id).is_some() {
                destroyed.push((object_id, kind));
                self.push_event(connection, object_id, EventKind::ObjectDestroyed(kind));
                // Queued after the destruction the client is being told about, so it learns the
                // object is gone before it learns the number is free again.
                self.push_event(connection, object_id, EventKind::IdRetired);
                if let Some(client) = self.connections.get_mut(&connection) {
                    client.registry.retire(object_id);
                }
            }
        }
    }

    fn object_requires(
        &self,
        connection: ConnectionId,
        id: ObjectId,
        capability: Capability,
    ) -> bool {
        let Ok(kind) = self
            .connection(connection)
            .and_then(|c| c.registry.kind(id))
        else {
            return false;
        };
        matches!(
            (kind, capability),
            (ObjectKind::Surface, Capability::SurfaceCreate)
                | (ObjectKind::Buffer, Capability::BufferImport)
                | (ObjectKind::Fence, Capability::ExplicitGpuFence)
                | (ObjectKind::InputStream, Capability::ExtendedInput)
                | (ObjectKind::Session, Capability::PrivilegedSession)
                | (ObjectKind::Seat, Capability::RevocableSeat)
        ) || (kind == ObjectKind::Buffer
            && self
                .buffer(connection, id)
                .is_ok_and(|buffer| buffer.descriptor.transport.capability() == capability))
    }

    fn focusable(&self, key: SurfaceKey) -> Result<(), StateError> {
        let surface = self.surface(key.connection, key.object_id)?;
        // The draft does not define when a surface becomes focus-eligible. Requiring an active,
        // mapped surface prevents input from reaching invisible or inactive client state.
        if surface.current.is_none() || !self.scene.positions.contains_key(&key) {
            return Err(StateError::InvalidState {
                object_id: key.object_id,
            });
        }
        self.expect_active_session(key.connection, surface.session)
    }

    /// Carve a buffer out of a pool, checking it lies inside the memory the pool covers.
    ///
    /// The pool's size came from the descriptor the client handed over, not from anything it
    /// said, so this is the check that a buffer describes memory that exists. Without it a
    /// client could name a region past the end of its own mapping and the compositor would read
    /// it.
    #[expect(
        clippy::too_many_arguments,
        reason = "every field is one wire argument of create_buffer, and grouping them into a                   struct would only move the same list somewhere the message does not describe"
    )]
    fn create_buffer(
        &mut self,
        connection: ConnectionId,
        new_id: ObjectId,
        pool: ObjectId,
        offset: u32,
        size: Size,
        stride: u32,
        format: u32,
    ) -> Result<(), StateError> {
        self.require_capability(connection, new_id, Capability::BufferImport)?;
        let memory = self
            .pool_memory(connection, pool)
            .ok_or(StateError::WrongObjectType {
                object_id: pool,
                expected: ObjectKind::ShmPool,
                actual: self
                    .object_kind(connection, pool)
                    .ok_or(StateError::InvalidObject { object_id: pool })?,
            })?;

        let descriptor = BufferDescriptor {
            transport: BufferTransport::SoftwareShm,
            size,
            stride,
            byte_len: u64::from(stride).saturating_mul(u64::from(size.height)),
        };
        if !descriptor.is_structurally_valid() || !pixel_format_is_known(format) {
            return Err(StateError::InvalidState { object_id: new_id });
        }
        // Checked as one sum rather than two comparisons: a large offset and a large extent each
        // fit on their own, and it is the total that has to be inside the pool.
        let end = u64::from(offset)
            .checked_add(descriptor.byte_len)
            .ok_or(StateError::InvalidState { object_id: new_id })?;
        if end > memory.size() {
            return Err(StateError::InvalidState { object_id: new_id });
        }

        self.check_kind_quota(connection, ObjectKind::Buffer)?;
        self.allocate_client(
            connection,
            new_id,
            ObjectKind::Buffer,
            Object::Buffer(BufferObject {
                descriptor,
                state: BufferState::Available,
                source: Some(BufferSource {
                    memory,
                    offset,
                    descriptor,
                }),
            }),
        )
    }

    /// Give a surface window semantics.
    ///
    /// A surface may take one role. A second is refused rather than replacing the first: the role
    /// object carries state a client is entitled to keep, and silently discarding it would make
    /// a mistake look like it worked.
    fn get_toplevel(
        &mut self,
        connection: ConnectionId,
        surface: ObjectId,
        new_id: ObjectId,
    ) -> Result<(), StateError> {
        self.expect_kind(connection, surface, ObjectKind::Surface)?;
        if self.role_of(connection, surface).is_some() {
            return Err(StateError::InvalidState { object_id: surface });
        }
        self.check_kind_quota(connection, ObjectKind::Toplevel)?;
        let handle = ToplevelHandle(self.next_handle);
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or(StateError::QuotaExceeded { object_id: new_id })?;
        self.allocate_client(
            connection,
            new_id,
            ObjectKind::Toplevel,
            Object::Toplevel(ToplevelState {
                surface,
                title: TitleText::default(),
                minimum: None,
                maximum: None,
                last_configure: 0,
                configured: None,
                handle,
            }),
        )?;
        self.toplevels.insert(handle, (connection, new_id));
        Ok(())
    }

    /// Every window that exists, as a shell refers to them.
    ///
    /// This is the restart contract in one call: a shell starting for the first time and a shell
    /// replacing one that died are handed the same thing, so neither has to be a special case and
    /// a replacement never begins from nothing.
    #[must_use]
    pub fn shell_toplevels(&self) -> Vec<ToplevelRecord> {
        self.toplevels
            .iter()
            .filter_map(|(handle, (connection, object_id))| {
                let entry = self
                    .connections
                    .get(connection)?
                    .registry
                    .entry(*object_id)
                    .ok()?;
                let Object::Toplevel(state) = &entry.value else {
                    return None;
                };
                Some(ToplevelRecord {
                    handle: *handle,
                    connection: *connection,
                    object_id: *object_id,
                    title: String::from(state.title.as_str()),
                    configured: state.configured,
                })
            })
            .collect()
    }

    /// Resolve a shell's handle, checking the shell holds the authority to use it.
    fn shell_target(
        &self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(ConnectionId, ObjectId), ShellError> {
        // Asked before the handle is looked up, so a program without the grant learns nothing
        // about which windows exist by probing handles.
        if !self
            .capabilities(shell)
            .is_some_and(|held| held.contains(Capability::ShellControl))
        {
            return Err(ShellError::NotGranted);
        }
        self.toplevels
            .get(&handle)
            .copied()
            .ok_or(ShellError::UnknownHandle { handle })
    }

    /// Ask a window to adopt a size and a set of states, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// Returns [`ShellError::NotGranted`] without the shell-control grant, and
    /// [`ShellError::UnknownHandle`] for a window that has gone.
    pub fn shell_configure(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
        size: Size,
        state: u32,
    ) -> Result<u32, ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        self.configure_toplevel(connection, object_id, size, state)
            .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Ask a window to close, on a shell's behalf. The window goes when its own client destroys it.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_close(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        self.close_toplevel(connection, object_id)
            .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Place a window's surface, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_place(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
        position: Point,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        let surface = self
            .toplevel_surface(connection, object_id)
            .ok_or(ShellError::UnknownHandle { handle })?;
        self.place_surface(
            SurfaceKey {
                connection,
                object_id: surface,
            },
            position,
        )
        .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Raise a window's surface to the front, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_raise(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        let surface = self
            .toplevel_surface(connection, object_id)
            .ok_or(ShellError::UnknownHandle { handle })?;
        self.raise_surface(SurfaceKey {
            connection,
            object_id: surface,
        })
        .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Move keyboard focus to a window, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_focus(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        let surface = self
            .toplevel_surface(connection, object_id)
            .ok_or(ShellError::UnknownHandle { handle })?;
        self.set_keyboard_focus(Some(SurfaceKey {
            connection,
            object_id: surface,
        }))
        .map(|_| ())
        .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// How a shell refers to this window.
    #[must_use]
    pub fn toplevel_handle(
        &self,
        connection: ConnectionId,
        toplevel: ObjectId,
    ) -> Option<ToplevelHandle> {
        let entry = self
            .connections
            .get(&connection)?
            .registry
            .entry(toplevel)
            .ok()?;
        match &entry.value {
            Object::Toplevel(state) => Some(state.handle),
            _ => None,
        }
    }

    /// The surface a window is laid over.
    fn toplevel_surface(&self, connection: ConnectionId, toplevel: ObjectId) -> Option<ObjectId> {
        let entry = self
            .connections
            .get(&connection)?
            .registry
            .entry(toplevel)
            .ok()?;
        match &entry.value {
            Object::Toplevel(state) => Some(state.surface),
            _ => None,
        }
    }

    /// The role object a surface already has, if any.
    fn role_of(&self, connection: ConnectionId, surface: ObjectId) -> Option<ObjectId> {
        let client = self.connections.get(&connection)?;
        client.registry.live.iter().find_map(|(id, entry)| {
            matches!(&entry.value, Object::Toplevel(state) if state.surface == surface)
                .then_some(*id)
        })
    }

    fn toplevel_mut(
        &mut self,
        connection: ConnectionId,
        toplevel: ObjectId,
    ) -> Result<&mut ToplevelState, StateError> {
        let client = self.connection_mut(connection)?;
        let entry = client.registry.entry_mut(toplevel)?;
        match &mut entry.value {
            Object::Toplevel(state) => Ok(state),
            _ => Err(StateError::WrongObjectType {
                object_id: toplevel,
                expected: ObjectKind::Toplevel,
                actual: entry.kind,
            }),
        }
    }

    /// The title a toplevel has set, for a shell that displays it.
    #[must_use]
    pub fn toplevel_title(&self, connection: ConnectionId, toplevel: ObjectId) -> Option<&str> {
        let entry = self
            .connections
            .get(&connection)?
            .registry
            .entry(toplevel)
            .ok()?;
        match &entry.value {
            Object::Toplevel(state) => Some(state.title.as_str()),
            _ => None,
        }
    }

    /// Ask a toplevel to adopt a size and a set of states.
    ///
    /// Advisory: a client may commit before receiving one and may keep drawing if none arrives,
    /// which is what lets the compositor run with no shell attached. The serial is what a commit
    /// echoes back, so a compositor that did send one can tell whether the content answers it.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::InvalidState`] when `state` sets a bit the bound interface version
    /// does not define — §12.4 refuses an undefined value rather than ignoring it.
    pub fn configure_toplevel(
        &mut self,
        connection: ConnectionId,
        toplevel: ObjectId,
        size: Size,
        state: u32,
    ) -> Result<u32, StateError> {
        if state & !toplevel_state::VALID_V1 != 0 {
            return Err(StateError::InvalidState {
                object_id: toplevel,
            });
        }
        let serial = self.next_configure;
        self.next_configure = self.next_configure.wrapping_add(1);
        let target = self.toplevel_mut(connection, toplevel)?;
        target.last_configure = serial;
        target.configured = Some(size);
        self.push_event(
            connection,
            toplevel,
            EventKind::Configure {
                serial,
                size,
                state,
            },
        );
        Ok(serial)
    }

    /// Ask a toplevel to close. The window goes away when its client destroys it.
    pub fn close_toplevel(
        &mut self,
        connection: ConnectionId,
        toplevel: ObjectId,
    ) -> Result<(), StateError> {
        self.expect_kind(connection, toplevel, ObjectKind::Toplevel)?;
        self.push_event(connection, toplevel, EventKind::Close);
        Ok(())
    }

    /// Where a buffer's pixels are, for a caller about to read them.
    #[must_use]
    pub fn buffer_source(
        &self,
        connection: ConnectionId,
        buffer: ObjectId,
    ) -> Option<BufferSource> {
        let entry = self
            .connections
            .get(&connection)?
            .registry
            .entry(buffer)
            .ok()?;
        match &entry.value {
            Object::Buffer(object) => object.source,
            _ => None,
        }
    }

    fn expect_active_session(
        &self,
        connection: ConnectionId,
        session: ObjectId,
    ) -> Result<(), StateError> {
        let object = self.object(connection, session, ObjectKind::Session)?;
        let Object::Session(session_state) = object else {
            return Err(StateError::InvalidState { object_id: session });
        };
        if !session_state.active {
            return Err(StateError::InvalidState { object_id: session });
        }
        if let Some(seat_id) = session_state.seat {
            let seat = self.object(connection, seat_id, ObjectKind::Seat)?;
            let Object::Seat(seat_state) = seat else {
                return Err(StateError::InvalidState { object_id: seat_id });
            };
            if !seat_state.active {
                return Err(StateError::InvalidState { object_id: session });
            }
        }
        Ok(())
    }

    fn require_capability(
        &self,
        connection: ConnectionId,
        object_id: ObjectId,
        capability: Capability,
    ) -> Result<(), StateError> {
        if self
            .connection(connection)?
            .capabilities
            .contains(capability)
        {
            Ok(())
        } else {
            Err(StateError::UnsupportedCapability {
                object_id,
                capability,
            })
        }
    }

    fn check_kind_quota(
        &self,
        connection: ConnectionId,
        kind: ObjectKind,
    ) -> Result<(), StateError> {
        let client = self.connection(connection)?;
        let count = client
            .registry
            .live
            .values()
            .filter(|entry| entry.kind == kind)
            .count();
        let maximum = match kind {
            ObjectKind::Surface => self.limits.max_surfaces,
            ObjectKind::Buffer => self
                .limits
                .max_buffers
                .min(self.limits.max_imported_handles),
            // A pool is one handed-over resource, so it is bounded by the same allowance as the
            // handles a connection may have imported.
            ObjectKind::ShmPool => self.limits.max_imported_handles,
            // One role per surface, so a connection cannot hold more windows than surfaces.
            ObjectKind::Toplevel => self.limits.max_surfaces,
            _ => self.limits.max_objects,
        };
        if count >= maximum {
            Err(StateError::QuotaExceeded {
                object_id: ObjectId::DISPLAY,
            })
        } else {
            Ok(())
        }
    }

    fn allocate_client(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
        kind: ObjectKind,
        value: Object,
    ) -> Result<(), StateError> {
        let limit = self.limits.max_objects;
        let client = self.connection_mut(connection)?;
        if client.registry.len() >= limit {
            return Err(StateError::QuotaExceeded { object_id: id });
        }
        client.registry.allocate_client(id, kind, value)
    }

    fn expect_any(&self, connection: ConnectionId, id: ObjectId) -> Result<(), StateError> {
        self.connection(connection)?.registry.entry(id).map(|_| ())
    }

    fn expect_kind(
        &self,
        connection: ConnectionId,
        id: ObjectId,
        expected: ObjectKind,
    ) -> Result<(), StateError> {
        self.object(connection, id, expected).map(|_| ())
    }

    fn object(
        &self,
        connection: ConnectionId,
        id: ObjectId,
        expected: ObjectKind,
    ) -> Result<&Object, StateError> {
        let entry = self.connection(connection)?.registry.entry(id)?;
        if entry.kind != expected {
            return Err(StateError::WrongObjectType {
                object_id: id,
                expected,
                actual: entry.kind,
            });
        }
        Ok(&entry.value)
    }

    fn object_mut(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
        expected: ObjectKind,
    ) -> Result<&mut Object, StateError> {
        let entry = self.connection_mut(connection)?.registry.entry_mut(id)?;
        if entry.kind != expected {
            return Err(StateError::WrongObjectType {
                object_id: id,
                expected,
                actual: entry.kind,
            });
        }
        Ok(&mut entry.value)
    }

    fn surface(&self, connection: ConnectionId, id: ObjectId) -> Result<&SurfaceState, StateError> {
        let Object::Surface(surface) = self.object(connection, id, ObjectKind::Surface)? else {
            return Err(StateError::InvalidState { object_id: id });
        };
        Ok(surface)
    }

    fn surface_mut(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
    ) -> Result<&mut SurfaceState, StateError> {
        let Object::Surface(surface) = self.object_mut(connection, id, ObjectKind::Surface)? else {
            return Err(StateError::InvalidState { object_id: id });
        };
        Ok(surface)
    }

    fn buffer(&self, connection: ConnectionId, id: ObjectId) -> Result<&BufferObject, StateError> {
        let Object::Buffer(buffer) = self.object(connection, id, ObjectKind::Buffer)? else {
            return Err(StateError::InvalidState { object_id: id });
        };
        Ok(buffer)
    }

    fn set_buffer_state(
        &mut self,
        connection: ConnectionId,
        id: ObjectId,
        state: BufferState,
    ) -> Result<(), StateError> {
        let Object::Buffer(buffer) = self.object_mut(connection, id, ObjectKind::Buffer)? else {
            return Err(StateError::InvalidState { object_id: id });
        };
        buffer.state = state;
        Ok(())
    }

    fn connection(&self, id: ConnectionId) -> Result<&Connection, StateError> {
        self.connections
            .get(&id)
            .ok_or(StateError::ConnectionClosed)
    }

    fn connection_mut(&mut self, id: ConnectionId) -> Result<&mut Connection, StateError> {
        self.connections
            .get_mut(&id)
            .ok_or(StateError::ConnectionClosed)
    }
}

/// A shell's refusal, as an error the connection can be told about.
///
/// Neither case says anything about the window. A connection without the grant learns nothing
/// about which handles exist — the grant is checked before the handle is resolved, so a refusal
/// cannot be used to discover what windows the compositor holds.
const fn shell_refusal(error: ShellError, object: ObjectId) -> StateError {
    match error {
        ShellError::NotGranted => StateError::UnsupportedCapability {
            object_id: object,
            capability: Capability::ShellControl,
        },
        ShellError::UnknownHandle { .. } => StateError::InvalidState { object_id: object },
    }
}
