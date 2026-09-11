//! A client that connects to a running atriad and draws one solid rectangle.
//!
//! This exists so the composed frame can be looked at. Everything it does goes over the same
//! socket and the same wire format a real client uses — there is no in-process shortcut, because
//! a shortcut would prove the shortcut works.
//!
//! Run with: cargo run --example draw -- <socket-path> [width height rrggbb title]

use std::env::args;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::exit;

use atria_compositor::FORMAT_XRGB8888;
use atria_protocol::interface::{Interface, Operation};
use atria_protocol::message::{
    Attach, Bind, Commit, CreateBuffer, CreatePool, DamageBuffer, EncodePayload, GetToplevel,
    KeyboardKey, NewId, RegistryGlobal, SetTitle, encode_message,
};
use atria_protocol::wire::{Frame, HandleIndex, HandleKind, MAX_MESSAGE_SIZE};
use atria_protocol::{ObjectId, Opcode};
use atria_transport::{Transport, UnixTransport};
use libc::{
    AF_UNIX, MFD_CLOEXEC, SOCK_CLOEXEC, SOCK_SEQPACKET, c_char, connect, ftruncate, memfd_create,
    off_t, pwrite, sa_family_t, sockaddr, sockaddr_un, socket as make_socket, socklen_t,
};

const BYTES_PER_PIXEL: u32 = 4;

const REGISTRY: u32 = 2;
const COMPOSITOR: u32 = 3;
const SHM: u32 = 4;
const POOL: u32 = 5;
const BUFFER: u32 = 6;
const SURFACE: u32 = 7;
const SHELL: u32 = 8;
const TOPLEVEL: u32 = 12;
const SEAT: u32 = 13;
const KEYBOARD: u32 = 14;

/// How far each keypress turns the window's colour.
const HUE_STEP: u8 = 0x29;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn main() {
    let path = args().nth(1).unwrap_or_else(|| {
        eprintln!("draw: usage: draw <socket-path> [width height rrggbb title]");
        exit(2);
    });
    // Size and colour are arguments so that two of these are telling apart in one frame.
    let width = number(2, 400);
    let height = number(3, 300);
    let colour = colour(4, 0x2f_9e_d8);
    let title = args().nth(5).unwrap_or_else(|| String::from("window"));

    let mut client = Client::new(connect_to(&path));

    client.request(
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &NewId {
            new_id: id(REGISTRY),
        },
    );

    // A global is matched by the interface it announces, never by the order it arrives in.
    // Announcement order is the compositor's business and may change without the protocol
    // changing.
    let compositor_name = client.global_named(Interface::Compositor);
    let shm_name = client.global_named(Interface::Shm);
    let shell_name = client.global_named(Interface::Shell);
    let seat_name = client.global_named(Interface::Seat);

    client.request(
        id(REGISTRY),
        Operation::RegistryBind,
        &Bind {
            name: compositor_name,
            version: 1,
            new_id: id(COMPOSITOR),
        },
    );
    client.request(
        id(REGISTRY),
        Operation::RegistryBind,
        &Bind {
            name: shm_name,
            version: 1,
            new_id: id(SHM),
        },
    );

    client.request(
        id(REGISTRY),
        Operation::RegistryBind,
        &Bind {
            name: shell_name,
            version: 1,
            new_id: id(SHELL),
        },
    );

    client.request(
        id(REGISTRY),
        Operation::RegistryBind,
        &Bind {
            name: seat_name,
            version: 1,
            new_id: id(SEAT),
        },
    );
    client.request(
        id(SEAT),
        Operation::SeatGetKeyboard,
        &NewId {
            new_id: id(KEYBOARD),
        },
    );

    let stride = width * BYTES_PER_PIXEL;
    let bytes_of_pixels = (stride * height) as usize;
    let region = memory(bytes_of_pixels);
    let mut pixels = Vec::with_capacity(bytes_of_pixels);
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&colour);
    }
    write_at(&region, 0, &pixels);

    client.request_with_handle(
        id(SHM),
        Operation::ShmCreatePool,
        &CreatePool {
            new_id: id(POOL),
            memory: HandleIndex::new(0, HandleKind::SharedMemory),
            size: bytes_of_pixels as u32,
        },
        &region,
    );
    client.request(
        id(POOL),
        Operation::ShmPoolCreateBuffer,
        &CreateBuffer {
            new_id: id(BUFFER),
            offset: 0,
            width,
            height,
            stride,
            format: FORMAT_XRGB8888,
        },
    );
    client.request(
        id(COMPOSITOR),
        Operation::CompositorCreateSurface,
        &NewId {
            new_id: id(SURFACE),
        },
    );
    // A surface with a window role is a window, which is what a shell arranges. Without this the
    // client draws pixels nothing can be asked to move.
    client.request(
        id(SHELL),
        Operation::ShellGetToplevel,
        &GetToplevel {
            surface: id(SURFACE),
            new_id: id(TOPLEVEL),
        },
    );
    client.request(
        id(TOPLEVEL),
        Operation::ToplevelSetTitle,
        &SetTitle { title: &title },
    );

    client.request(
        id(SURFACE),
        Operation::SurfaceAttach,
        &Attach {
            buffer: id(BUFFER),
            x_offset: 0,
            y_offset: 0,
        },
    );
    client.request(
        id(SURFACE),
        Operation::SurfaceDamageBuffer,
        &DamageBuffer {
            x: 0,
            y: 0,
            width,
            height,
        },
    );
    client.request(
        id(SURFACE),
        Operation::SurfaceCommit,
        &Commit {
            commit_id: 1,
            configure_serial: 0,
        },
    );

    println!("draw: committed {width}x{height}");

    // Held open deliberately. A connection that closes takes its buffers with it, and there
    // would be nothing left to look at.
    // Repaint on every keypress, so typing is visible rather than merely delivered.
    let mut turned = 0_u8;
    loop {
        let Ok(mut envelope) = client.transport.receive() else {
            return;
        };
        let bytes = envelope.take_bytes();
        let Ok(frame) = Frame::decode(&bytes) else {
            continue;
        };
        // An opcode is local to its interface, so the object a message names says what it is.
        if frame.header.object_id == ObjectId::DISPLAY
            && frame.header.opcode.into_raw() == Operation::DisplayError.opcode()
        {
            eprintln!(
                "draw: the compositor refused something: {:?}",
                frame.payload
            );
            continue;
        }
        // A shell asking a window to close is a request, not an instruction. This client obeys
        // it; one with unsaved work would be entitled to ask first.
        if frame.header.object_id == id(TOPLEVEL)
            && frame.header.opcode.into_raw() == Operation::ToplevelClose.opcode()
        {
            println!("draw: the shell asked this window to close");
            return;
        }
        if frame.header.object_id != id(KEYBOARD)
            || frame.header.opcode.into_raw() != Operation::KeyboardKey.opcode()
        {
            continue;
        }
        let Ok(key) = KeyboardKey::decode(frame.payload) else {
            continue;
        };
        if key.state == 0 {
            continue;
        }
        turned = turned.wrapping_add(HUE_STEP);
        let shifted = [
            colour[0].wrapping_add(turned),
            colour[1].wrapping_add(turned.wrapping_mul(2)),
            colour[2].wrapping_sub(turned),
            0xff,
        ];
        let repainted: Vec<u8> = shifted
            .iter()
            .copied()
            .cycle()
            .take(bytes_of_pixels)
            .collect();
        write_at(&region, 0, &repainted);
        println!("draw: key {:#04x} repainted the window", key.key);
        client.request(
            id(SURFACE),
            Operation::SurfaceDamageBuffer,
            &DamageBuffer {
                x: 0,
                y: 0,
                width,
                height,
            },
        );
        client.request(
            id(SURFACE),
            Operation::SurfaceCommit,
            &Commit {
                commit_id: 2,
                configure_serial: 0,
            },
        );
    }
}

/// A positional argument, or a default when it is absent or unreadable.
fn number(position: usize, fallback: u32) -> u32 {
    args()
        .nth(position)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// A positional `rrggbb` argument, laid out the way a buffer stores it.
fn colour(position: usize, fallback: u32) -> [u8; 4] {
    let packed = args()
        .nth(position)
        .and_then(|value| u32::from_str_radix(&value, 16).ok())
        .unwrap_or(fallback);
    [
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
        0xff,
    ]
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

fn memory(bytes: usize) -> OwnedFd {
    let name = c"atria-draw";
    // SAFETY: `memfd_create` returns an owned descriptor or -1, and the name is a valid C string.
    let raw = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
    assert!(raw >= 0, "the platform must provide memfd");
    // SAFETY: the descriptor was just created and is owned here.
    let handle = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: `ftruncate` sizes the object the descriptor names.
    assert_eq!(
        unsafe { ftruncate(raw, bytes as off_t) },
        0,
        "the object must take a size"
    );
    handle
}

fn write_at(handle: &OwnedFd, offset: u32, bytes: &[u8]) {
    // SAFETY: `pwrite` reads `bytes`, borrowed here, and writes to the borrowed descriptor
    // without moving its offset.
    let written = unsafe {
        pwrite(
            handle.as_raw_fd(),
            bytes.as_ptr().cast::<c_void>(),
            bytes.len(),
            off_t::from(offset),
        )
    };
    assert_eq!(
        written,
        bytes.len() as isize,
        "the whole region must be written"
    );
}

struct Client {
    transport: UnixTransport,
    buffer: [u8; MAX_MESSAGE_SIZE],
    sequence: u32,
}

impl Client {
    fn new(transport: UnixTransport) -> Self {
        Self {
            transport,
            buffer: [0; MAX_MESSAGE_SIZE],
            sequence: 1,
        }
    }

    /// Read announcements until the one for `interface` arrives, and return its name.
    fn global_named(&mut self, interface: Interface) -> u32 {
        loop {
            let envelope = self
                .transport
                .receive()
                .unwrap_or_else(|error| panic!("an announcement must arrive: {error:?}"));
            let frame = Frame::decode(envelope.bytes())
                .unwrap_or_else(|error| panic!("an event must decode: {error:?}"));
            // Addressed to the registry, because an opcode alone does not say which interface a
            // message belongs to.
            if frame.header.object_id != id(REGISTRY)
                || frame.header.opcode.into_raw() != Operation::RegistryGlobal.opcode()
            {
                continue;
            }
            let announced = RegistryGlobal::decode(frame.payload)
                .unwrap_or_else(|error| panic!("an announcement must decode: {error:?}"));
            if announced.interface == interface.name() {
                return announced.name;
            }
        }
    }

    fn request(&mut self, object: ObjectId, operation: Operation, payload: &impl EncodePayload) {
        self.send(object, operation, payload, &[]);
    }

    fn request_with_handle(
        &mut self,
        object: ObjectId,
        operation: Operation,
        payload: &impl EncodePayload,
        handle: &OwnedFd,
    ) {
        self.send(object, operation, payload, &[handle]);
    }

    fn send(
        &mut self,
        object: ObjectId,
        operation: Operation,
        payload: &impl EncodePayload,
        handles: &[&OwnedFd],
    ) {
        let opcode = Opcode::from_raw(operation.opcode());
        let used = encode_message(object, opcode, self.sequence, payload, &mut self.buffer)
            .unwrap_or_else(|error| panic!("a request must encode: {error:?}"));
        self.sequence = self.sequence.wrapping_add(1);
        self.transport
            .send(&self.buffer[..used], handles)
            .unwrap_or_else(|error| panic!("a request must send: {error:?}"));
    }
}
