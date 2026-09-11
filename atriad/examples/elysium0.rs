//! The smallest thing that is a shell.
//!
//! It binds the shell authority, takes the snapshot of windows that already exist, and cascades
//! everything it is told about. That is all it does — no dock, no materials, no animation, no
//! spaces. Its purpose is to prove the boundary underneath it: that Atria composes with no shell,
//! that a shell can attach to a compositor already running, that windows outlive the shell, and
//! that a replacement is handed the same handles its predecessor held.
//!
//! Run with: cargo run --example elysium0 -- <shell-socket-path> [super|alt|control|shift]

use std::env::args;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::exit;

use atria_protocol::interface::{Interface, Operation};
use atria_protocol::message::{
    Bind, EncodePayload, NewId, RegistryGlobal, SeatHandle, SeatPoint, ShellHandle,
    ShellInteraction, ShellPlace, ShellToplevel, ShortcutRegistration, ShortcutTriggered,
    encode_message,
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

/// The chord manager, and the chords this shell claims.
const SHORTCUTS: u32 = 4;
const CLOSE_FOCUSED: u32 = 1;
const CYCLE_WINDOWS: u32 = 2;

/// HID usages for the keys those chords use.
const USAGE_Q: u32 = 0x14;
const USAGE_TAB: u32 = 0x2b;

/// The modifier bits, as the compositor reports them.
const MOD_CONTROL: u32 = 1 << 0;
const MOD_SHIFT: u32 = 1 << 1;
const MOD_ALT: u32 = 1 << 2;
const MOD_META: u32 = 1 << 3;

/// Modifiers must match exactly, so Super+Shift+Q is somebody else's chord.
const MATCH_EXACT: u32 = 0;

/// Each window is offset from the last by this much, so none hides another completely.
///
/// The crudest arrangement that leaves every window reachable. Deliberately not clever: a shell
/// that cascades is obviously a shell, and anything more would be window management this is too
/// early to be designing.
const CASCADE_STEP: i32 = 32;

/// How far the cascade runs before starting again, so windows stay on the output.
const CASCADE_WRAP: i32 = 8;

/// The strip at the top of a window that the shell treats as its own to drag by.
const TITLE_STRIP: i32 = 28;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn main() {
    let path = args().nth(1).unwrap_or_else(|| {
        eprintln!("elysium0: usage: elysium0 <shell-socket-path> [super|alt|control|shift]");
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
    let manager = shell.global_named(Interface::Shortcuts);
    shell.request(
        id(REGISTRY),
        Operation::RegistryBind,
        &Bind {
            name: manager,
            version: 1,
            new_id: id(SHORTCUTS),
        },
    );
    // Claimed rather than watched for: the shell learns these chords fired and nothing about
    // anything else typed.
    // Which modifier holds the shell's chords is policy, and a desktop the viewer runs on
    // usually keeps Super for itself — so a remote session needs to be able to pick another.
    let modifier = match args().nth(2).as_deref() {
        Some("alt") => MOD_ALT,
        Some("control") => MOD_CONTROL,
        Some("shift") => MOD_SHIFT,
        _ => MOD_META,
    };
    for (shortcut, trigger) in [(CLOSE_FOCUSED, USAGE_Q), (CYCLE_WINDOWS, USAGE_TAB)] {
        shell.request(
            id(SHORTCUTS),
            Operation::ShortcutsRegister,
            &ShortcutRegistration {
                shortcut,
                seat: SEAT,
                trigger,
                modifiers: modifier,
                mode: MATCH_EXACT,
            },
        );
    }

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
    let mut focused: Option<u64> = None;
    let mut dragging: Option<u64> = None;
    let mut grabbed_at: Option<(i32, i32)> = None;
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
            Some(Told::Pressed { handle, x, y }) => {
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

                // A press in the top strip is a press on the window rather than in it. There is
                // no chrome drawn there yet, so the strip stands in for a title bar until
                // Elysium owns decoration of its own.
                if y < TITLE_STRIP {
                    shell.request(
                        id(CONTROL),
                        Operation::ShellControlGrab,
                        &SeatHandle { seat: SEAT, handle },
                    );
                    dragging = Some(handle);
                    // Where in the window the press landed is the offset to hold for the whole
                    // drag. Taking it from the first motion instead would lose however far the
                    // pointer travelled between the press and that motion, and the window would
                    // jump by exactly that much.
                    grabbed_at = Some((x, y));
                }
            }
            Some(Told::GrabMotion(x, y)) => {
                let Some(handle) = dragging else {
                    continue;
                };
                // The first report fixes where in the window the pointer was, so the window moves
                // with the pointer instead of jumping its own top-left corner to it.
                let Some((offset_x, offset_y)) = grabbed_at else {
                    continue;
                };
                shell.request(
                    id(CONTROL),
                    Operation::ShellControlPlace,
                    &ShellPlace {
                        handle,
                        x: x - offset_x,
                        y: y - offset_y,
                    },
                );
            }
            Some(Told::GrabEnd) => {
                dragging = None;
                grabbed_at = None;
            }
            Some(Told::Shortcut(CLOSE_FOCUSED)) => {
                let Some(handle) = focused else {
                    continue;
                };
                shell.request(
                    id(CONTROL),
                    Operation::ShellControlClose,
                    &ShellHandle { handle },
                );
                println!("elysium0: asked window {handle} to close");
            }
            Some(Told::Shortcut(CYCLE_WINDOWS)) => {
                let Some(next) = known
                    .iter()
                    .copied()
                    .cycle()
                    .skip_while(|handle| Some(*handle) != focused)
                    .nth(1)
                else {
                    continue;
                };
                shell.request(
                    id(CONTROL),
                    Operation::ShellControlRaise,
                    &ShellHandle { handle: next },
                );
                shell.request(
                    id(CONTROL),
                    Operation::ShellControlFocus,
                    &SeatHandle {
                        seat: SEAT,
                        handle: next,
                    },
                );
                println!("elysium0: cycled to window {next}");
            }
            Some(Told::Shortcut(_)) => {}
            Some(Told::FocusChanged(handle)) => {
                focused = (handle != 0).then_some(handle);
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
    /// Somebody pressed a window, and where in it. Nothing about the press itself.
    Pressed {
        handle: u64,
        x: i32,
        y: i32,
    },
    /// Where the pointer went while this shell held it.
    GrabMotion(i32, i32),
    /// The shell no longer holds the pointer.
    GrabEnd,
    /// A chord this shell claimed fired.
    Shortcut(u32),
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
        let opcode = frame.header.opcode.into_raw();
        // The object is checked before the opcode, always. An opcode is local to its interface,
        // so `shortcuts.triggered` and `shell_control.toplevel` are both zero.
        if frame.header.object_id == id(SHORTCUTS) {
            if opcode == Operation::ShortcutsTriggered.opcode() {
                return ShortcutTriggered::decode(frame.payload)
                    .ok()
                    .map(|payload| Told::Shortcut(payload.shortcut));
            }
            return Some(Told::Other);
        }
        if frame.header.object_id != id(CONTROL) {
            return Some(Told::Other);
        }
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
                .map(|payload| Told::Pressed {
                    handle: payload.handle,
                    x: payload.x,
                    y: payload.y,
                });
        }
        if opcode == Operation::ShellControlGrabMotion.opcode() {
            return SeatPoint::decode(frame.payload)
                .ok()
                .map(|payload| Told::GrabMotion(payload.x, payload.y));
        }
        if opcode == Operation::ShellControlGrabEnd.opcode() {
            return Some(Told::GrabEnd);
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
