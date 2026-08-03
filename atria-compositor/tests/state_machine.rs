use std::panic::{AssertUnwindSafe, catch_unwind};

use atria_compositor::{
    BufferDescriptor, BufferState, BufferTransport, ClientRequest, CompositorState, ConnectionId,
    ConnectionLimits, Damage, ErrorCode, EventKind, FocusEvent, ObjectKind, Point, Rect,
    SeatCapabilities, Size, StateError, SurfaceKey, SurfaceRole,
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
    CompositorState::new(software_capabilities(), limits)
}

fn connect(state: &mut CompositorState) -> ConnectionId {
    state
        .connect(software_capabilities(), CapabilitySet::empty())
        .expect("baseline capabilities overlap")
}

fn create_session(state: &mut CompositorState, connection: ConnectionId, raw: u32) {
    state
        .create_session(connection, id(raw), None, true)
        .expect("session creation must succeed");
}

fn create_surface(
    state: &mut CompositorState,
    connection: ConnectionId,
    session: u32,
    surface: u32,
) {
    state
        .dispatch(
            connection,
            ClientRequest::CreateSurface {
                session: id(session),
                new_id: id(surface),
            },
        )
        .expect("surface creation must succeed");
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
        .expect("buffer import must succeed");
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
        .expect("attach must succeed");
    state
        .dispatch(
            connection,
            ClientRequest::Commit {
                surface: id(surface),
            },
        )
        .expect("commit must succeed");
}

#[test]
fn client_cannot_reference_or_affect_another_clients_object() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client_a = connect(&mut state);
    let client_b = connect(&mut state);
    create_session(&mut state, client_b, 256);
    create_surface(&mut state, client_b, 256, 257);
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
        create_surface(&mut state, client, 256, 257);
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

#[test]
fn destroyed_id_cannot_be_reused_before_connection_teardown() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 256, 257);
    state
        .dispatch(client, ClientRequest::Destroy { object: id(257) })
        .expect("first destruction succeeds");

    let error = state.dispatch(
        client,
        ClientRequest::CreateSurface {
            session: id(256),
            new_id: id(257),
        },
    );
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
    create_surface(&mut state, client, 256, 257);

    let result = state.dispatch(
        client,
        ClientRequest::CreateSurface {
            session: id(256),
            new_id: id(258),
        },
    );
    assert_eq!(
        result,
        Err(StateError::QuotaExceeded {
            object_id: ObjectId::DISPLAY
        })
    );
    assert!(state.is_connected(client));
}

#[test]
fn commit_before_attach_is_an_object_error_and_destroys_only_surface() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 256, 257);

    let result = state.dispatch(client, ClientRequest::Commit { surface: id(257) });

    assert_eq!(result, Err(StateError::InvalidState { object_id: id(257) }));
    assert_eq!(ErrorCode::InvalidState.category(), ErrorCategory::Object);
    assert!(state.is_connected(client));
    assert!(state.surface_snapshot(client, id(257)).is_none());
    let second = state.dispatch(client, ClientRequest::Commit { surface: id(257) });
    assert!(matches!(second, Err(StateError::InvalidObject { .. })));
    assert!(
        !state.is_connected(client),
        "referencing the destroyed object is fatal"
    );
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
    create_surface(&mut state, client, 257, 258);
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
    create_surface(&mut state, client, 256, 257);
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
        create_surface(&mut state, client, 256, surface);
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
        create_surface(&mut state, client, 256, surface);
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
    let mut state = CompositorState::new(software_capabilities(), ConnectionLimits::default());
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
    let mut state = CompositorState::new(capabilities, ConnectionLimits::default());
    let client = state
        .connect(capabilities, CapabilitySet::empty())
        .expect("capabilities overlap");
    create_session(&mut state, client, 256);
    create_surface(&mut state, client, 256, 257);
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
    create_surface(&mut state, client, 256, 257);
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
    create_surface(&mut state, client, 256, 257);
    create_surface(&mut state, client, 256, 258);
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
fn reserved_ids_unknown_opcodes_and_wrong_types_are_typed_protocol_errors() {
    let mut state = state_with_limits(ConnectionLimits::default());
    let client = connect(&mut state);
    create_session(&mut state, client, 256);
    let reserved = state.dispatch(
        client,
        ClientRequest::CreateSurface {
            session: id(256),
            new_id: id(3),
        },
    );
    assert_eq!(
        reserved,
        Err(StateError::InvalidObjectId { object_id: id(3) })
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
