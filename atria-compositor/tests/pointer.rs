//! Where pointer events go, and what must not change where they go.

use atria_compositor::{
    BufferDescriptor, BufferTransport, ClientRequest, CompositorState, ConnectionId,
    ConnectionLimits, EventKind, InteractionKind, ObjectKind, Point, SeatId, ServerLimits, Size,
    SurfaceKey,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn server() -> CompositorState {
    let mut state = CompositorState::new(
        CapabilitySet::default_grants()
            .with(Capability::SoftwareShm)
            .with(Capability::ShellControl),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    state
        .advertise_global(ObjectKind::Seat, 1)
        .unwrap_or_else(|| panic!("a seat is advertised"));
    state
}

/// A client with one window of `side` pixels, placed at `at`, holding a pointer object.
fn window(state: &mut CompositorState, side: u32, at: Point) -> (ConnectionId, SurfaceKey) {
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::SoftwareShm),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("a client connects: {error:?}"));
    state
        .create_session(connection, id(9), None, true)
        .unwrap_or_else(|error| panic!("its session: {error:?}"));
    state
        .dispatch(connection, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let seat = state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name,
                interface: atria_protocol::interface::Interface::Seat,
                ..
            } => Some(name),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the seat is advertised"));
    state
        .dispatch(
            connection,
            ClientRequest::Bind {
                name: seat,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("the client binds the seat: {error:?}"));
    state
        .dispatch(
            connection,
            ClientRequest::GetPointer {
                seat: id(3),
                new_id: id(4),
            },
        )
        .unwrap_or_else(|error| panic!("it asks for a pointer: {error:?}"));

    state
        .dispatch(connection, ClientRequest::CreateSurface { new_id: id(256) })
        .unwrap_or_else(|error| panic!("a surface: {error:?}"));
    state
        .dispatch(
            connection,
            ClientRequest::ImportBuffer {
                new_id: id(300),
                descriptor: BufferDescriptor {
                    transport: BufferTransport::SoftwareShm,
                    size: Size {
                        width: side,
                        height: side,
                    },
                    stride: side * 4,
                    byte_len: u64::from(side) * u64::from(side) * 4,
                },
            },
        )
        .unwrap_or_else(|error| panic!("a buffer: {error:?}"));
    state
        .dispatch(
            connection,
            ClientRequest::Attach {
                surface: id(256),
                buffer: id(300),
                offset: Point::default(),
                acquire_fence: None,
            },
        )
        .unwrap_or_else(|error| panic!("an attach: {error:?}"));
    state
        .dispatch(connection, ClientRequest::Commit { surface: id(256) })
        .unwrap_or_else(|error| panic!("a commit: {error:?}"));

    let key = SurfaceKey {
        connection,
        object_id: id(256),
    };
    state
        .place_surface(key, at)
        .unwrap_or_else(|error| panic!("a placement: {error:?}"));
    (connection, key)
}

fn kinds_for(state: &mut CompositorState, connection: ConnectionId) -> Vec<EventKind> {
    state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == connection && event.object_id == id(4))
        .map(|event| event.kind)
        .collect()
}

#[test]
fn the_pointer_enters_what_it_is_over_and_leaves_what_it_was() {
    let mut state = server();
    let (client, _) = window(&mut state, 100, Point { x: 0, y: 0 });
    let _ = state.take_events();

    state.move_pointer(Point { x: 10, y: 20 }, 1_000);
    let told = kinds_for(&mut state, client);
    assert!(
        told.iter().any(|kind| matches!(
            kind,
            EventKind::PointerEnter { position, .. } if *position == Point { x: 10, y: 20 }
        )),
        "entering says where in the surface's own coordinates, not the screen's"
    );

    state.move_pointer(Point { x: 400, y: 400 }, 2_000);
    let told = kinds_for(&mut state, client);
    assert!(
        told.iter()
            .any(|kind| matches!(kind, EventKind::PointerLeave { .. })),
        "moving off it says so"
    );
}

/// The property everything else depends on: a press holds routing until the release.
///
/// A client dragging a scrollbar out of its own window must keep receiving the drag. Without this
/// the release never arrives and the client is left believing a button is still down.
#[test]
fn a_press_holds_routing_until_the_release() {
    let mut state = server();
    let (client, _) = window(&mut state, 100, Point { x: 0, y: 0 });
    state.move_pointer(Point { x: 50, y: 50 }, 1_000);
    let _ = state.take_events();

    state.pointer_button(1, true, 2_000);
    state.move_pointer(Point { x: 900, y: 900 }, 3_000);
    let told = kinds_for(&mut state, client);
    assert!(
        told.iter()
            .any(|kind| matches!(kind, EventKind::PointerMotion { .. })),
        "motion far outside the surface still reaches the client that was pressed"
    );
    assert!(
        !told
            .iter()
            .any(|kind| matches!(kind, EventKind::PointerLeave { .. })),
        "and it is not told it was left, because it was not"
    );

    state.pointer_button(1, false, 4_000);
    let told = kinds_for(&mut state, client);
    assert!(
        told.iter()
            .any(|kind| matches!(kind, EventKind::PointerButton { pressed: false, .. })),
        "the release reaches the client that took the press"
    );
    assert!(
        told.iter()
            .any(|kind| matches!(kind, EventKind::PointerLeave { .. })),
        "and only then is it told the pointer has gone elsewhere"
    );
}

#[test]
fn the_pointer_goes_to_the_topmost_window_under_it() {
    let mut state = server();
    let (below, _) = window(&mut state, 200, Point { x: 0, y: 0 });
    let (above, _) = window(&mut state, 100, Point { x: 0, y: 0 });
    let _ = state.take_events();

    state.move_pointer(Point { x: 50, y: 50 }, 1_000);
    assert!(
        kinds_for(&mut state, above)
            .iter()
            .any(|kind| matches!(kind, EventKind::PointerEnter { .. })),
        "the window on top receives it where they overlap"
    );
    assert!(
        kinds_for(&mut state, below).is_empty(),
        "and the one underneath hears nothing"
    );

    state.move_pointer(Point { x: 150, y: 150 }, 2_000);
    assert!(
        kinds_for(&mut state, below)
            .iter()
            .any(|kind| matches!(kind, EventKind::PointerEnter { .. })),
        "and it receives it where the upper window is not"
    );
}

/// A shell learns that a window was pressed, and nothing else about the input.
#[test]
fn a_press_tells_the_shell_which_window_without_telling_it_the_input() {
    let mut state = server();
    state
        .advertise_global(ObjectKind::ShellControl, 1)
        .unwrap_or_else(|| panic!("the authority is advertised"));
    state
        .advertise_global(ObjectKind::Shell, 1)
        .unwrap_or_else(|| panic!("the role factory is advertised"));

    let (client, surface) = window(&mut state, 100, Point { x: 0, y: 0 });
    state
        .dispatch(
            client,
            ClientRequest::GetToplevel {
                surface: surface.object_id,
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("a window role: {error:?}"));
    let handle = state
        .toplevel_handle(client, id(257))
        .unwrap_or_else(|| panic!("the window has a handle"));

    let shell = state
        .connect(
            CapabilitySet::default_grants().with(Capability::ShellControl),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("a shell connects: {error:?}"));
    state
        .grant_capability(shell, Capability::ShellControl)
        .unwrap_or_else(|error| panic!("the grant: {error:?}"));
    state
        .dispatch(shell, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let control = state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name,
                interface: atria_protocol::interface::Interface::ShellControl,
                ..
            } => Some(name),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the authority is offered"));
    state
        .dispatch(
            shell,
            ClientRequest::Bind {
                name: control,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("the shell binds it: {error:?}"));

    state.move_pointer(Point { x: 50, y: 50 }, 1_000);
    let _ = state.take_events();
    state.pointer_button(1, true, 2_000);

    let told: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .map(|event| event.kind)
        .collect();
    assert!(
        told.iter().any(|kind| matches!(
            kind,
            EventKind::ShellInteraction { handle: pressed, seat, kind: InteractionKind::PointerPress, .. }
                if *pressed == handle && *seat == SeatId(1)
        )),
        "the shell is told a window was pressed, which is what click-to-focus is built from"
    );
    assert!(
        !told.iter().any(|kind| matches!(
            kind,
            EventKind::PointerMotion { .. } | EventKind::PointerButton { .. }
        )),
        "and is told none of the input itself, which is the client's alone"
    );
}

/// Breaking routing continuity starts a new epoch, so stale events are recognisable.
#[test]
fn resetting_routing_starts_an_epoch_a_client_can_tell_apart() {
    let mut state = server();
    let (client, _) = window(&mut state, 100, Point { x: 0, y: 0 });
    state.move_pointer(Point { x: 50, y: 50 }, 1_000);
    state.pointer_button(1, true, 2_000);
    let before = state.pointer_epoch();
    let _ = state.take_events();

    state.reset_pointer();
    assert_eq!(
        state.pointer_epoch(),
        before + 1,
        "a break in routing is a new epoch, not a continuation of the old one"
    );
    let told = kinds_for(&mut state, client);
    assert!(
        told.iter()
            .any(|kind| matches!(kind, EventKind::PointerLeave { epoch, .. } if *epoch == before)),
        "the leave belongs to the world it ended"
    );

    // The grab did not survive, so a press now goes wherever the pointer actually is.
    state.move_pointer(Point { x: 900, y: 900 }, 3_000);
    let told = kinds_for(&mut state, client);
    assert!(
        told.is_empty(),
        "a grab cannot outlive the routing it belonged to"
    );
}
