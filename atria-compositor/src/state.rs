use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::error::Error;
use core::fmt;
use core::mem::take;

use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::error::ErrorCategory;

use crate::error::StateError;
use crate::model::{
    BufferDescriptor, BufferState, ClientRequest, CommitId, ConnectionId, Damage, Event, EventKind,
    FocusEvent, ObjectKind, Point, Rect, SeatCapabilities, SeatSnapshot, SessionSnapshot,
    SurfaceKey, SurfaceRole, SurfaceSnapshot,
};
use crate::registry::{ObjectRegistry, Teardown};
use crate::resolve::SharedMemory;

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

#[derive(Clone, Debug, Default)]
struct PendingSurface {
    attachment: Option<(ObjectId, Point)>,
    acquire_fence: Option<ObjectId>,
    damage: Vec<Damage>,
    refresh_range: Option<(u32, u32)>,
}

#[derive(Clone, Debug)]
struct CurrentSurface {
    snapshot: SurfaceSnapshot,
    frame_callback: bool,
}

#[derive(Clone, Debug)]
struct SurfaceState {
    session: ObjectId,
    role: Option<SurfaceRole>,
    pending: PendingSurface,
    current: Option<CurrentSurface>,
    frame_requested: bool,
    consecutive_deadline_misses: u8,
}

#[derive(Clone, Copy, Debug)]
struct BufferObject {
    descriptor: BufferDescriptor,
    state: BufferState,
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

/// The protocol state machine between decoded messages and a display/input backend.
#[derive(Clone, Debug)]
pub struct CompositorState {
    server_capabilities: CapabilitySet,
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
        self.allocate_client(
            connection,
            id,
            ObjectKind::Session,
            Object::Session(SessionState { seat, active }),
        )
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
                self.allocate_client(connection, new_id, ObjectKind::Registry, Object::Registry)
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
            ClientRequest::CreateSurface { session, new_id } => {
                self.require_capability(connection, new_id, Capability::SurfaceCreate)?;
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
                        consecutive_deadline_misses: 0,
                    }),
                )
            }
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
            ClientRequest::SetRole { surface, role } => self.set_role(connection, surface, role),
            ClientRequest::RequestFrame { surface } => self.request_frame(connection, surface),
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
        self.expect_kind(connection, buffer, ObjectKind::Buffer)?;
        if let Some(fence) = acquire_fence {
            self.require_capability(connection, surface, Capability::ExplicitGpuFence)?;
            self.expect_kind(connection, fence, ObjectKind::Fence)?;
        }
        let key = SurfaceKey {
            connection,
            object_id: surface,
        };
        let old_pending = self.surface(connection, surface)?.pending.attachment;
        // §7 says attach MUST be followed by commit and does not define replacement of an
        // uncommitted attachment. Rejecting a second attach avoids ambiguous release behavior.
        if old_pending.is_some() {
            return Err(StateError::InvalidState { object_id: surface });
        }
        if self.buffer(connection, buffer)?.state != BufferState::Available {
            return Err(StateError::InvalidState { object_id: buffer });
        }
        self.set_buffer_state(connection, buffer, BufferState::Pending { surface: key })?;
        let state = self.surface_mut(connection, surface)?;
        state.pending.attachment = Some((buffer, offset));
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
        let (buffer, offset, damage, refresh_range, frame_callback, role, acquire_fence) = {
            let state = self.surface_mut(connection, surface)?;
            let Some((buffer, offset)) = state.pending.attachment.take() else {
                return Err(StateError::InvalidState { object_id: surface });
            };
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
            (
                buffer,
                offset,
                damage,
                refresh_range,
                callback,
                state.role,
                acquire_fence,
            )
        };
        let commit = CommitId(self.next_commit);
        self.next_commit += 1;
        let key = SurfaceKey {
            connection,
            object_id: surface,
        };
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
        });
        Ok(())
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
    ) -> Result<(), StateError> {
        let state = self.surface_mut(connection, surface)?;
        if state.frame_requested {
            return Err(StateError::InvalidState { object_id: surface });
        }
        state.frame_requested = true;
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
                EventKind::FrameDone { timestamp_ns },
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
                        if surface
                            .pending
                            .attachment
                            .is_some_and(|(id, _)| id == object_id)
                        {
                            surface.pending.attachment = None;
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
