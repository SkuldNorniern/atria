//! The authority a shell holds, and what a shell sees when it takes over from one that died.

use atria_compositor::{
    ClientRequest, CompositorState, ConnectionId, ConnectionLimits, Point, ServerLimits,
    ShellError, Size, TitleText, ToplevelHandle,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::interface::toplevel_state;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn server() -> CompositorState {
    CompositorState::new(
        CapabilitySet::default_grants().with(Capability::ShellControl),
        ServerLimits::default(),
        ConnectionLimits::default(),
    )
}

/// An ordinary application: a connection with no shell authority, holding one window.
fn application(state: &mut CompositorState, title: &str) -> (ConnectionId, ToplevelHandle) {
    let connection = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("an application connects: {error:?}"));
    state
        .create_session(connection, id(9), None, true)
        .unwrap_or_else(|error| panic!("its session is established: {error:?}"));
    state
        .dispatch(connection, ClientRequest::CreateSurface { new_id: id(256) })
        .unwrap_or_else(|error| panic!("it creates a surface: {error:?}"));
    state
        .dispatch(
            connection,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("it takes a window role: {error:?}"));
    state
        .dispatch(
            connection,
            ClientRequest::SetTitle {
                toplevel: id(257),
                title: TitleText::new(title)
                    .unwrap_or_else(|| panic!("the test title is short enough")),
            },
        )
        .unwrap_or_else(|error| panic!("it sets a title: {error:?}"));
    let handle = state
        .toplevel_handle(connection, id(257))
        .unwrap_or_else(|| panic!("the window has a handle"));
    (connection, handle)
}

/// A shell: a connection that was *granted* the authority to arrange windows.
///
/// Two steps, and deliberately so. Connecting does not confer authority however the client asks;
/// something already holding the right to delegate it has to hand it over.
fn shell(state: &mut CompositorState) -> ConnectionId {
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::ShellControl),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("a shell connects: {error:?}"));
    state
        .grant_capability(connection, Capability::ShellControl)
        .unwrap_or_else(|error| panic!("the authority is granted: {error:?}"));
    connection
}

/// Connecting is not how authority is obtained, however the client asks at the handshake.
///
/// Negotiation settles what the backend can do; a grant settles what this program may do. Letting
/// the handshake confer the second would make shell control something any client could request.
#[test]
fn connecting_does_not_confer_the_authority_to_arrange_windows() {
    let mut state = server();
    let (_app, handle) = application(&mut state, "Notes");

    let asked_nicely = state
        .connect(
            CapabilitySet::default_grants().with(Capability::ShellControl),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("the connection is accepted: {error:?}"));

    assert_eq!(
        state.shell_raise(asked_nicely, handle),
        Err(ShellError::NotGranted),
        "asking for it at the handshake does not confer it"
    );

    state
        .grant_capability(asked_nicely, Capability::ShellControl)
        .unwrap_or_else(|error| panic!("it can be granted: {error:?}"));
    state
        .shell_raise(asked_nicely, handle)
        .unwrap_or_else(|error| panic!("and then it works: {error:?}"));
}

/// Authority is a grant, and a test can hold it back and watch exactly that come out.
#[test]
fn arranging_windows_requires_the_grant_to_arrange_windows() {
    let mut state = server();
    let (_app, handle) = application(&mut state, "Notes");

    // An ordinary application, which happens to know a handle, may not use it.
    let impostor = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("it connects: {error:?}"));
    assert_eq!(
        state.shell_raise(impostor, handle),
        Err(ShellError::NotGranted)
    );
    assert_eq!(
        state.shell_focus(impostor, handle),
        Err(ShellError::NotGranted)
    );
    assert_eq!(
        state.shell_close(impostor, handle),
        Err(ShellError::NotGranted)
    );
    assert_eq!(
        state.shell_place(impostor, handle, Point { x: 0, y: 0 }),
        Err(ShellError::NotGranted)
    );

    // The same operations succeed for a connection that holds the grant.
    let elysium = shell(&mut state);
    state
        .shell_place(elysium, handle, Point { x: 40, y: 20 })
        .unwrap_or_else(|error| panic!("a shell may place: {error:?}"));
    state
        .shell_raise(elysium, handle)
        .unwrap_or_else(|error| panic!("a shell may raise: {error:?}"));
}

/// A program without the grant learns nothing about which windows exist by trying handles: the
/// authority is checked before the handle is looked up, so both answers are the same.
#[test]
fn a_program_without_the_grant_cannot_probe_for_windows() {
    let mut state = server();
    let (_app, real) = application(&mut state, "Notes");
    let invented = ToplevelHandle(9_999);

    let impostor = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("it connects: {error:?}"));

    assert_eq!(
        state.shell_raise(impostor, real),
        state.shell_raise(impostor, invented),
        "a real handle and an invented one are indistinguishable without the grant"
    );
}

/// The restart contract: a shell that takes over is handed every window that exists, by the same
/// handles, so it reconstructs rather than starting from nothing.
#[test]
fn a_replacement_shell_is_handed_every_window_the_dead_one_arranged() {
    let mut state = server();
    let (_app, handle) = application(&mut state, "Notes");
    let first = shell(&mut state);

    state
        .shell_configure(
            first,
            handle,
            Size {
                width: 800,
                height: 600,
            },
            toplevel_state::ACTIVATED,
        )
        .unwrap_or_else(|error| panic!("the shell configures the window: {error:?}"));

    let before = state.shell_toplevels();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].handle, handle);
    assert_eq!(before[0].title, "Notes");
    assert_eq!(
        before[0].configured,
        Some(Size {
            width: 800,
            height: 600
        })
    );

    // The shell dies. Nothing about the application changes.
    state.close_connection(first);

    let after = state.shell_toplevels();
    assert_eq!(
        after, before,
        "the window, its handle, its title and what it was asked to be all survive the shell"
    );

    // A replacement takes over and can arrange the same window by the same handle.
    let second = shell(&mut state);
    state
        .shell_raise(second, handle)
        .unwrap_or_else(|error| panic!("the replacement arranges it: {error:?}"));
}

/// A handle is retired when its window goes, and never reissued — a shell's request already in
/// flight must not land on a different window than the one it named.
#[test]
fn a_handle_is_retired_with_its_window_and_never_reissued() {
    let mut state = server();
    let (app, first) = application(&mut state, "Notes");
    let elysium = shell(&mut state);

    state
        .dispatch(app, ClientRequest::Destroy { object: id(257) })
        .unwrap_or_else(|error| panic!("the application closes its window: {error:?}"));

    assert_eq!(
        state.shell_raise(elysium, first),
        Err(ShellError::UnknownHandle { handle: first }),
        "the shell is told the window is gone"
    );
    assert!(state.shell_toplevels().is_empty());

    // A second window gets a different handle, so a request naming the first cannot reach it.
    state
        .dispatch(app, ClientRequest::CreateSurface { new_id: id(300) })
        .unwrap_or_else(|error| panic!("it creates another surface: {error:?}"));
    state
        .dispatch(
            app,
            ClientRequest::GetToplevel {
                surface: id(300),
                new_id: id(301),
            },
        )
        .unwrap_or_else(|error| panic!("and another window: {error:?}"));
    let second = state
        .toplevel_handle(app, id(301))
        .unwrap_or_else(|| panic!("it has a handle"));

    assert_ne!(first, second, "a retired handle is never handed out again");
}

/// An application dying takes its windows out of the shell's view, without the shell noticing
/// anything about the application itself.
#[test]
fn an_application_dying_retires_the_windows_it_held() {
    let mut state = server();
    let (app, handle) = application(&mut state, "Notes");
    let elysium = shell(&mut state);
    assert_eq!(state.shell_toplevels().len(), 1);

    state.close_connection(app);

    assert!(state.shell_toplevels().is_empty());
    assert_eq!(
        state.shell_raise(elysium, handle),
        Err(ShellError::UnknownHandle { handle })
    );
}
