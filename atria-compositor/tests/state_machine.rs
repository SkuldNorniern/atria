use std::panic::{AssertUnwindSafe, catch_unwind};

use atria_compositor::{
    BufferDescriptor, BufferState, BufferTransport, ClientRequest, CompositorState, ConnectionId,
    ConnectionLimits, Damage, EventKind, FocusEvent, NegotiationError, ObjectKind, Point, Rect,
    SeatCapabilities, ServerLimits, Size, StateError, SurfaceKey, SurfaceRole,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::error::ErrorCategory;
use atria_protocol::opcode::Opcode;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn software_capabilities() -> CapabilitySet {
    CapabilitySet::default_grants().with(Capability::SoftwareShm)
}

fn descriptor(width: u32, height: u32) -> BufferDescriptor {
    BufferDescriptor {
        transport: BufferTransport::SoftwareShm,
        size: Size { width, height },
        stride: width.saturating_mul(4),
        byte_len: u64::from(width) * u64::from(height) * 4,
    }
}

fn state_with_limits(limits: ConnectionLimits) -> CompositorState {
    CompositorState::new(software_capabilities(), ServerLimits::default(), limits)
}

fn connect(state: &mut CompositorState) -> ConnectionId {
    state
        .connect(software_capabilities(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("baseline capabilities overlap: {error:?}"))
}

fn create_session(state: &mut CompositorState, connection: ConnectionId, raw: u32) {
    state
        .create_session(connection, id(raw), None, true)
        .unwrap_or_else(|error| panic!("session creation must succeed: {error:?}"));
}

fn create_surface(state: &mut CompositorState, connection: ConnectionId, surface: u32) {
    state
        .dispatch(
            connection,
            ClientRequest::CreateSurface {
                new_id: id(surface),
            },
        )
        .unwrap_or_else(|error| panic!("surface creation must succeed: {error:?}"));
}

fn import_buffer(state: &mut CompositorState, connection: ConnectionId, raw: u32) {
    state
        .dispatch(
            connection,
            ClientRequest::ImportBuffer {
                new_id: id(raw),
                descriptor: descriptor(100, 80),
            },
        )
        .unwrap_or_else(|error| panic!("buffer import must succeed: {error:?}"));
}

fn attach_and_commit(
    state: &mut CompositorState,
    connection: ConnectionId,
    surface: u32,
    buffer: u32,
) {
    state
        .dispatch(
            connection,
            ClientRequest::Attach {
                surface: id(surface),
                buffer: id(buffer),
                offset: Point::default(),
                acquire_fence: None,
            },
        )
        .unwrap_or_else(|error| panic!("attach must succeed: {error:?}"));
    state
        .dispatch(
            connection,
            ClientRequest::Commit {
                surface: id(surface),
            },
        )
        .unwrap_or_else(|error| panic!("commit must succeed: {error:?}"));
}

#[test]
fn client_cannot_reference_or_affect_another_clients_object() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client_a = connect(&mut state);
    let client_b = connect(&mut state);
    create_session(&mut state, client_b, 256);
    create_surface(&mut state, client_b, 257);
    import_buffer(&mut state, client_b, 258);
    attach_and_commit(&mut state, client_b, 257, 258);

    let result = state.dispatch(client_a, ClientRequest::Destroy { object: id(257) });

    assert_eq!(
        result,
        Err(StateError::InvalidObject { object_id: id(257) })
    );
    assert!(
        !state.is_connected(client_a),
        "protocol errors terminate only the offender"
    );
    assert!(state.is_connected(client_b));
    assert!(state.surface_snapshot(client_b, id(257)).is_some());
    assert!(matches!(
        state.buffer_state(client_b, id(258)),
        Some(BufferState::CompositorHeld { .. })
    ));
}

#[test]
fn destroying_same_numeric_id_on_one_connection_does_not_touch_another() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client_a = connect(&mut state);
    let client_b = connect(&mut state);
    for client in [client_a, client_b] {
        create_session(&mut state, client, 256);
        create_surface(&mut state, client, 257);
        import_buffer(&mut state, client, 258);
    }
    attach_and_commit(&mut state, client_b, 257, 258);

    state
        .dispatch(client_a, ClientRequest::Destroy { object: id(258) })
        .expect("client A owns its buffer");

    assert_eq!(
        state.surface_snapshot(client_b, id(257)).unwrap().buffer,
        id(258)
    );
    assert!(matches!(
        state.buffer_state(client_b, id(258)),
        Some(BufferState::CompositorHeld { .. })
    ));
}

/// A connection-scoped identifier is a `u32`, so a long-lived client that creates and destroys
/// objects has to be able to reuse them or it eventually runs out. Reuse is safe because the
/// client is told the number is free, and everything it sent naming the old object was sent
/// before it could have learned that.
#[test]
fn a_destroyed_id_is_announced_as_retired_and_may_then_be_reused() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);
    let _ = state.take_events();

    state
        .dispatch(client, ClientRequest::Destroy { object: id(257) })
        .unwrap_or_else(|error| panic!("destruction succeeds: {error:?}"));

    let kinds: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.object_id == id(257))
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            EventKind::ObjectDestroyed(ObjectKind::Surface),
            EventKind::IdRetired
        ],
        "the object is reported gone before the number is reported free"
    );

    state
        .dispatch(client, ClientRequest::CreateSurface { new_id: id(257) })
        .unwrap_or_else(|error| panic!("a retired identifier is allocatable again: {error:?}"));
    assert_eq!(
        state.object_kind(client, id(257)),
        Some(ObjectKind::Surface)
    );
    assert!(state.is_connected(client));
}

/// Reuse is only legal after retirement. An identifier that still names a live object is a
/// protocol error, because accepting it would leave two objects answering to one number.
#[test]
fn a_live_id_cannot_be_claimed_a_second_time() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);

    let error = state.dispatch(client, ClientRequest::CreateSurface { new_id: id(257) });
    assert_eq!(
        error,
        Err(StateError::ObjectIdAlreadyUsed { object_id: id(257) })
    );
    assert!(!state.is_connected(client));
}

#[test]
fn exceeding_per_connection_surface_quota_is_recoverable() {
    let mut state = state_with_limits(ConnectionLimits {
        max_surfaces: 1,
        ..ConnectionLimits::default()
    });
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);

    let result = state.dispatch(client, ClientRequest::CreateSurface { new_id: id(258) });
    assert_eq!(
        result,
        Err(StateError::QuotaExceeded {
            object_id: ObjectId::DISPLAY
        })
    );
    assert!(state.is_connected(client));
}

#[test]
fn committing_a_surface_that_has_never_had_content_shows_nothing() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);

    state
        .dispatch(client, ClientRequest::Commit { surface: id(257) })
        .unwrap_or_else(|error| panic!("a commit need not attach: {error:?}"));

    assert!(
        state.surface_snapshot(client, id(257)).is_none(),
        "a surface nothing has drawn into has no content to show"
    );
    assert!(
        state.stacking_order().is_empty(),
        "and nothing without content belongs in the scene"
    );
    assert!(state.is_connected(client));
}

#[test]
fn connection_teardown_destroys_everything_in_reverse_creation_order() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    state
        .dispatch(client, ClientRequest::CreateRegistry { new_id: id(2) })
        .expect("reserved registry singleton");
    state
        .create_seat(client, id(256), SeatCapabilities::empty(), true)
        .expect("seat creation");
    state
        .create_session(client, id(257), Some(id(256)), true)
        .expect("session creation");
    create_surface(&mut state, client, 258);
    import_buffer(&mut state, client, 259);

    let teardown = state.close_connection(client);
    assert_eq!(
        teardown.destroyed,
        vec![
            (id(259), ObjectKind::Buffer),
            (id(258), ObjectKind::Surface),
            (id(257), ObjectKind::Session),
            (id(256), ObjectKind::Seat),
            (id(2), ObjectKind::Registry),
            (ObjectId::DISPLAY, ObjectKind::Display),
        ]
    );
    assert!(!state.is_connected(client));
}

#[test]
fn pending_surface_state_does_not_replace_current_until_commit() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 258);
    import_buffer(&mut state, client, 259);
    attach_and_commit(&mut state, client, 257, 258);
    let first_commit = state
        .surface_snapshot(client, id(257))
        .expect("current state")
        .commit;

    state
        .dispatch(
            client,
            ClientRequest::Attach {
                surface: id(257),
                buffer: id(259),
                offset: Point { x: 4, y: -3 },
                acquire_fence: None,
            },
        )
        .expect("second attachment stages");
    state
        .dispatch(
            client,
            ClientRequest::Damage {
                surface: id(257),
                rect: Rect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4,
                },
            },
        )
        .expect("damage stages");
    assert_eq!(
        state.surface_snapshot(client, id(257)).unwrap().buffer,
        id(258)
    );
    assert!(matches!(
        state.buffer_state(client, id(259)),
        Some(BufferState::Pending { .. })
    ));

    state
        .dispatch(client, ClientRequest::Commit { surface: id(257) })
        .expect("second commit");
    let current = state.surface_snapshot(client, id(257)).unwrap();
    assert_eq!(current.buffer, id(259));
    assert_eq!(current.offset, Point { x: 4, y: -3 });
    assert_eq!(
        current.damage,
        vec![Damage::Rect(Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4
        })]
    );
    assert!(current.commit > first_commit);
    state
        .present(
            SurfaceKey {
                connection: client,
                object_id: id(257),
            },
            99,
        )
        .expect("presentation");
    assert_eq!(
        state.buffer_state(client, id(258)),
        Some(BufferState::Available)
    );
}

#[test]
fn keyboard_focus_change_orders_leave_before_enter_atomically() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    for (surface, buffer, x) in [(257, 259, 0), (258, 260, 25)] {
        create_surface(&mut state, client, surface);
        import_buffer(&mut state, client, buffer);
        attach_and_commit(&mut state, client, surface, buffer);
        state
            .place_surface(
                SurfaceKey {
                    connection: client,
                    object_id: id(surface),
                },
                Point { x, y: 0 },
            )
            .expect("place surface");
    }
    let first = SurfaceKey {
        connection: client,
        object_id: id(257),
    };
    let second = SurfaceKey {
        connection: client,
        object_id: id(258),
    };
    state
        .set_keyboard_focus(Some(first))
        .expect("initial focus");
    state.take_events();

    assert_eq!(
        state
            .set_keyboard_focus(Some(second))
            .expect("focus transfer"),
        vec![
            FocusEvent::KeyboardLeave(first),
            FocusEvent::KeyboardEnter(second)
        ]
    );
    assert_eq!(state.keyboard_target(), Some(second));
    let delivered = state.take_events();
    assert_eq!(delivered[0].kind, EventKind::KeyboardLeave);
    assert_eq!(delivered[0].object_id, first.object_id);
    assert_eq!(delivered[1].kind, EventKind::KeyboardEnter);
    assert_eq!(delivered[1].object_id, second.object_id);
}

#[test]
fn pointer_target_uses_topmost_surface_under_pointer() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    for (surface, buffer) in [(257, 259), (258, 260)] {
        create_surface(&mut state, client, surface);
        import_buffer(&mut state, client, buffer);
        attach_and_commit(&mut state, client, surface, buffer);
        state
            .place_surface(
                SurfaceKey {
                    connection: client,
                    object_id: id(surface),
                },
                Point::default(),
            )
            .expect("place surface");
    }
    assert_eq!(
        state.update_pointer_target(Point { x: 10, y: 10 }),
        Some(SurfaceKey {
            connection: client,
            object_id: id(258)
        })
    );
    state
        .raise_surface(SurfaceKey {
            connection: client,
            object_id: id(257),
        })
        .expect("raise");
    assert_eq!(
        state.update_pointer_target(Point { x: 10, y: 10 }),
        Some(SurfaceKey {
            connection: client,
            object_id: id(257)
        })
    );
}

#[test]
fn absent_fence_and_seat_capabilities_are_discoverable_and_not_emulated() {
    let mut state = CompositorState::new(
        software_capabilities(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let client = connect(&mut state);
    let negotiated = state.capabilities(client).expect("live client");
    assert!(!negotiated.contains(Capability::ExplicitGpuFence));
    assert!(!negotiated.contains(Capability::RevocableSeat));
    assert_eq!(
        state.create_fence(client, id(256)),
        Err(StateError::UnsupportedCapability {
            object_id: id(256),
            capability: Capability::ExplicitGpuFence,
        })
    );
    let required = CapabilitySet::empty().with(Capability::RevocableSeat);
    assert!(
        state
            .connect(
                software_capabilities().with(Capability::RevocableSeat),
                required
            )
            .is_err()
    );
}

#[test]
fn acquire_fence_blocks_presentation_until_signaled() {
    let capabilities = software_capabilities().with(Capability::ExplicitGpuFence);
    let mut state = CompositorState::new(
        capabilities,
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let client = state
        .connect(capabilities, CapabilitySet::empty())
        .expect("capabilities overlap");
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 258);
    state.create_fence(client, id(259)).expect("fence object");
    state
        .dispatch(
            client,
            ClientRequest::Attach {
                surface: id(257),
                buffer: id(258),
                offset: Point::default(),
                acquire_fence: Some(id(259)),
            },
        )
        .expect("fenced attach");
    state
        .dispatch(client, ClientRequest::Commit { surface: id(257) })
        .expect("commit");
    let surface = SurfaceKey {
        connection: client,
        object_id: id(257),
    };

    assert_eq!(state.commit_ready(surface), Ok(false));
    assert_eq!(
        state.present(surface, 10),
        Err(StateError::InvalidState { object_id: id(257) })
    );
    state.signal_fence(client, id(259)).expect("signal fence");
    assert_eq!(state.commit_ready(surface), Ok(true));
    assert!(state.present(surface, 11).is_ok());
}

#[test]
fn second_consecutive_deadline_miss_emits_frame_late_once() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);
    let surface = SurfaceKey {
        connection: client,
        object_id: id(257),
    };
    state.take_events();
    state.record_deadline_miss(surface).expect("first miss");
    assert!(state.take_events().is_empty());
    state.record_deadline_miss(surface).expect("second miss");
    assert_eq!(state.take_events()[0].kind, EventKind::FrameLate);
    state.record_deadline_miss(surface).expect("third miss");
    assert!(state.take_events().is_empty());
}

#[test]
fn capability_revocation_event_precedes_reverse_order_object_destruction() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 257);
    create_surface(&mut state, client, 258);
    state.take_events();

    let destroyed = state
        .revoke_capability(client, Capability::SurfaceCreate)
        .expect("revocation");
    assert_eq!(
        destroyed,
        vec![
            (id(258), ObjectKind::Surface),
            (id(257), ObjectKind::Surface)
        ]
    );
    let events = state.take_events();
    assert_eq!(
        events[0].kind,
        EventKind::CapabilityRevoked(Capability::SurfaceCreate)
    );
    assert_eq!(
        events[1].kind,
        EventKind::ObjectDestroyed(ObjectKind::Surface)
    );
}

#[test]
fn the_null_id_unknown_opcodes_and_wrong_types_are_typed_protocol_errors() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    // Zero names nothing, so it is the one identifier a client may never allocate. Everything
    // above the display is the client's to choose.
    let null = state.dispatch(client, ClientRequest::CreateSurface { new_id: id(0) });
    assert_eq!(null, Err(StateError::InvalidObjectId { object_id: id(0) }));
    assert!(!state.is_connected(client));

    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    // One is the display and is not the client's to allocate, so it is refused on the range
    // rather than as a reuse — the client never owned it to reuse.
    let display = state.dispatch(
        client,
        ClientRequest::CreateSurface {
            new_id: ObjectId::DISPLAY,
        },
    );
    assert_eq!(
        display,
        Err(StateError::InvalidObjectId {
            object_id: ObjectId::DISPLAY
        })
    );
    assert!(!state.is_connected(client));

    let client = connect(&mut state);
    let unknown = state.dispatch(
        client,
        ClientRequest::Unknown {
            object: ObjectId::DISPLAY,
            opcode: Opcode::from_raw(0x55),
        },
    );
    assert!(matches!(unknown, Err(StateError::UnknownOpcode { .. })));
    assert!(!state.is_connected(client));

    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    let wrong_type = state.dispatch(client, ClientRequest::Commit { surface: id(256) });
    assert!(matches!(
        wrong_type,
        Err(StateError::WrongObjectType { .. })
    ));
    assert!(!state.is_connected(client));
}

#[test]
fn arbitrary_decoded_requests_return_errors_without_panicking() {
    for seed in 0_u32..512 {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut state = state_with_limits(ConnectionLimits {
                max_objects: 8,
                max_surfaces: 2,
                max_buffers: 2,
                max_imported_handles: 2,
                max_damage_rects_per_commit: 2,
                max_pending_events: 8,
            });
            let client = connect(&mut state);
            let object = id(seed);
            let requests = [
                ClientRequest::Destroy { object },
                ClientRequest::Commit { surface: object },
                ClientRequest::Damage {
                    surface: object,
                    rect: Rect {
                        x: seed as i32,
                        y: -(seed as i32),
                        width: seed,
                        height: seed,
                    },
                },
                ClientRequest::SetRole {
                    surface: object,
                    role: SurfaceRole(seed),
                },
                ClientRequest::Unknown {
                    object,
                    opcode: Opcode::from_raw(seed as u16),
                },
            ];
            for request in requests {
                let _ = state.dispatch(client, request);
            }
        }));
        assert!(result.is_ok(), "state machine panicked for seed {seed}");
    }
}

/// A connection table that grows without a bound is a client-reachable allocation, and the
/// transport makes it reachable by anything that can open a socket. Refusing past the limit is
/// what keeps the compositor's failure mode inside the compositor.
#[test]
fn connections_past_the_server_limit_are_refused_rather_than_accepted() {
    let limit = 3;
    let mut state = CompositorState::new(
        software_capabilities(),
        ServerLimits {
            max_connections: limit,
        },
        ConnectionLimits::default(),
    );

    let accepted: Vec<ConnectionId> = (0..limit).map(|_| connect(&mut state)).collect();
    assert_eq!(accepted.len(), limit);

    assert_eq!(
        state.connect(software_capabilities(), CapabilitySet::empty()),
        Err(NegotiationError::ConnectionLimitReached { limit })
    );

    // Refusal is not permanent: closing one makes room for exactly one more, so a compositor
    // that has been at its limit is not a compositor that has stopped accepting clients.
    state.close_connection(accepted[0]);
    let replacement = connect(&mut state);
    assert!(state.is_connected(replacement));
    assert_eq!(
        state.connect(software_capabilities(), CapabilitySet::empty()),
        Err(NegotiationError::ConnectionLimitReached { limit })
    );
}

/// A shared outgoing queue is memory every other connection lives beside, so one client that
/// stops reading must not be able to grow it. Reaching the bound is not a protocol violation —
/// the client sent nothing wrong — so it produces a resource error and a close.
#[test]
fn a_connection_that_stops_reading_events_is_disconnected_not_grown() {
    let limit = 4;
    let mut state = state_with_limits(ConnectionLimits {
        max_pending_events: limit,
        ..ConnectionLimits::default()
    });
    let client = connect(&mut state);
    create_session(&mut state, client, 256);

    // Revoking a capability no object holds emits exactly one event per call and destroys
    // nothing, so the only bound this exercises is the one under test.
    for _ in 0..limit {
        let _ = state.revoke_capability(client, Capability::Screencopy);
    }
    assert!(
        state.is_connected(client),
        "the bound itself is not an error"
    );

    let _ = state.revoke_capability(client, Capability::Screencopy);
    assert!(
        !state.is_connected(client),
        "a connection past its event bound is closed"
    );

    let events = state.take_events();
    let overflow = events
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            EventKind::Error(error) => Some(error),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the client is told why it was disconnected"));
    assert_eq!(overflow, StateError::EventQueueOverflow { queued: limit });
    assert_eq!(overflow.category(), ErrorCategory::Resource);
}

/// Closing a connection used to queue one destroy event per object it held — up to
/// `max_objects` of them, addressed to a connection that had just been removed and so could
/// never receive them. `Teardown` already carries that list to the caller.
#[test]
fn closing_a_connection_reports_its_objects_without_queuing_events_for_it() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    state
        .dispatch(client, ClientRequest::CreateSurface { new_id: id(257) })
        .unwrap_or_else(|error| panic!("surface creation must succeed: {error:?}"));
    let _ = state.take_events();

    let teardown = state.close_connection(client);
    assert!(
        teardown
            .destroyed
            .iter()
            .any(|(id, _)| id.into_raw() == 257),
        "the caller is told what the connection held"
    );
    assert!(
        state.take_events().is_empty(),
        "nothing is queued for a connection that has been removed"
    );
}

#[test]
fn a_committed_surface_is_in_the_scene_with_no_shell_attached() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 300);

    assert!(
        state.stacking_order().is_empty(),
        "a surface with no content is not in the scene"
    );

    attach_and_commit(&mut state, client, 257, 300);

    let key = SurfaceKey {
        connection: client,
        object_id: id(257),
    };
    assert_eq!(
        state.stacking_order(),
        [key],
        "content is what puts a surface in the scene, and nothing else is attached to do it"
    );
    assert_eq!(
        state.surface_position(key),
        Some(Point::default()),
        "a surface starts at the origin until something places it"
    );
}

#[test]
fn placing_a_surface_survives_a_later_commit() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 300);
    attach_and_commit(&mut state, client, 257, 300);

    let key = SurfaceKey {
        connection: client,
        object_id: id(257),
    };
    state
        .place_surface(key, Point { x: 40, y: 25 })
        .unwrap_or_else(|error| panic!("a placement must succeed: {error:?}"));

    state
        .release_buffer(client, id(300))
        .unwrap_or_else(|error| panic!("a release must succeed: {error:?}"));
    attach_and_commit(&mut state, client, 257, 300);

    assert_eq!(
        state.surface_position(key),
        Some(Point { x: 40, y: 25 }),
        "drawing again does not move a window back to the origin"
    );
}

#[test]
fn a_commit_without_an_attach_keeps_the_content_already_shown() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 300);
    attach_and_commit(&mut state, client, 257, 300);

    let first = state
        .surface_snapshot(client, id(257))
        .expect("content after the first commit")
        .commit;

    state
        .dispatch(
            client,
            ClientRequest::Damage {
                surface: id(257),
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 4,
                },
            },
        )
        .unwrap_or_else(|error| panic!("damage must stage: {error:?}"));
    state
        .dispatch(client, ClientRequest::Commit { surface: id(257) })
        .unwrap_or_else(|error| panic!("a state-only commit must succeed: {error:?}"));

    let second = state
        .surface_snapshot(client, id(257))
        .expect("content survives a commit that attached nothing");
    assert_eq!(
        second.buffer,
        id(300),
        "the surface still shows the buffer it was given"
    );
    assert!(
        second.commit > first,
        "and the commit that carried it forward is a new one"
    );
}

#[test]
fn attaching_nothing_takes_the_surface_out_of_the_scene_and_returns_its_buffer() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 300);
    attach_and_commit(&mut state, client, 257, 300);
    assert_eq!(state.stacking_order().len(), 1);
    let _ = state.take_events();

    state
        .dispatch(
            client,
            ClientRequest::Attach {
                surface: id(257),
                buffer: ObjectId::NULL,
                offset: Point::default(),
                acquire_fence: None,
            },
        )
        .unwrap_or_else(|error| panic!("a null attach must be accepted: {error:?}"));
    state
        .dispatch(client, ClientRequest::Commit { surface: id(257) })
        .unwrap_or_else(|error| panic!("the detaching commit must succeed: {error:?}"));

    assert!(
        state.surface_snapshot(client, id(257)).is_none(),
        "the surface shows nothing once it is detached"
    );
    assert!(
        state.stacking_order().is_empty(),
        "and a surface showing nothing is not in the scene"
    );
    assert_eq!(
        state.buffer_state(client, id(300)),
        Some(BufferState::Available),
        "the buffer goes back to the client rather than being held forever"
    );
    let told: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.object_id == id(300))
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        told,
        vec![EventKind::BufferRelease],
        "and the client is told, because a buffer it does not know is free is a buffer it cannot reuse"
    );
}

#[test]
fn a_detached_surface_returns_to_the_place_it_was_given() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 300);
    attach_and_commit(&mut state, client, 257, 300);

    let key = SurfaceKey {
        connection: client,
        object_id: id(257),
    };
    state
        .place_surface(key, Point { x: 60, y: 12 })
        .unwrap_or_else(|error| panic!("a placement must succeed: {error:?}"));

    state
        .dispatch(
            client,
            ClientRequest::Attach {
                surface: id(257),
                buffer: ObjectId::NULL,
                offset: Point::default(),
                acquire_fence: None,
            },
        )
        .expect("null attach");
    state
        .dispatch(client, ClientRequest::Commit { surface: id(257) })
        .expect("detaching commit");
    attach_and_commit(&mut state, client, 257, 300);

    assert_eq!(
        state.surface_position(key),
        Some(Point { x: 60, y: 12 }),
        "a window that stopped drawing and started again is the same window"
    );
}

#[test]
fn the_scene_never_names_anything_the_compositor_has_let_go() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);
    import_buffer(&mut state, client, 300);
    attach_and_commit(&mut state, client, 257, 300);
    assert_eq!(state.scene_faults(), Ok(()));

    // Every way a surface can leave, and after each the scene must agree with what is left.
    state
        .dispatch(client, ClientRequest::Destroy { object: id(257) })
        .unwrap_or_else(|error| panic!("a surface is destroyed: {error:?}"));
    assert_eq!(
        state.scene_faults(),
        Ok(()),
        "destroying a surface takes it out of the scene"
    );

    create_surface(&mut state, client, 258);
    attach_and_commit(&mut state, client, 258, 300);
    assert_eq!(state.scene_faults(), Ok(()));

    state.close_connection(client);
    assert_eq!(
        state.scene_faults(),
        Ok(()),
        "a connection going takes its surfaces out of the scene"
    );
    assert!(state.stacking_order().is_empty());
}

/// A shell may place a window before its client has drawn anything.
///
/// A shell hears about a window when it takes its role, which is before the first commit. If
/// placing put the surface in the scene, the very next composition would be asked to draw a
/// window with no pixels — and the shell would have to know to wait, which is the compositor's
/// business and not its.
#[test]
fn placing_a_window_before_it_draws_leaves_the_scene_consistent() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 9);
    create_surface(&mut state, client, 257);

    let key = SurfaceKey {
        connection: client,
        object_id: id(257),
    };
    state
        .place_surface(key, Point { x: 40, y: 25 })
        .unwrap_or_else(|error| panic!("a placement is accepted: {error:?}"));
    assert!(
        state.stacking_order().is_empty(),
        "a window with nothing to show is not in the scene, however early it was placed"
    );
    assert_eq!(
        state.scene_faults(),
        Ok(()),
        "and the scene does not disagree with itself in the meantime"
    );

    state
        .raise_surface(key)
        .unwrap_or_else(|error| panic!("a raise is accepted too: {error:?}"));
    assert!(
        state.stacking_order().is_empty(),
        "and neither does raising it, for the same reason"
    );
    assert_eq!(state.scene_faults(), Ok(()));

    import_buffer(&mut state, client, 300);
    attach_and_commit(&mut state, client, 257, 300);
    assert_eq!(
        state.stacking_order(),
        [key],
        "drawing is what puts it in the scene"
    );
    assert_eq!(
        state.surface_position(key),
        Some(Point { x: 40, y: 25 }),
        "and the place the shell chose was waiting for it"
    );
}
