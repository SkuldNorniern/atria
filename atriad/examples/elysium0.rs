//! The smallest thing that is a shell.
//!
//! It binds the shell authority, takes the snapshot of windows that already exist, and cascades
//! everything it is told about. That is all it does — no dock, no materials, no animation, no
//! spaces. Its purpose is to prove the boundary underneath it: that Atria composes with no shell,
//! that a shell can attach to a compositor already running, that windows outlive the shell, and
//! that a replacement is handed the same handles its predecessor held.
//!
//! Run with: cargo run --example elysium0 -- <shell-socket-path>

use std::env::args;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::exit;

use atria_protocol::interface::{Interface, Operation};
use atria_protocol::message::{
    Bind, EncodePayload, NewId, RegistryGlobal, SeatHandle, ShellHandle, ShellInteraction,
    ShellPlace, ShellToplevel, encode_message,
};
use atria_protocol::wire::{Frame, MAX_MESSAGE_SIZE};
use atria_protocol::{ObjectId, Opcode};
use atria_transport::{Transport, UnixTransport};
use libc::{
    AF_UNIX, SOCK_CLOEXEC, SOCK_SEQPACKET, c_char, connect, sa_family_t, sockaddr, sockaddr_un,
    socket as make_socket, socklen_t,
};

const REGISTRY: u32 = 2;
const CONTROL: u32 = 3;

/// The one seat this server has. Named rather than assumed, because focus belongs to a seat.
const SEAT: u64 = 1;

/// Each window is offset from the last by this much, so none hides another completely.
///
/// The crudest arrangement that leaves every window reachable. Deliberately not clever: a shell
/// that cascades is obviously a shell, and anything more would be window management this is too
/// early to be designing.
const CASCADE_STEP: i32 = 32;

/// How far the cascade runs before starting again, so windows stay on the output.
const CASCADE_WRAP: i32 = 8;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn main() {
    let path = args().nth(1).unwrap_or_else(|| {
        eprintln!("elysium0: usage: elysium0 <shell-socket-path>");
        exit(2);
    });

    let mut shell = Shell::new(connect_to(&path));
    shell.request(
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &NewId {
            new_id: id(REGISTRY),
        },
    );

    let authority = shell.global_named(Interface::ShellControl);
    shell.request(
        id(REGISTRY),
        Operation::RegistryBind,
        &Bind {
            name: authority,
            version: 1,
            new_id: id(CONTROL),
        },
    );

    // Everything that already exists arrives before the boundary. After it, the same event means
    // a window just appeared — which is why there is no second event for that case.
    let mut inherited = shell.take_snapshot();
    inherited.dedup();
    println!("elysium0: inherited {} window(s)", inherited.len());
    for (slot, handle) in inherited.iter().enumerate() {
        shell.place(*handle, slot);
    }

    // Keyed by handle, not counted. One event means "here is this window as it now stands", so
    // the same window arrives again whenever anything about it changes — a shell that counted
    // arrivals would move a window every time it was renamed.
    let mut known: Vec<u64> = inherited;
    loop {
        match shell.next_event() {
            Some(Told::Toplevel(handle)) => {
                if known.contains(&handle) {
                    continue;
                }
                shell.place(handle, known.len());
                known.push(handle);
                println!("elysium0: placed window {handle}");
            }
            Some(Told::ToplevelGone(handle)) => {
                known.retain(|known| *known != handle);
                println!("elysium0: window {handle} is gone");
            }
            Some(Told::Pressed(handle)) => {
                // Click to focus and raise. This is the whole of it: Atria routes the press to
                // the application as well, so the client gets its click and the shell gets to
                // decide what the press means for arrangement.
                shell.request(
                    id(CONTROL),
                    Operation::ShellControlRaise,
                    &ShellHandle { handle },
                );
                shell.request(
                    id(CONTROL),
                    Operation::ShellControlFocus,
                    &SeatHandle { seat: SEAT, handle },
                );
                println!("elysium0: raised and focused {handle}");
            }
            Some(Told::FocusChanged(handle)) => {
                println!("elysium0: focus is now {handle}");
            }
            Some(Told::Other) => {}
            None => return,
        }
    }
}

/// What the compositor told the shell.
enum Told {
    Toplevel(u64),
    ToplevelGone(u64),
    FocusChanged(u64),
    /// Somebody pressed a window. Which window, and nothing about the press itself.
    Pressed(u64),
    Other,
}

struct Shell {
    transport: UnixTransport,
    buffer: [u8; MAX_MESSAGE_SIZE],
    sequence: u32,
}

impl Shell {
    fn new(transport: UnixTransport) -> Self {
        Self {
            transport,
            buffer: [0; MAX_MESSAGE_SIZE],
            sequence: 1,
        }
    }

    /// Put a window at its place in the cascade.
    fn place(&mut self, handle: u64, slot: usize) {
        let step = (slot as i32) % CASCADE_WRAP;
        self.request(
            id(CONTROL),
            Operation::ShellControlPlace,
            &ShellPlace {
                handle,
                x: step * CASCADE_STEP,
                y: step * CASCADE_STEP,
            },
        );
        self.request(
            id(CONTROL),
            Operation::ShellControlRaise,
            &ShellHandle { handle },
        );
    }

    /// Read until the snapshot boundary, collecting the windows that already existed.
    fn take_snapshot(&mut self) -> Vec<u64> {
        let mut inherited = Vec::new();
        loop {
            let Some(envelope) = self.receive() else {
                return inherited;
            };
            let Ok(frame) = Frame::decode(&envelope) else {
                continue;
            };
            if frame.header.object_id != id(CONTROL) {
                continue;
            }
            let opcode = frame.header.opcode.into_raw();
            if opcode == Operation::ShellControlSnapshotDone.opcode() {
                return inherited;
            }
            if opcode == Operation::ShellControlToplevel.opcode()
                && let Ok(payload) = ShellToplevel::decode(frame.payload)
            {
                inherited.push(payload.handle);
            }
        }
    }

    fn next_event(&mut self) -> Option<Told> {
        let envelope = self.receive()?;
        let frame = Frame::decode(&envelope).ok()?;
        if frame.header.object_id != id(CONTROL) {
            return Some(Told::Other);
        }
        let opcode = frame.header.opcode.into_raw();
        if opcode == Operation::ShellControlToplevel.opcode() {
            return ShellToplevel::decode(frame.payload)
                .ok()
                .map(|payload| Told::Toplevel(payload.handle));
        }
        if opcode == Operation::ShellControlToplevelGone.opcode() {
            return ShellHandle::decode(frame.payload)
                .ok()
                .map(|payload| Told::ToplevelGone(payload.handle));
        }
        if opcode == Operation::ShellControlFocusChanged.opcode() {
            return SeatHandle::decode(frame.payload)
                .ok()
                .map(|payload| Told::FocusChanged(payload.handle));
        }
        if opcode == Operation::ShellControlInteraction.opcode() {
            return ShellInteraction::decode(frame.payload)
                .ok()
                .map(|payload| Told::Pressed(payload.handle));
        }
        Some(Told::Other)
    }

    /// Read announcements until the one for `interface` arrives, and return its name.
    fn global_named(&mut self, interface: Interface) -> u32 {
        loop {
            let envelope = self
                .receive()
                .unwrap_or_else(|| panic!("an announcement must arrive"));
            let Ok(frame) = Frame::decode(&envelope) else {
                continue;
            };
            if frame.header.object_id != id(REGISTRY)
                || frame.header.opcode.into_raw() != Operation::RegistryGlobal.opcode()
            {
                continue;
            }
            let Ok(announced) = RegistryGlobal::decode(frame.payload) else {
                continue;
            };
            if announced.interface == interface.name() {
                return announced.name;
            }
        }
    }

    fn receive(&mut self) -> Option<Vec<u8>> {
        let mut envelope = self.transport.receive().ok()?;
        Some(envelope.take_bytes())
    }

    fn request(&mut self, object: ObjectId, operation: Operation, payload: &impl EncodePayload) {
        let opcode = Opcode::from_raw(operation.opcode());
        let used = encode_message(object, opcode, self.sequence, payload, &mut self.buffer)
            .unwrap_or_else(|error| panic!("a request must encode: {error:?}"));
        self.sequence = self.sequence.wrapping_add(1);
        self.transport
            .send(&self.buffer[..used], &[])
            .unwrap_or_else(|error| panic!("a request must send: {error:?}"));
    }
}

fn connect_to(path: &str) -> UnixTransport {
    // SAFETY: a seqpacket socket in the Unix domain, or -1.
    let raw = unsafe { make_socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0) };
    assert!(raw >= 0, "the platform must provide a seqpacket socket");
    // SAFETY: the descriptor was just created and is owned here.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };

    // SAFETY: `sockaddr_un` is plain data with no invalid bit patterns.
    let mut address: sockaddr_un = unsafe { zeroed() };
    address.sun_family = AF_UNIX as sa_family_t;
    assert!(
        path.len() < address.sun_path.len(),
        "the socket path must fit in sun_path"
    );
    for (slot, byte) in address.sun_path.iter_mut().zip(path.as_bytes()) {
        *slot = *byte as c_char;
    }

    // SAFETY: the address is fully initialised and its length is the size of the structure.
    let joined = unsafe {
        connect(
            socket.as_raw_fd(),
            std::ptr::addr_of!(address).cast::<sockaddr>(),
            size_of::<sockaddr_un>() as socklen_t,
        )
    };
    assert_eq!(joined, 0, "atriad must be listening on {path}");
    UnixTransport::new(socket)
}
