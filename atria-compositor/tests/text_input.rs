//! Turning keys into text, and the one place that must not happen.
//!
//! A key is a position. What it becomes depends on a layout, a dead-key sequence, or — for
//! Korean and Japanese — a conversation with an input method. That conversation is the whole
//! reason this layer exists: there is no useful correspondence between the keys pressed and the
//! characters produced, so the field is sent what they became rather than what they were.

use atria_compositor::{
    BufferDescriptor, BufferTransport, ClientRequest, CompositorState, ConnectionId,
    ConnectionLimits, EventKind, ObjectKind, Point, ServerLimits, Size, SurfaceKey, TextBuffer,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::interface::Interface;
use atria_protocol::key::{PhysicalKey, usage};
use atria_protocol::message::TextPurpose;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn server() -> CompositorState {
    let mut state = CompositorState::new(
        CapabilitySet::default_grants()
            .with(Capability::SoftwareShm)
            .with(Capability::InputMethod),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    for kind in [
        ObjectKind::TextInput,
        ObjectKind::InputMethod,
        ObjectKind::Seat,
    ] {
        state
            .advertise_global(kind, 1)
            .unwrap_or_else(|| panic!("{kind:?} is advertised"));
    }
    state
}

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

fn bind(state: &mut CompositorState, connection: ConnectionId, name: u32, object: u32) {
    state
        .dispatch(
            connection,
            ClientRequest::Bind {
                name,
                version: 1,
                new_id: id(object),
            },
        )
        .unwrap_or_else(|error| panic!("binding {object} must succeed: {error:?}"));
}

/// An application with a mapped window and a text field, focused.
fn application(state: &mut CompositorState, purpose: TextPurpose) -> ConnectionId {
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::SoftwareShm),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("an application connects: {error:?}"));
    state
        .create_session(connection, id(9), None, true)
        .unwrap_or_else(|error| panic!("its session: {error:?}"));
    state
        .dispatch(connection, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let text = global(state, connection, Interface::TextInput);
    bind(state, connection, text, 3);

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
    state
        .set_keyboard_focus(Some(SurfaceKey {
            connection,
            object_id: id(256),
        }))
        .unwrap_or_else(|error| panic!("focus: {error:?}"));

    state
        .dispatch(
            connection,
            ClientRequest::EnableText {
                text_input: id(3),
                purpose,
            },
        )
        .unwrap_or_else(|error| panic!("the field wants text: {error:?}"));
    connection
}

/// The input method, with its object bound at 3.
fn method(state: &mut CompositorState) -> ConnectionId {
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::InputMethod),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("a method connects: {error:?}"));
    state
        .grant_capability(connection, Capability::InputMethod)
        .unwrap_or_else(|error| panic!("the grant: {error:?}"));
    state
        .dispatch(connection, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let name = global(state, connection, Interface::InputMethod);
    bind(state, connection, name, 3);
    connection
}

/// Everything queued, kept together.
///
/// Taken once and filtered, never taken per connection: `take_events` drains the whole queue, so
/// asking for one connection's events twice throws away everyone else's the first time.
fn drain(state: &mut CompositorState) -> Vec<(ConnectionId, EventKind)> {
    state
        .take_events()
        .into_iter()
        .map(|event| (event.connection, event.kind))
        .collect()
}

fn kinds(events: &[(ConnectionId, EventKind)], connection: ConnectionId) -> Vec<EventKind> {
    events
        .iter()
        .filter(|(owner, _)| *owner == connection)
        .map(|(_, kind)| kind.clone())
        .collect()
}

#[test]
fn being_the_input_method_requires_the_grant_to_be_it() {
    let mut state = server();
    let ungranted = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("an ordinary program connects: {error:?}"));
    state
        .dispatch(ungranted, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let name = global(&mut state, ungranted, Interface::InputMethod);

    assert!(
        state
            .dispatch(
                ungranted,
                ClientRequest::Bind {
                    name,
                    version: 1,
                    new_id: id(3),
                },
            )
            .is_err(),
        "an input method sees every key of whatever it composes for"
    );
}

#[test]
fn a_second_input_method_is_refused() {
    let mut state = server();
    let first = method(&mut state);
    let _ = first;
    let second = state
        .connect(
            CapabilitySet::default_grants().with(Capability::InputMethod),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("a second connects: {error:?}"));
    state
        .grant_capability(second, Capability::InputMethod)
        .unwrap_or_else(|error| panic!("the grant: {error:?}"));
    state
        .dispatch(second, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let name = global(&mut state, second, Interface::InputMethod);

    assert!(
        state
            .dispatch(
                second,
                ClientRequest::Bind {
                    name,
                    version: 1,
                    new_id: id(3),
                },
            )
            .is_err(),
        "two methods would both see every key, which is the reach this capability keeps rare"
    );
}

#[test]
fn a_focused_field_is_composed_into_and_both_sides_are_told() {
    let mut state = server();
    let ime = method(&mut state);
    let app = application(&mut state, TextPurpose::Normal);

    let events = drain(&mut state);
    assert!(
        kinds(&events, app)
            .iter()
            .any(|kind| matches!(kind, EventKind::TextEnter { .. })),
        "the field is told it now has the seat's text"
    );
    assert!(
        kinds(&events, ime)
            .iter()
            .any(|kind| matches!(kind, EventKind::MethodActivated { .. })),
        "and the method is told which field it is composing into"
    );
}

/// The point of the whole layer: keys go to the method, text comes back.
#[test]
fn keys_reach_the_method_and_the_field_is_sent_what_they_became() {
    let mut state = server();
    let ime = method(&mut state);
    let app = application(&mut state, TextPurpose::Normal);
    let _ = state.take_events();

    state.key(PhysicalKey::from_usage(usage::A), true, 1_000);

    let events = drain(&mut state);
    assert!(
        kinds(&events, ime)
            .iter()
            .any(|kind| matches!(kind, EventKind::Key { .. })),
        "the method gets the key"
    );
    assert!(
        kinds(&events, app).is_empty(),
        "and the field does not, because the key is not yet text"
    );

    // The method decides what those keys became, in two stages.
    let composing = TextBuffer::new("한").unwrap_or_else(|| panic!("the text fits"));
    state
        .dispatch(
            ime,
            ClientRequest::SetPreedit {
                method: id(3),
                text: composing.clone(),
                cursor_begin: 0,
                cursor_end: 3,
            },
        )
        .unwrap_or_else(|error| panic!("a preedit: {error:?}"));
    state
        .dispatch(
            ime,
            ClientRequest::CommitText {
                method: id(3),
                text: composing.clone(),
            },
        )
        .unwrap_or_else(|error| panic!("a commit: {error:?}"));

    let events = drain(&mut state);
    let told = kinds(&events, app);
    assert!(
        told.iter().any(|kind| matches!(
            kind,
            EventKind::TextPreedit { text, .. } if text.as_str() == "한"
        )),
        "the field is shown what is being composed"
    );
    assert!(
        told.iter().any(|kind| matches!(
            kind,
            EventKind::TextCommit { text } if text.as_str() == "한"
        )),
        "and then told it is text"
    );
}

/// The one place composition must not happen.
#[test]
fn a_password_field_is_never_composed_into() {
    let mut state = server();
    let ime = method(&mut state);
    let app = application(&mut state, TextPurpose::Password);

    let events = drain(&mut state);
    assert!(
        !kinds(&events, app)
            .iter()
            .any(|kind| matches!(kind, EventKind::TextEnter { .. })),
        "the field is not told it is being composed into, because it is not"
    );
    assert!(
        !kinds(&events, ime)
            .iter()
            .any(|kind| matches!(kind, EventKind::MethodActivated { .. })),
        "an input method sees every key of what it composes for, and a password is not that"
    );

    let _ = state.take_events();
    state.key(PhysicalKey::from_usage(usage::A), true, 1_000);
    let events = drain(&mut state);
    assert!(
        !kinds(&events, ime)
            .iter()
            .any(|kind| matches!(kind, EventKind::Key { .. })),
        "so the keys go to the field, not through the method"
    );
}

#[test]
fn focus_leaving_stops_the_composition() {
    let mut state = server();
    let ime = method(&mut state);
    let app = application(&mut state, TextPurpose::Normal);
    let _ = state.take_events();

    state
        .set_keyboard_focus(None)
        .unwrap_or_else(|error| panic!("focus leaves: {error:?}"));

    let events = drain(&mut state);
    assert!(
        kinds(&events, app)
            .iter()
            .any(|kind| matches!(kind, EventKind::TextLeave { .. })),
        "the field is told it no longer has the text"
    );
    assert!(
        kinds(&events, ime)
            .iter()
            .any(|kind| matches!(kind, EventKind::MethodDeactivated)),
        "and the method is told to stop"
    );
}

#[test]
fn text_longer_than_the_protocol_carries_is_refused() {
    assert!(
        TextBuffer::new(&"a".repeat(4097)).is_none(),
        "an input method commits a phrase, not a document"
    );
    assert!(TextBuffer::new(&"a".repeat(4096)).is_some());
}
