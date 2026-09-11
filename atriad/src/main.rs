//! The Atria display server.
//!
//! Listens on a Unix socket and serves each client that connects. One thread, one client at a
//! time: the presentation thread the plan calls for is a later split, and splitting before there
//! is a measured deadline to split around would be guessing.

use std::env::args;
use std::fs::remove_file;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::process::{ExitCode, exit};
use std::ptr::null_mut;

use libc::{
    AF_UNIX, POLLERR, POLLHUP, POLLIN, SOCK_CLOEXEC, SOCK_SEQPACKET, accept4, bind, c_char,
    listen as listen_on, nfds_t, poll, pollfd, sa_family_t, sockaddr, sockaddr_un,
    socket as make_socket, socklen_t,
};

use atria_compositor::{
    CompositorState, ConnectionId, ConnectionLimits, IdentitySource, ObjectKind, OutputIdentity,
    OutputInfo, Point, ServerLimits, Size, StateError,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::key::PhysicalKey;
use atria_software_output::PixelLayout;
use atria_transport::UnixTransport;
use atria_vnc::{Input, VncSink};
use atriad::{Presenter, Session, SessionError, compositor, present_all, software_capabilities};

/// The session the server establishes for each connection.
///
/// A session is not a global: a connection belongs to one, and does not choose it.
const SESSION_ID: u32 = 9;

/// The version each global is advertised at. One, because none of them has a second yet.
const VERSION: u32 = 1;

/// The output this server composes for, until a display backend reports a real one.
///
/// Described as a display rather than assumed as a size: clients bind it, learn its mode and
/// scale, and a shell can arrange against it. When a backend arrives it reports a different
/// topology through the same call and nothing above this changes.
const OUTPUT_WIDTH: u32 = 1280;
const OUTPUT_HEIGHT: u32 = 720;
const OUTPUT_REFRESH_MILLIHERTZ: u32 = 60_000;

/// Descriptors watched before the sessions: the two listeners and the viewer's wakeup.
const WATCHED_LISTENERS: usize = 3;

const OUTPUT_MILLIMETRES: Size = Size {
    width: 340,
    height: 190,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("atriad: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let path = args().nth(1).unwrap_or_else(|| {
        eprintln!("atriad: usage: atriad <socket-path> [--vnc <address>] [--shell <socket-path>]");
        exit(2);
    });
    let watching = args().skip_while(|argument| argument != "--vnc").nth(1);
    // A second socket, for the shell. Authority comes from which socket a program could open,
    // which the filesystem enforces — rather than from anything a client says about itself.
    let shell_path = args().skip_while(|argument| argument != "--shell").nth(1);

    let listener = listen(&path)?;
    println!("atriad: listening on {path}");

    let layout = PixelLayout::new(4).map_err(|_| io::Error::other("four bytes a pixel"))?;
    let mut presenter = Presenter::new(
        Size {
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
        },
        layout,
    )
    .map_err(|_| io::Error::other("the output size is unusable"))?;

    // Bound whether or not anyone is watching, and presentation does not wait for them: a
    // compositor that behaved differently while being observed would be useless for confirming
    // how it behaves.
    let mut viewer = match watching {
        Some(address) => {
            let sink = VncSink::bind(&address, OUTPUT_WIDTH as u16, OUTPUT_HEIGHT as u16, "Atria")
                .map_err(|error| io::Error::other(error.to_string()))?;
            println!("atriad: a viewer may connect to {address}");
            Some(sink)
        }
        None => None,
    };

    let mut state = compositor(ServerLimits::default(), ConnectionLimits::default());

    // Advertised once. Which globals exist is the compositor's decision, and a client learns
    // them from its registry rather than being handed identifiers it did not choose.
    // The shell authority is offered to everyone and bindable only with the grant. What exists is
    // not a secret; holding it is what is gated.
    for kind in [
        ObjectKind::Compositor,
        ObjectKind::Shm,
        ObjectKind::Shell,
        ObjectKind::ShellControl,
        ObjectKind::Seat,
        ObjectKind::Shortcuts,
    ] {
        if state.advertise_global(kind, VERSION).is_none() {
            return Err(io::Error::other("a global could not be advertised"));
        }
    }

    // The software output, reported as a topology so it reaches clients the way a real display
    // will. Its identity comes from the connector it stands for, because nothing here has a panel
    // asserting one of its own.
    let identity = OutputIdentity(1);
    let delta = state.apply_topology(
        &[(
            identity,
            IdentitySource::Position,
            OutputInfo {
                size: Size {
                    width: OUTPUT_WIDTH,
                    height: OUTPUT_HEIGHT,
                },
                physical_millimetres: OUTPUT_MILLIMETRES,
                scale_numerator: 1,
                scale_denominator: 1,
                refresh_millihertz: OUTPUT_REFRESH_MILLIHERTZ,
            },
        )],
        VERSION,
    );
    if delta.arrived.len() != 1 {
        return Err(io::Error::other("the output could not be advertised"));
    }
    state.place_output(identity, Point { x: 0, y: 0 });
    println!("atriad: one output, {OUTPUT_WIDTH}x{OUTPUT_HEIGHT}");

    let shell_listener = match shell_path.as_deref() {
        Some(path) => {
            let listener = listen(path)?;
            println!("atriad: a shell may connect to {path}");
            Some(listener)
        }
        None => None,
    };

    let mut sessions: Vec<Session<UnixTransport>> = Vec::new();
    let mut composed = 0_u64;

    // Composed once before anything has connected. An output with nothing on it still has a
    // current state, and a viewer attaching to an idle compositor must be shown that state rather
    // than be left waiting for somebody else to draw.
    if let Some(sink) = viewer.as_mut() {
        composed += 1;
        if let Err(error) = present_all(
            &mut presenter,
            &mut state,
            &mut sessions,
            composed,
            &mut *sink,
        ) {
            eprintln!("atriad: could not compose an empty output: {error:?}");
        }
    }
    // A monotonic count, not a clock. Input events need an ordering, and nothing here has
    // measured a real one — calling this nanoseconds would be a timing claim with nothing
    // behind it.
    let mut clock = 0_u64;

    loop {
        // One wait covers the listener and every client. Serving clients in turn instead would
        // make each one's responsiveness depend on what the others are doing, which is the
        // difference between several clients and one at a time.
        // How many sessions this wait covers. A client admitted below joins the next wait, and
        // reading its slot out of an array built before it existed is how that goes wrong.
        let polled = sessions.len();
        let mut watched = Vec::with_capacity(polled + 2);
        watched.push(pollfd {
            fd: listener.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        });
        watched.push(pollfd {
            fd: shell_listener
                .as_ref()
                .map_or(-1, |listener| listener.as_raw_fd()),
            events: POLLIN,
            revents: 0,
        });
        watched.push(pollfd {
            fd: viewer.as_ref().map_or(-1, |sink| sink.wakeup().as_raw_fd()),
            events: POLLIN,
            revents: 0,
        });
        for session in &sessions {
            watched.push(pollfd {
                fd: session.transport().as_fd().as_raw_fd(),
                events: POLLIN,
                revents: 0,
            });
        }

        // Waits indefinitely: a viewer's input makes its wakeup readable, so there is nothing to
        // check for on a timer.
        // SAFETY: `poll` reads and writes exactly the descriptors in the slice, which is owned
        // here and outlives the call.
        let ready = unsafe { poll(watched.as_mut_ptr(), watched.len() as nfds_t, -1) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }

        if watched[0].revents & POLLIN != 0 {
            match admit(&listener, &mut state, false) {
                Ok(session) => {
                    println!(
                        "atriad: a client connected, {} now served",
                        sessions.len() + 1
                    );
                    sessions.push(session);
                }
                Err(error) => eprintln!("atriad: could not admit a client: {error}"),
            }
        }

        if watched[1].revents & POLLIN != 0
            && let Some(listener) = shell_listener.as_ref()
        {
            match admit(listener, &mut state, true) {
                Ok(session) => {
                    println!("atriad: a shell connected");
                    sessions.push(session);
                }
                Err(error) => eprintln!("atriad: could not admit a shell: {error}"),
            }
        }

        let mut dispatched = false;
        if let Some(sink) = viewer.as_ref() {
            // Input was lost, so what the compositor believes about held keys and buttons no
            // longer matches the device. A new routing epoch says so rather than guessing.
            if sink.overflowed() {
                eprintln!("atriad: input was lost; starting a new routing epoch");
                state.reset_pointer();
                dispatched = true;
            }
            for report in sink.take_input() {
                clock = clock.saturating_add(1);
                match report {
                    Input::Pointer(moved) => state.move_pointer(
                        Point {
                            x: moved.x,
                            y: moved.y,
                        },
                        clock,
                    ),
                    Input::Button(button) => {
                        state.pointer_button(button.button, button.pressed, clock);
                    }
                    Input::Key(key) => {
                        state.key(PhysicalKey::from_usage(key.usage), key.pressed, clock);
                    }
                }
                dispatched = true;
            }
        }

        let mut departed = Vec::new();
        for (index, session) in sessions.iter_mut().enumerate().take(polled) {
            if watched[index + WATCHED_LISTENERS].revents & (POLLIN | POLLHUP | POLLERR) == 0 {
                continue;
            }
            match session.serve_one(&mut state) {
                Ok(served) => dispatched |= served.dispatched,
                Err(SessionError::Closed) => departed.push(index),
                Err(SessionError::Transport(error)) => {
                    eprintln!("atriad: a client's transport failed: {error}");
                    departed.push(index);
                }
            }
        }

        // Removed back to front so an earlier removal cannot shift a later index. One client
        // leaving is one client leaving: the rest keep their sessions and their content.
        for index in departed.into_iter().rev() {
            let session = sessions.remove(index);
            state.close_connection(session.connection());
            println!(
                "atriad: a client disconnected, {} still served",
                sessions.len()
            );
            dispatched = true;
        }

        if dispatched && let Some(sink) = viewer.as_mut() {
            // Composed after the requests a wait delivered rather than on a clock: there is no
            // measured deadline to pace against yet, and inventing one would be a timing claim
            // with nothing behind it.
            composed += 1;
            if let Err(error) = present_all(
                &mut presenter,
                &mut state,
                &mut sessions,
                composed,
                &mut *sink,
            ) {
                eprintln!("atriad: could not compose: {error:?}");
            }
        }
    }
}

/// Accept one connection and take it as far as a session the compositor has admitted.
fn admit(
    listener: &OwnedFd,
    state: &mut CompositorState,
    shell: bool,
) -> io::Result<Session<UnixTransport>> {
    let socket = accept(listener)?;
    // What a connection may ever be granted is settled when it is admitted, from which socket it
    // arrived on. A client from the ordinary socket cannot be granted shell authority later,
    // whatever it asks for — the refusal is in what it was admitted with, not in a check further
    // down.
    let available = if shell {
        software_capabilities()
            .with(Capability::ShellControl)
            .with(Capability::ShortcutControl)
    } else {
        software_capabilities()
    };
    let connection = state
        .connect(available, CapabilitySet::empty())
        .map_err(|error| io::Error::other(format!("the connection was refused: {error:?}")))?;
    if shell {
        for granted in [Capability::ShellControl, Capability::ShortcutControl] {
            if let Err(error) = state.grant_capability(connection, granted) {
                state.close_connection(connection);
                return Err(io::Error::other(format!(
                    "a shell authority could not be granted: {error:?}"
                )));
            }
        }
    }
    if let Err(error) = establish_session(state, connection) {
        state.close_connection(connection);
        return Err(io::Error::other(format!(
            "the session could not be established: {error:?}"
        )));
    }
    Ok(Session::new(UnixTransport::new(socket), connection))
}

fn establish_session(
    state: &mut CompositorState,
    connection: ConnectionId,
) -> Result<(), StateError> {
    state.create_session(connection, ObjectId::from_raw(SESSION_ID), None, true)
}

/// Bind and listen on a `SOCK_SEQPACKET` socket at `path`.
fn listen(path: &str) -> io::Result<OwnedFd> {
    let mut address: sockaddr_un = unsafe { zeroed() };
    address.sun_family = AF_UNIX as sa_family_t;
    let bytes = path.as_bytes();
    if bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the socket path is longer than the address allows",
        ));
    }
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as c_char;
    }

    // A stale socket from a previous run would make binding fail. Removing it is what makes a
    // restart work without a manual step, which the recovery contract needs of a service.
    let _ = remove_file(path);

    // SAFETY: the three calls take a freshly created descriptor and an address that outlives them.
    let socket = unsafe {
        let raw = make_socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0);
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = OwnedFd::from_raw_fd(raw);
        if bind(
            raw,
            (&raw const address).cast::<sockaddr>(),
            size_of::<sockaddr_un>() as socklen_t,
        ) < 0
        {
            return Err(io::Error::last_os_error());
        }
        if listen_on(raw, 16) < 0 {
            return Err(io::Error::last_os_error());
        }
        socket
    };
    Ok(socket)
}

/// Take the next connection.
fn accept(listener: &OwnedFd) -> io::Result<OwnedFd> {
    // SAFETY: `accept4` returns an owned descriptor or -1, and takes no address here.
    let raw = unsafe { accept4(listener.as_raw_fd(), null_mut(), null_mut(), SOCK_CLOEXEC) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just accepted and is owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}
