//! The Atria display server.
//!
//! Listens on a Unix socket and serves each client that connects. One thread, one client at a
//! time: the presentation thread the plan calls for is a later split, and splitting before there
//! is a measured deadline to split around would be guessing.

use std::env::args;
use std::fs::remove_file;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{ExitCode, exit};
use std::ptr::null_mut;

use libc::{
    AF_UNIX, SOCK_CLOEXEC, SOCK_SEQPACKET, accept4, bind, c_char, listen as listen_on, sa_family_t,
    sockaddr, sockaddr_un, socket as make_socket, socklen_t,
};

use atria_compositor::{
    CompositorState, ConnectionId, ConnectionLimits, ObjectKind, ServerLimits, Size, StateError,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::CapabilitySet;
use atria_software_output::PixelLayout;
use atria_transport::UnixTransport;
use atria_vnc::VncSink;
use atriad::{
    Presenter, Session, SessionError, compositor, present_committed, software_capabilities,
};

/// The session the server establishes for each connection.
///
/// A session is not a global: a connection belongs to one, and does not choose it.
const SESSION_ID: u32 = 9;

/// The version each global is advertised at. One, because none of them has a second yet.
const VERSION: u32 = 1;

/// The output this server composes for, until a real display backend chooses it.
const OUTPUT_WIDTH: u32 = 1280;
const OUTPUT_HEIGHT: u32 = 720;

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
        eprintln!("atriad: usage: atriad <socket-path> [--vnc <address>]");
        exit(2);
    });
    let watching = args().skip_while(|argument| argument != "--vnc").nth(1);

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
    for kind in [ObjectKind::Compositor, ObjectKind::Shm] {
        if state.advertise_global(kind, VERSION).is_none() {
            return Err(io::Error::other("a global could not be advertised"));
        }
    }

    loop {
        let socket = accept(&listener)?;
        let connection = match state.connect(software_capabilities(), CapabilitySet::empty()) {
            Ok(connection) => connection,
            Err(error) => {
                // A refusal here is the connection bound doing its job. The socket is dropped,
                // which the client sees as the connection closing.
                eprintln!("atriad: refused a connection: {error}");
                continue;
            }
        };

        if let Err(error) = establish_session(&mut state, connection) {
            eprintln!("atriad: could not establish a session: {error}");
            state.close_connection(connection);
            continue;
        }

        let mut session = Session::new(UnixTransport::new(socket), connection);
        println!("atriad: client connected");
        loop {
            match session.serve_one(&mut state) {
                Ok(served) => {
                    if served.dispatched
                        && let Some(sink) = viewer.as_mut()
                    {
                        // Composed after every accepted request rather than on a clock: there is
                        // no measured deadline to pace against yet, and inventing one would be
                        // a timing claim with nothing behind it.
                        if let Err(error) = present_committed(
                            &mut presenter,
                            &mut state,
                            &mut session,
                            0,
                            &mut *sink,
                        ) {
                            eprintln!("atriad: could not compose: {error:?}");
                        }
                    }
                }
                Err(SessionError::Closed) => break,
                Err(SessionError::Transport(error)) => {
                    eprintln!("atriad: transport failed: {error}");
                    break;
                }
            }
        }
        state.close_connection(connection);
        println!("atriad: client disconnected");
    }
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
