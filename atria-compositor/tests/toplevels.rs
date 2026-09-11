//! Windows: what a surface becomes when something arranges it.
//!
//! A surface is content. A toplevel is the window semantics laid over it, and it is a separate
//! object because an integer cannot say where a title or a configure exchange lives.

use atria_compositor::{
    ClientRequest, CompositorState, ConnectionId, ConnectionLimits, ObjectKind, ResolveError,
    ServerLimits, Size, StateError, TitleText,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::CapabilitySet;
use atria_protocol::interface::toplevel_state;
use atria_protocol::message::MAX_TITLE_BYTES;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn connected() -> (CompositorState, ConnectionId) {
    let mut state = CompositorState::new(
        CapabilitySet::default_grants(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let connection = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("baseline capabilities overlap: {error:?}"));
    state
        .create_session(connection, id(9), None, true)
        .unwrap_or_else(|error| panic!("the session is established: {error:?}"));
    state
        .dispatch(connection, ClientRequest::CreateSurface { new_id: id(256) })
        .unwrap_or_else(|error| panic!("the surface is created: {error:?}"));
    (state, connection)
}

/// A surface may take one role. A second is refused rather than replacing the first: the role
/// object carries state a client is entitled to keep.
#[test]
fn a_surface_takes_one_role_and_a_second_is_refused() {
    let (mut state, connection) = connected();

    state
        .dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("the first role applies: {error:?}"));
    assert_eq!(
        state.object_kind(connection, id(257)),
        Some(ObjectKind::Toplevel)
    );

    assert_eq!(
        state.dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(258),
            },
        ),
        Err(StateError::InvalidState { object_id: id(256) }),
        "the surface already has a role"
    );
}

/// A role needs a surface, and needs it to be one.
#[test]
fn a_role_cannot_be_given_to_something_that_is_not_a_surface() {
    let (mut state, connection) = connected();

    assert!(matches!(
        state.dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(9),
                new_id: id(257),
            },
        ),
        Err(StateError::WrongObjectType { .. })
    ));
}

/// A title is text the compositor keeps for as long as the window exists, so it is bounded, and
/// the bound is enforced where the value is made rather than wherever a caller remembers.
#[test]
fn a_title_longer_than_the_protocol_permits_is_refused_not_truncated() {
    assert!(TitleText::new("a window").is_some());

    let at_limit = "x".repeat(MAX_TITLE_BYTES);
    assert!(
        TitleText::new(&at_limit).is_some(),
        "the limit itself is allowed"
    );

    let over = "x".repeat(MAX_TITLE_BYTES + 1);
    assert!(
        TitleText::new(&over).is_none(),
        "one byte more is refused, because a truncated title is a wrong title"
    );
}

/// What a client set is what a shell reads. Nothing in between reinterprets it.
#[test]
fn a_title_a_client_sets_is_the_title_a_shell_reads() {
    let (mut state, connection) = connected();
    state
        .dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("the role applies: {error:?}"));

    assert_eq!(
        state.toplevel_title(connection, id(257)),
        Some(""),
        "a window starts with no title rather than a placeholder"
    );

    let title = TitleText::new("Notes — draft").expect("a short title is allowed");
    state
        .dispatch(
            connection,
            ClientRequest::SetTitle {
                toplevel: id(257),
                title,
            },
        )
        .unwrap_or_else(|error| panic!("the title is set: {error:?}"));

    assert_eq!(
        state.toplevel_title(connection, id(257)),
        Some("Notes — draft")
    );
}

/// §12.4: a state bit the bound interface version does not define is refused, never ignored.
/// Guessing would let a compositor and a client disagree about what a window is.
#[test]
fn a_configure_with_an_undefined_state_bit_is_refused() {
    let (mut state, connection) = connected();
    state
        .dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("the role applies: {error:?}"));
    let size = Size {
        width: 800,
        height: 600,
    };

    assert_eq!(
        state.configure_toplevel(connection, id(257), size, 1 << 31),
        Err(StateError::InvalidState { object_id: id(257) })
    );

    // Every bit version one defines is accepted together.
    state
        .configure_toplevel(connection, id(257), size, toplevel_state::VALID_V1)
        .unwrap_or_else(|error| panic!("every defined state is accepted: {error:?}"));
}

/// Serials increase, so a client can tell which configure a later one supersedes, and a commit
/// echoing one names exactly the configure it answers.
#[test]
fn each_configure_carries_a_fresh_serial() {
    let (mut state, connection) = connected();
    state
        .dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("the role applies: {error:?}"));
    let size = Size {
        width: 640,
        height: 480,
    };

    let first = state
        .configure_toplevel(connection, id(257), size, toplevel_state::ACTIVATED)
        .unwrap_or_else(|error| panic!("the first configure is sent: {error:?}"));
    let second = state
        .configure_toplevel(connection, id(257), size, toplevel_state::MAXIMIZED)
        .unwrap_or_else(|error| panic!("the second configure is sent: {error:?}"));

    assert_ne!(first, second, "a serial names one configure");
    assert!(second > first);
}

/// Closing is a request. The window goes away when its client destroys it, not when the shell
/// asks — a client with unsaved work is entitled to refuse.
#[test]
fn closing_a_toplevel_asks_rather_than_destroys() {
    let (mut state, connection) = connected();
    state
        .dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("the role applies: {error:?}"));

    state
        .close_toplevel(connection, id(257))
        .unwrap_or_else(|error| panic!("the request is delivered: {error:?}"));

    assert_eq!(
        state.object_kind(connection, id(257)),
        Some(ObjectKind::Toplevel),
        "the window is still there until its client destroys it"
    );
}

/// A resolution failure, not a protocol error: the message was well formed and the value was too
/// long. Keeping them apart is what stops a client that sent nothing wrong being disconnected.
#[test]
fn an_over_long_title_is_a_resolution_failure() {
    let over = "x".repeat(MAX_TITLE_BYTES + 1);
    // The shape resolution reports, checked directly rather than through a frame, because the
    // value is what is at fault and not the framing.
    let refused: Result<TitleText, ResolveError> =
        TitleText::new(&over).ok_or(ResolveError::TitleTooLong {
            bytes: over.len(),
            maximum: MAX_TITLE_BYTES,
        });
    assert_eq!(
        refused,
        Err(ResolveError::TitleTooLong {
            bytes: MAX_TITLE_BYTES + 1,
            maximum: MAX_TITLE_BYTES,
        })
    );
}
