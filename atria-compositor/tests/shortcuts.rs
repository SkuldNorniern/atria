//! Claiming a chord, and what claiming one must not let you see.
//!
//! A shell needs to know that Super+Q happened. It does not need to watch what is typed into a
//! password field to discover that. These prove the difference is real: a holder is told about
//! the chord it named and about nothing else, and the application never sees a key that was
//! taken from it.

use atria_compositor::{
    BufferDescriptor, BufferTransport, Chord, ChordMatch, ClientRequest, CompositorState,
    ConnectionId, ConnectionLimits, EventKind, ObjectKind, Point, SeatId, ServerLimits, Size,
    SurfaceKey,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::interface::Interface;
use atria_protocol::key::{Modifiers, PhysicalKey, usage};

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn server() -> CompositorState {
    let mut state = CompositorState::new(
        CapabilitySet::default_grants()
            .with(Capability::SoftwareShm)
            .with(Capability::ShortcutControl),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    state
        .advertise_global(ObjectKind::Shortcuts, 1)
        .unwrap_or_else(|| panic!("the shortcut manager is advertised"));
    state
        .advertise_global(ObjectKind::Seat, 1)
        .unwrap_or_else(|| panic!("a seat is advertised"));
    state
}

/// Which global name an interface was advertised under.
fn global(state: &mut CompositorState, connection: ConnectionId, wanted: Interface) -> u32 {
    state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name, interface, ..
            } if event.connection == connection && interface == wanted => Some(name),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{wanted:?} is advertised"))
}

/// A holder that may claim chords, with its manager bound at object 3.
fn holder(state: &mut CompositorState) -> ConnectionId {
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::ShortcutControl),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("a holder connects: {error:?}"));
    state
        .grant_capability(connection, Capability::ShortcutControl)
        .unwrap_or_else(|error| panic!("the grant: {error:?}"));
    state
        .dispatch(connection, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let name = global(state, connection, Interface::Shortcuts);
    state
        .dispatch(
            connection,
            ClientRequest::Bind {
                name,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("the manager binds: {error:?}"));
    connection
}

fn super_q() -> Chord {
    Chord {
        trigger: PhysicalKey::from_usage(usage::Q),
        modifiers: Modifiers::META,
        mode: ChordMatch::Exact,
    }
}

fn claim(state: &mut CompositorState, connection: ConnectionId, shortcut: u32, chord: Chord) {
    state
        .dispatch(
            connection,
            ClientRequest::RegisterShortcut {
                manager: id(3),
                shortcut,
                seat: SeatId(1),
                chord,
            },
        )
        .unwrap_or_else(|error| panic!("the chord is claimed: {error:?}"));
}

#[test]
fn claiming_a_chord_requires_the_grant_to_claim_chords() {
    let mut state = server();
    let ungranted = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("an ordinary program connects: {error:?}"));
    state
        .dispatch(ungranted, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let name = global(&mut state, ungranted, Interface::Shortcuts);

    let refused = state.dispatch(
        ungranted,
        ClientRequest::Bind {
            name,
            version: 1,
            new_id: id(3),
        },
    );
    assert!(
        refused.is_err(),
        "what exists is not a secret, but holding it is what is gated"
    );
}

#[test]
fn a_chord_fires_for_the_holder_that_named_it() {
    let mut state = server();
    let shell = holder(&mut state);
    claim(&mut state, shell, 7, super_q());
    let _ = state.take_events();

    state.key(PhysicalKey::from_usage(usage::LEFT_META), true, 1_000);
    state.key(PhysicalKey::from_usage(usage::Q), true, 2_000);

    let fired: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .filter_map(|event| match event.kind {
            EventKind::ShortcutTriggered { shortcut, .. } => Some(shortcut),
            _ => None,
        })
        .collect();
    assert_eq!(
        fired,
        vec![7],
        "the holder is told by the name it gave, not by the key"
    );
}

#[test]
fn a_chord_that_was_not_claimed_does_not_fire() {
    let mut state = server();
    let shell = holder(&mut state);
    claim(&mut state, shell, 7, super_q());
    let _ = state.take_events();

    // The same key without the modifier, and the modifier with a different key.
    state.key(PhysicalKey::from_usage(usage::Q), true, 1_000);
    state.key(PhysicalKey::from_usage(usage::Q), false, 2_000);
    state.key(PhysicalKey::from_usage(usage::LEFT_META), true, 3_000);
    state.key(PhysicalKey::from_usage(usage::W), true, 4_000);

    assert!(
        !state
            .take_events()
            .into_iter()
            .any(|event| matches!(event.kind, EventKind::ShortcutTriggered { .. })),
        "a chord fires for what it named and nothing near it"
    );
}

#[test]
fn an_exact_chord_does_not_fire_under_an_extra_modifier() {
    let mut state = server();
    let shell = holder(&mut state);
    claim(&mut state, shell, 7, super_q());
    let _ = state.take_events();

    state.key(PhysicalKey::from_usage(usage::LEFT_META), true, 1_000);
    state.key(PhysicalKey::from_usage(usage::LEFT_SHIFT), true, 2_000);
    state.key(PhysicalKey::from_usage(usage::Q), true, 3_000);

    assert!(
        !state
            .take_events()
            .into_iter()
            .any(|event| matches!(event.kind, EventKind::ShortcutTriggered { .. })),
        "Super+Shift+Q is a different chord from Super+Q"
    );
}

#[test]
fn at_least_matching_survives_an_extra_modifier() {
    let mut state = server();
    let shell = holder(&mut state);
    claim(
        &mut state,
        shell,
        7,
        Chord {
            mode: ChordMatch::AtLeast,
            ..super_q()
        },
    );
    let _ = state.take_events();

    state.key(PhysicalKey::from_usage(usage::LEFT_META), true, 1_000);
    state.key(PhysicalKey::from_usage(usage::LEFT_SHIFT), true, 2_000);
    state.key(PhysicalKey::from_usage(usage::Q), true, 3_000);

    assert!(
        state
            .take_events()
            .into_iter()
            .any(|event| matches!(event.kind, EventKind::ShortcutTriggered { .. })),
        "at-least matching is what a chord that should survive an extra modifier asks for"
    );
}

#[test]
fn one_holder_per_chord_and_a_second_claim_is_refused() {
    let mut state = server();
    let first = holder(&mut state);
    claim(&mut state, first, 7, super_q());

    let second = holder(&mut state);
    let refused = state.dispatch(
        second,
        ClientRequest::RegisterShortcut {
            manager: id(3),
            shortcut: 9,
            seat: SeatId(1),
            chord: super_q(),
        },
    );
    assert!(
        refused.is_err(),
        "last-registration-wins would let anything take a chord from under the shell"
    );
}

#[test]
fn a_holder_going_releases_the_chords_it_claimed() {
    let mut state = server();
    let first = holder(&mut state);
    claim(&mut state, first, 7, super_q());

    state.close_connection(first);

    let second = holder(&mut state);
    state
        .dispatch(
            second,
            ClientRequest::RegisterShortcut {
                manager: id(3),
                shortcut: 9,
                seat: SeatId(1),
                chord: super_q(),
            },
        )
        .unwrap_or_else(|error| panic!("the chord is free again: {error:?}"));
}

/// The focused application never sees a key a shortcut took.
///
/// This is the whole point of matching before routing. A window that saw the Q of Super+Q would
/// act on a keystroke nobody meant for it — and a press hidden with its release delivered is not
/// a state any client can make sense of.
#[test]
fn the_application_never_sees_a_key_a_shortcut_took() {
    let mut state = server();
    let shell = holder(&mut state);
    claim(&mut state, shell, 7, super_q());

    let app = state
        .connect(
            CapabilitySet::default_grants().with(Capability::SoftwareShm),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("an application connects: {error:?}"));
    state
        .create_session(app, id(9), None, true)
        .unwrap_or_else(|error| panic!("its session: {error:?}"));
    state
        .dispatch(app, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let seat = global(&mut state, app, Interface::Seat);
    state
        .dispatch(
            app,
            ClientRequest::Bind {
                name: seat,
                version: 1,
                new_id: id(4),
            },
        )
        .unwrap_or_else(|error| panic!("the seat binds: {error:?}"));
    state
        .dispatch(
            app,
            ClientRequest::GetKeyboard {
                seat: id(4),
                new_id: id(5),
            },
        )
        .unwrap_or_else(|error| panic!("a keyboard: {error:?}"));
    let surface = mapped_surface(&mut state, app);
    state
        .set_keyboard_focus(Some(surface))
        .unwrap_or_else(|error| panic!("focus: {error:?}"));
    let _ = state.take_events();

    // The whole chord, pressed and released.
    let meta = PhysicalKey::from_usage(usage::LEFT_META);
    let q = PhysicalKey::from_usage(usage::Q);
    state.key(meta, true, 1_000);
    state.key(q, true, 2_000);
    state.key(q, false, 3_000);
    state.key(meta, false, 4_000);

    let delivered: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == app && event.object_id == id(5))
        .filter_map(|event| match event.kind {
            EventKind::Key { key, pressed, .. } => Some((key, pressed)),
            _ => None,
        })
        .collect();

    assert!(
        !delivered.iter().any(|(key, _)| *key == q),
        "neither the press nor the release of the trigger reaches it: {delivered:?}"
    );
    assert_eq!(
        delivered,
        vec![(meta, true), (meta, false)],
        "the modifier is ordinary input and still arrives, both ways"
    );
}

/// Give a connection a surface with content, so it can hold focus.
fn mapped_surface(state: &mut CompositorState, connection: ConnectionId) -> SurfaceKey {
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
                        width: 64,
                        height: 64,
                    },
                    stride: 256,
                    byte_len: 16_384,
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
    SurfaceKey {
        connection,
        object_id: id(256),
    }
}
