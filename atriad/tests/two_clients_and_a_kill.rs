//! Two clients over real sockets, and one of them dying.
//!
//! The property a compositor is judged on is not that it composites. It is that one client's
//! death is one client's death — that the other keeps drawing, that what the dead one held is
//! given back, and that the compositor is still able to take a replacement.

use std::ffi::c_void;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use atria_compositor::{
    CompositorState, ConnectionLimits, FORMAT_XRGB8888, ObjectKind, Point, ServerLimits, Size,
    SurfaceKey,
};
use atria_protocol::capability::CapabilitySet;
use atria_protocol::interface::Operation;
use atria_protocol::message::{
    Attach, Bind, Commit, CreateBuffer, CreatePool, DamageBuffer, EncodePayload, NewId,
    encode_message,
};
use atria_protocol::wire::{HandleIndex, HandleKind, Header, MAX_MESSAGE_SIZE};
use atria_protocol::{ObjectId, Opcode};
use atria_software_output::{HeadlessSink, PixelLayout};
use atria_transport::{Transport, UnixTransport};
use atriad::{Presenter, Session, SessionError, present_for, software_capabilities};
use libc::{
    AF_UNIX, MFD_CLOEXEC, SOCK_CLOEXEC, SOCK_SEQPACKET, ftruncate, memfd_create, off_t, pwrite,
    socketpair,
};

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn pair() -> (UnixTransport, UnixTransport) {
    let mut fds = [0_i32; 2];
    // SAFETY: `socketpair` writes exactly two descriptors into the array provided.
    let made = unsafe { socketpair(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0, fds.as_mut_ptr()) };
    assert_eq!(made, 0, "the platform must provide a seqpacket pair");
    // SAFETY: both descriptors were just created and are owned by this process.
    unsafe {
        (
            UnixTransport::new(OwnedFd::from_raw_fd(fds[0])),
            UnixTransport::new(OwnedFd::from_raw_fd(fds[1])),
        )
    }
}

fn memory(bytes: usize) -> OwnedFd {
    let name = c"atriad-test";
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

/// Put bytes into a client's own shared memory, the way a client draws into it.
fn write_at(handle: &OwnedFd, offset: u32, bytes: &[u8]) {
    // SAFETY: `pwrite` reads `bytes`, which is borrowed here, and writes to the borrowed
    // descriptor without moving its offset.
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

/// A client, as seen from its own side of the socket.
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

    /// Read one event the compositor sent.
    fn receive_event(&mut self) -> Header {
        let envelope = self
            .transport
            .receive()
            .unwrap_or_else(|error| panic!("an event must arrive: {error:?}"));
        Header::decode(envelope.bytes())
            .unwrap_or_else(|error| panic!("an event must decode: {error:?}"))
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

/// Bring a client all the way to a committed frame.
fn draw(
    client: &mut Client,
    session: &mut Session,
    state: &mut CompositorState,
    pool: u32,
    buffer: u32,
    surface: u32,
) {
    let region = memory(4096);

    client.request_with_handle(
        id(10),
        Operation::ShmCreatePool,
        &CreatePool {
            new_id: id(pool),
            memory: HandleIndex::new(0, HandleKind::SharedMemory),
            size: 4096,
        },
        &region,
    );
    session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the pool is adopted: {error:?}"));

    client.request(
        id(pool),
        Operation::ShmPoolCreateBuffer,
        &CreateBuffer {
            new_id: id(buffer),
            offset: 0,
            width: 16,
            height: 16,
            stride: 64,
            format: FORMAT_XRGB8888,
        },
    );
    session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the buffer is carved: {error:?}"));

    client.request(
        id(11),
        Operation::CompositorCreateSurface,
        &NewId {
            new_id: id(surface),
        },
    );
    session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the surface is created: {error:?}"));

    client.request(
        id(surface),
        Operation::SurfaceAttach,
        &Attach {
            buffer: id(buffer),
            x_offset: 0,
            y_offset: 0,
        },
    );
    session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the buffer is attached: {error:?}"));

    client.request(
        id(surface),
        Operation::SurfaceDamageBuffer,
        &DamageBuffer {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
        },
    );
    session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the damage is staged: {error:?}"));

    client.request(
        id(surface),
        Operation::SurfaceCommit,
        &Commit {
            commit_id: 1,
            configure_serial: 0,
        },
    );
    session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the frame is committed: {error:?}"));
}

/// Which globals the compositor offers, advertised once as a server does at startup.
fn advertise(state: &mut CompositorState) -> (u32, u32) {
    let compositor = state
        .advertise_global(ObjectKind::Compositor, 1)
        .unwrap_or_else(|| panic!("the compositor global is advertised"));
    let shm = state
        .advertise_global(ObjectKind::Shm, 1)
        .unwrap_or_else(|| panic!("the shared-memory global is advertised"));
    (compositor, shm)
}

/// Accept a connection and take it as far as a client that has bound what it needs.
///
/// The client does the binding, from names it learned from its registry — the server hands it no
/// identifier it did not choose.
fn accept(
    state: &mut CompositorState,
    transport: UnixTransport,
    client: &mut Client,
    globals: (u32, u32),
) -> Session {
    let connection = state
        .connect(software_capabilities(), CapabilitySet::empty())
        .unwrap_or_else(|error| panic!("the server's own capabilities always overlap: {error:?}"));
    state
        .create_session(connection, id(9), None, true)
        .unwrap_or_else(|error| panic!("the connection's session is established: {error:?}"));
    let mut session = Session::new(transport, connection);

    client.request(
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &NewId { new_id: id(2) },
    );
    let served = session
        .serve_one(state)
        .unwrap_or_else(|error| panic!("the registry is created: {error:?}"));
    assert_eq!(
        served.events_sent, 2,
        "a fresh registry announces every global"
    );
    for _ in 0..2 {
        let announced = client.receive_event();
        assert_eq!(
            announced.opcode.into_raw(),
            Operation::RegistryGlobal.opcode(),
            "the client learns what exists rather than being handed an identifier"
        );
    }

    for (name, at) in [(globals.0, 11), (globals.1, 10)] {
        client.request(
            id(2),
            Operation::RegistryBind,
            &Bind {
                name,
                version: 1,
                new_id: id(at),
            },
        );
        session
            .serve_one(state)
            .unwrap_or_else(|error| panic!("a global binds: {error:?}"));
    }
    session
}

#[test]
fn two_clients_draw_at_once_and_one_dying_does_not_disturb_the_other() {
    let mut state = CompositorState::new(
        software_capabilities(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );

    let globals = advertise(&mut state);
    let (first_socket, first_server) = pair();
    let (second_socket, second_server) = pair();
    let mut first_client = Client::new(first_socket);
    let mut second_client = Client::new(second_socket);
    let mut first = accept(&mut state, first_server, &mut first_client, globals);
    let mut second = accept(&mut state, second_server, &mut second_client, globals);

    draw(&mut first_client, &mut first, &mut state, 256, 257, 258);
    draw(&mut second_client, &mut second, &mut state, 256, 257, 258);

    assert_eq!(
        state.object_kind(first.connection(), id(258)),
        Some(ObjectKind::Surface)
    );
    assert_eq!(
        state.object_kind(second.connection(), id(258)),
        Some(ObjectKind::Surface),
        "each connection names its own objects, and 258 is a different surface on each"
    );
    assert_eq!(first.adopted_memory(), 1);
    assert_eq!(second.adopted_memory(), 1);

    // The first client dies, abruptly: its socket simply goes.
    let dead = first.connection();
    drop(first_client);
    assert!(
        matches!(first.serve_one(&mut state), Err(SessionError::Closed)),
        "the server notices rather than blocking"
    );
    let teardown = state.close_connection(dead);
    assert!(
        teardown
            .destroyed
            .iter()
            .any(|(id, _)| id.into_raw() == 258),
        "what it held is given back"
    );
    assert!(!state.is_connected(dead));

    // The survivor is untouched, and still draws.
    assert!(state.is_connected(second.connection()));
    second_client.request(
        id(258),
        Operation::SurfaceDamageBuffer,
        &DamageBuffer {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
    );
    second
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("the surviving client keeps drawing: {error:?}"));
    second_client.request(
        id(258),
        Operation::SurfaceCommit,
        &Commit {
            commit_id: 2,
            configure_serial: 0,
        },
    );
    second
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("and commits another frame: {error:?}"));

    // And the compositor can still take a replacement.
    let (replacement_socket, replacement_server) = pair();
    let mut replacement_client = Client::new(replacement_socket);
    let replacement = accept(
        &mut state,
        replacement_server,
        &mut replacement_client,
        globals,
    );
    assert!(state.is_connected(replacement.connection()));
}

/// The chain closes: a client's pixels reach a composed frame, and the release that lets it draw
/// again comes back over the socket.
///
/// Until the release arrives a client cannot touch that memory, so this is what makes a second
/// frame possible rather than a nicety.
#[test]
fn a_clients_pixels_reach_a_frame_and_its_buffer_comes_back() {
    let mut state = CompositorState::new(
        software_capabilities(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let globals = advertise(&mut state);
    let (socket, server) = pair();
    let mut client = Client::new(socket);
    let mut session = accept(&mut state, server, &mut client, globals);

    let region = memory(4096);
    // A recognisable colour, so the frame proves the client's own bytes arrived rather than any
    // bytes at all.
    let colour = [0x20_u8, 0x60, 0xc0, 0xff];
    let pixels: Vec<u8> = colour.iter().copied().cycle().take(16 * 16 * 4).collect();
    write_at(&region, 0, &pixels);

    client.request_with_handle(
        id(10),
        Operation::ShmCreatePool,
        &CreatePool {
            new_id: id(256),
            memory: HandleIndex::new(0, HandleKind::SharedMemory),
            size: 4096,
        },
        &region,
    );
    session
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("the pool is adopted: {error:?}"));

    client.request(
        id(256),
        Operation::ShmPoolCreateBuffer,
        &CreateBuffer {
            new_id: id(257),
            offset: 0,
            width: 16,
            height: 16,
            stride: 64,
            format: FORMAT_XRGB8888,
        },
    );
    session
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("the buffer is carved: {error:?}"));

    client.request(
        id(11),
        Operation::CompositorCreateSurface,
        &NewId { new_id: id(258) },
    );
    session
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("the surface is created: {error:?}"));
    // With no shell attached there is nobody to place it, so the fallback puts it at the origin.
    state
        .place_surface(
            SurfaceKey {
                connection: session.connection(),
                object_id: id(258),
            },
            Point { x: 0, y: 0 },
        )
        .unwrap_or_else(|error| panic!("the fallback placement applies: {error:?}"));

    client.request(
        id(258),
        Operation::SurfaceAttach,
        &Attach {
            buffer: id(257),
            x_offset: 0,
            y_offset: 0,
        },
    );
    session
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("the buffer is attached: {error:?}"));
    client.request(
        id(258),
        Operation::SurfaceCommit,
        &Commit {
            commit_id: 1,
            configure_serial: 0,
        },
    );
    session
        .serve_one(&mut state)
        .unwrap_or_else(|error| panic!("the frame is committed: {error:?}"));

    let mut presenter = Presenter::new(
        Size {
            width: 64,
            height: 64,
        },
        PixelLayout::new(4).unwrap_or_else(|error| panic!("four bytes a pixel: {error:?}")),
    )
    .unwrap_or_else(|error| panic!("the output is usable: {error:?}"));
    let mut sink = HeadlessSink::default();

    let report = present_for(
        &mut presenter,
        &mut state,
        &mut session,
        id(257),
        1_000,
        &mut sink,
    )
    .unwrap_or_else(|error| panic!("the frame presents: {error:?}"));
    assert_eq!(
        report.surfaces_composited, 1,
        "the client's surface is in the frame"
    );

    // The top-left pixel of the frame is the client's colour, because that is where its surface
    // was placed and those are the bytes it wrote.
    assert_eq!(
        &presenter.frame().bytes()[..4],
        &colour,
        "the composed frame holds the client's own pixels"
    );

    // And the release travelled back, which is what lets the client draw again.
    let released = client.receive_event();
    assert_eq!(
        released.opcode.into_raw(),
        Operation::BufferRelease.opcode(),
        "the client is told its buffer is free"
    );
    assert_eq!(released.object_id, id(257));
}
