//! The authority a shell holds, and what a shell sees when it takes over from one that died.

use atria_compositor::{
    BufferDescriptor, BufferTransport, ClientRequest, CompositorState, ConnectionId,
    ConnectionLimits, EventKind, ObjectKind, Point, SeatId, ServerLimits, ShellError, Size,
    TitleText, ToplevelHandle,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::interface::Interface;
use atria_protocol::interface::toplevel_state;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn server() -> CompositorState {
    let mut state = CompositorState::new(
        CapabilitySet::default_grants()
            .with(Capability::ShellControl)
            .with(Capability::SoftwareShm),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    state
        .advertise_global(ObjectKind::ShellControl, 1)
        .unwrap_or_else(|| panic!("the shell authority is advertised"));
    state
}

/// An ordinary application: a connection with no shell authority, holding one window.
fn application(state: &mut CompositorState, title: &str) -> (ConnectionId, ToplevelHandle) {
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::SoftwareShm),
            CapabilitySet::empty(),
        )
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

/// Give an application's window real content, so it can hold focus.
///
/// Focus requires a mapped surface: input reaching a window that is drawing nothing would be
/// input going somewhere the user cannot see.
fn map_window(state: &mut CompositorState, connection: ConnectionId) {
    state
        .dispatch(
            connection,
            ClientRequest::ImportBuffer {
                new_id: id(300),
                descriptor: BufferDescriptor {
                    transport: BufferTransport::SoftwareShm,
                    size: Size {
                        width: 100,
                        height: 80,
                    },
                    stride: 400,
                    byte_len: 32_000,
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
}

/// A shell that has bound its authority, past its snapshot.
fn attached_shell(state: &mut CompositorState) -> ConnectionId {
    let connection = shell(state);
    state
        .dispatch(connection, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let control = state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name,
                interface: Interface::ShellControl,
                ..
            } => Some(name),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the authority is offered"));
    state
        .dispatch(
            connection,
            ClientRequest::Bind {
                name: control,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("the shell binds: {error:?}"));
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
        state.shell_focus(impostor, SeatId(1), handle),
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

/// A shell attaching to a running compositor is told the world, then told the telling is over.
///
/// This is the boundary that makes a replacement shell possible. Without it a shell would have to
/// enumerate what exists while changes to it were already arriving, and race the compositor for
/// its own starting picture.
#[test]
fn a_shell_binding_is_told_every_window_and_where_the_telling_ends() {
    let mut state = server();
    let (_, first) = application(&mut state, "ledger");
    let (_, second) = application(&mut state, "map");

    let shell = shell(&mut state);
    state
        .dispatch(shell, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let control = state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name,
                interface: Interface::ShellControl,
                ..
            } => Some(name),
            _ => None,
        })
        .expect("the shell authority is offered as a global");

    state
        .dispatch(
            shell,
            ClientRequest::Bind {
                name: control,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("the shell binds its authority: {error:?}"));

    let told: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.object_id == id(3))
        .map(|event| event.kind)
        .collect();

    assert_eq!(
        told,
        vec![
            EventKind::ShellToplevel {
                handle: first,
                title: String::from("ledger"),
            },
            EventKind::ShellToplevel {
                handle: second,
                title: String::from("map"),
            },
            EventKind::ShellFocusChanged {
                seat: SeatId(1),
                handle: ToplevelHandle(0),
            },
            EventKind::ShellSnapshotDone,
        ],
        "every window that already existed, then the line that says the snapshot is whole"
    );
}

/// A replacement shell is given the same handles the dead one held.
///
/// The applications never learn that their shell died. A handle is compositor-wide and never
/// reused, so an arrangement keyed to one survives the program that made it — which is the
/// difference between restarting the shell and restarting the session.
#[test]
fn a_replacement_shell_receives_the_handles_its_predecessor_held() {
    let mut state = server();
    let (_, window) = application(&mut state, "ledger");

    let first = shell(&mut state);
    state
        .shell_place(first, window, Point { x: 40, y: 25 })
        .unwrap_or_else(|error| panic!("the shell arranges it: {error:?}"));

    // The shell dies. Nothing about that is the application's business.
    state.close_connection(first);
    assert_eq!(
        state.shell_toplevels().len(),
        1,
        "the window outlives the program that was arranging it"
    );

    let replacement = shell(&mut state);
    state
        .dispatch(replacement, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let control = state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name,
                interface: Interface::ShellControl,
                ..
            } => Some(name),
            _ => None,
        })
        .expect("the authority is offered again");
    state
        .dispatch(
            replacement,
            ClientRequest::Bind {
                name: control,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("the replacement binds: {error:?}"));

    let handles: Vec<_> = state
        .take_events()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ShellToplevel { handle, .. } => Some(handle),
            _ => None,
        })
        .collect();
    assert_eq!(
        handles,
        vec![window],
        "the same window answers to the same handle it always did"
    );

    // And the replacement can arrange it, over the wire, by that handle.
    state
        .dispatch(
            replacement,
            ClientRequest::ShellPlace {
                control: id(3),
                handle: window,
                position: Point { x: 7, y: 9 },
            },
        )
        .unwrap_or_else(|error| panic!("the replacement arranges it: {error:?}"));
}

/// A program without the grant cannot arrange windows even holding a shell-control object.
#[test]
fn arranging_over_the_wire_still_requires_the_grant() {
    let mut state = server();
    let (_, window) = application(&mut state, "ledger");
    let ungranted = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("an ordinary program connects: {error:?}"));

    let refused = state.dispatch(
        ungranted,
        ClientRequest::ShellRaise {
            control: id(3),
            handle: window,
        },
    );
    assert!(
        refused.is_err(),
        "authority is checked on the request, not only when the object was bound"
    );
}

/// An ungranted program cannot even take hold of the authority object.
///
/// Checked at the bind as well as on every request. Refusing only the requests would leave an
/// ordinary client holding an object whose whole purpose is a power it does not have, and the
/// only thing standing between it and every window would be a check somewhere else.
#[test]
fn binding_the_shell_authority_requires_the_grant() {
    let mut state = server();
    let ungranted = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("an ordinary program connects: {error:?}"));

    state
        .dispatch(ungranted, ClientRequest::CreateRegistry { new_id: id(2) })
        .unwrap_or_else(|error| panic!("a registry: {error:?}"));
    let control = state
        .take_events()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::Global {
                name,
                interface: Interface::ShellControl,
                ..
            } => Some(name),
            _ => None,
        })
        .expect("the global is announced to everyone, because what exists is not a secret");

    let refused = state.dispatch(
        ungranted,
        ClientRequest::Bind {
            name: control,
            version: 1,
            new_id: id(3),
        },
    );
    assert!(
        refused.is_err(),
        "and taking hold of it is refused without the grant"
    );

    // A granted shell binds the same global without trouble.
    let shell = shell(&mut state);
    state
        .dispatch(shell, ClientRequest::CreateRegistry { new_id: id(2) })
        .expect("a registry");
    let _ = state.take_events();
    state
        .dispatch(
            shell,
            ClientRequest::Bind {
                name: control,
                version: 1,
                new_id: id(3),
            },
        )
        .unwrap_or_else(|error| panic!("a granted shell binds it: {error:?}"));
}

/// After the snapshot, the same events mean "this just happened".
///
/// A shell that only learned the world once would arrange a desktop that stopped matching it the
/// moment anything opened, closed or took focus.
#[test]
fn a_shell_is_told_about_windows_that_arrive_after_it_attached() {
    let mut state = server();
    let shell = attached_shell(&mut state);
    let _ = state.take_events();

    let (client, window) = application(&mut state, "ledger");
    map_window(&mut state, client);

    let told: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .map(|event| event.kind)
        .collect();
    assert!(
        told.iter().any(|kind| matches!(
            kind,
            EventKind::ShellToplevel { handle, title } if *handle == window && title == "ledger"
        )),
        "a window that opened after the shell attached reaches it, by name and handle"
    );

    // Focus moving is the shell's business too: it is what draws the active window differently.
    let surface = state
        .shell_toplevels()
        .into_iter()
        .find(|record| record.handle == window)
        .expect("the window");
    let _ = state.take_events();
    state
        .shell_focus(shell, SeatId(1), window)
        .unwrap_or_else(|error| panic!("the shell focuses it: {error:?}"));
    let focused: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .map(|event| event.kind)
        .collect();
    assert!(
        focused.iter().any(
            |kind| matches!(kind, EventKind::ShellFocusChanged { handle, .. } if *handle == window)
        ),
        "and the shell is told which window now holds focus"
    );

    // The application closes the window. The shell must stop drawing it.
    let _ = state.take_events();
    state
        .dispatch(
            client,
            ClientRequest::Destroy {
                object: surface.object_id,
            },
        )
        .unwrap_or_else(|error| panic!("the client closes its window: {error:?}"));
    let gone: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .map(|event| event.kind)
        .collect();
    assert!(
        gone.iter().any(
            |kind| matches!(kind, EventKind::ShellToplevelGone { handle } if *handle == window)
        ),
        "a window that has gone is one the shell is told about, not one it discovers"
    );
}

/// A window that never sets a title still reaches the shell.
///
/// Otherwise the only thing announcing a window would be the title it happens to set, and a
/// window that sets none would be invisible to the shell while being visible on the screen.
#[test]
fn a_window_with_no_title_is_still_announced() {
    let mut state = server();
    let shell = attached_shell(&mut state);
    let _ = state.take_events();

    let client = state
        .connect(
            CapabilitySet::default_grants().with(Capability::SoftwareShm),
            CapabilitySet::empty(),
        )
        .unwrap_or_else(|error| panic!("an application connects: {error:?}"));
    state
        .create_session(client, id(9), None, true)
        .unwrap_or_else(|error| panic!("its session: {error:?}"));
    state
        .dispatch(client, ClientRequest::CreateSurface { new_id: id(256) })
        .unwrap_or_else(|error| panic!("a surface: {error:?}"));
    state
        .dispatch(
            client,
            ClientRequest::GetToplevel {
                surface: id(256),
                new_id: id(257),
            },
        )
        .unwrap_or_else(|error| panic!("a window role: {error:?}"));

    let handle = state
        .toplevel_handle(client, id(257))
        .unwrap_or_else(|| panic!("the window has a handle"));
    let told: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .map(|event| event.kind)
        .collect();
    assert!(
        told.iter().any(|kind| matches!(
            kind,
            EventKind::ShellToplevel { handle: announced, title } if *announced == handle && title.is_empty()
        )),
        "taking a window role is what announces a window, not naming it"
    );
}

/// A window whose program died reaches the shell the same as one deliberately closed.
///
/// Crashing is the ordinary way a window disappears. A shell told only about deliberate
/// destruction would go on drawing windows whose programs are gone.
#[test]
fn a_shell_is_told_when_a_clients_death_takes_its_windows() {
    let mut state = server();
    let shell = attached_shell(&mut state);
    let (client, window) = application(&mut state, "ledger");
    let _ = state.take_events();

    state.close_connection(client);

    let told: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.connection == shell)
        .map(|event| event.kind)
        .collect();
    assert!(
        told.iter().any(
            |kind| matches!(kind, EventKind::ShellToplevelGone { handle } if *handle == window)
        ),
        "the shell learns the window is gone rather than discovering it"
    );
    assert!(
        state.shell_toplevels().is_empty(),
        "and the compositor no longer holds it either"
    );
}

/// A seat this compositor does not have is refused rather than quietly taken as the one it does.
#[test]
fn naming_a_seat_that_does_not_exist_is_refused() {
    let mut state = server();
    let (_, window) = application(&mut state, "ledger");
    let shell = shell(&mut state);

    assert_eq!(
        state.shell_focus(shell, SeatId(9), window),
        Err(ShellError::UnknownSeat { seat: SeatId(9) }),
        "with one seat, a request naming another is about something that is not there"
    );
    assert_ne!(
        state.shell_focus(shell, SeatId(1), window),
        Err(ShellError::UnknownSeat { seat: SeatId(1) }),
        "and the seat that does exist is not refused for its name"
    );
}
