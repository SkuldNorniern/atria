//! What the transport must guarantee for the framing above it to hold.

use std::os::fd::{FromRawFd, OwnedFd};

use libc::{
    AF_UNIX, MFD_CLOEXEC, SOCK_CLOEXEC, SOCK_SEQPACKET, ftruncate, memfd_create, off_t, socketpair,
};

use atria_compositor::{HandleResolver, ResolveError};
use atria_protocol::wire::{HandleIndex, HandleKind, MAX_MESSAGE_SIZE};
use atria_transport::{
    Envelope, EnvelopeResolver, MAX_HANDLES, SharedMemoryStore, Transport, TransportError,
    UnixTransport,
};

/// A connected `SOCK_SEQPACKET` pair.
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

/// A sized memory object, which is what a pool is carved out of.
fn memory(bytes: usize) -> OwnedFd {
    let name = c"atria-transport-test";
    // SAFETY: `memfd_create` returns an owned descriptor or -1, and the name is a valid C string.
    let raw = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
    assert!(raw >= 0, "the platform must provide memfd");
    // SAFETY: the descriptor was just created and is owned here.
    let handle = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: `ftruncate` sizes the object the descriptor names.
    let sized = unsafe { ftruncate(raw, bytes as off_t) };
    assert_eq!(sized, 0, "the object must take a size");
    handle
}

/// Message boundaries are the whole reason for `SOCK_SEQPACKET`: one receive is one message, so
/// the length in the header is checked against what arrived rather than used to find the end.
#[test]
fn each_message_arrives_whole_and_separate() {
    let (mut client, mut server) = pair();

    client
        .send(&[1_u8; 12], &[])
        .expect("a short message sends");
    client.send(&[2_u8; 40], &[]).expect("a longer one sends");

    let first = server.receive().expect("the first message arrives");
    assert_eq!(first.bytes().len(), 12);
    assert_eq!(first.bytes()[0], 1);

    let second = server.receive().expect("the second arrives separately");
    assert_eq!(second.bytes().len(), 40);
    assert_eq!(second.bytes()[0], 2);
}

/// A handle travels beside the message, and resolution is what turns it into a resource. The
/// size comes from the descriptor, never from the client.
#[test]
fn a_handle_travels_beside_its_message_and_resolves_to_its_real_size() {
    let (mut client, mut server) = pair();
    let region = memory(4096);

    client
        .send(&[7_u8; 24], &[&region])
        .expect("a message with a handle sends");

    let mut envelope = server.receive().expect("it arrives");
    assert_eq!(envelope.handle_count(), 1);

    let mut store = SharedMemoryStore::new();
    let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);
    let resolved = resolver
        .shared_memory(HandleIndex::new(0, HandleKind::SharedMemory))
        .expect("slot zero holds the memory");

    assert_eq!(resolved.size(), 4096, "the size is read from the object");
    assert_eq!(store.len(), 1, "the store keeps the descriptor");
}

/// A slot may be claimed once. Claiming it twice would hand the same resource to two owners,
/// and the second would outlive a release it never saw.
#[test]
fn a_slot_is_claimed_once() {
    let (mut client, mut server) = pair();
    client
        .send(&[7_u8; 24], &[&memory(1024)])
        .expect("a message with a handle sends");
    let mut envelope = server.receive().expect("it arrives");
    let mut store = SharedMemoryStore::new();
    let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);
    let slot = HandleIndex::new(0, HandleKind::SharedMemory);

    resolver
        .shared_memory(slot)
        .expect("the first claim takes it");
    assert_eq!(
        resolver.shared_memory(slot),
        Err(ResolveError::SlotEmpty { slot: 0 }),
        "the second finds it gone"
    );
}

/// Naming a slot nothing was sent in is a resolution failure, not a protocol error: the client's
/// message was well formed and its transport carried nothing.
#[test]
fn an_unfilled_slot_is_a_resolution_failure() {
    let (mut client, mut server) = pair();
    client.send(&[7_u8; 24], &[]).expect("a bare message sends");
    let mut envelope = server.receive().expect("it arrives");
    let mut store = SharedMemoryStore::new();
    let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);

    assert_eq!(
        resolver.shared_memory(HandleIndex::new(2, HandleKind::SharedMemory)),
        Err(ResolveError::SlotEmpty { slot: 2 })
    );
}

/// A descriptor with no size cannot back a pool. Refusing it here, rather than at the first
/// buffer carved out of it, keeps the failure where the client can still be told about it.
#[test]
fn a_descriptor_that_is_not_sized_memory_is_refused() {
    let (mut client, mut server) = pair();
    let (spare, _keep) = pair();
    let socket = spare.into_socket();

    client
        .send(&[7_u8; 24], &[&socket])
        .expect("any descriptor may be sent");
    let mut envelope = server.receive().expect("it arrives");
    let mut store = SharedMemoryStore::new();
    let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);

    assert_eq!(
        resolver.shared_memory(HandleIndex::new(0, HandleKind::SharedMemory)),
        Err(ResolveError::WrongKind {
            slot: 0,
            expected: HandleKind::SharedMemory
        })
    );
    assert!(store.is_empty(), "nothing refused is kept");
}

/// The protocol's ceiling is enforced before the platform sees the message.
#[test]
fn a_message_past_the_ceiling_is_refused_before_it_is_sent() {
    let (mut client, _server) = pair();
    let oversize = vec![0_u8; MAX_MESSAGE_SIZE + 4];

    assert!(matches!(
        client.send(&oversize, &[]),
        Err(TransportError::MessageTooLarge { .. })
    ));
}

/// More handles than the transport carries is refused rather than truncated, because a truncated
/// control message silently drops descriptors the sender believes it handed over.
#[test]
fn more_handles_than_the_transport_carries_are_refused() {
    let (mut client, _server) = pair();
    let regions: Vec<OwnedFd> = (0..=MAX_HANDLES).map(|_| memory(64)).collect();
    let borrowed: Vec<&OwnedFd> = regions.iter().collect();

    assert!(matches!(
        client.send(&[0_u8; 12], &borrowed),
        Err(TransportError::TooManyHandles { .. })
    ));
}

/// A closed peer is reported as closed, not as an unexplained platform error.
#[test]
fn a_closed_peer_is_reported_as_closed() {
    let (mut client, server) = pair();
    drop(server);

    let result = client.receive();
    assert!(
        matches!(result, Err(TransportError::Closed)),
        "expected a closed connection, got {result:?}"
    );
}

/// A packet too short to hold a header is refused by the transport, so nothing above it has to
/// consider a message that cannot name an object.
#[test]
fn a_packet_shorter_than_a_header_is_refused() {
    let (mut client, mut server) = pair();
    client.send(&[0_u8; 4], &[]).expect("a short packet sends");

    assert!(matches!(
        server.receive(),
        Err(TransportError::Truncated { size: 4 })
    ));
}

/// `Envelope` is what the decode stage reads, so it must hand over the exact bytes that arrived.
#[test]
fn an_envelope_carries_the_bytes_that_arrived() {
    let (mut client, mut server) = pair();
    let message: Vec<u8> = (0..64_u8).collect();
    client.send(&message, &[]).expect("it sends");

    let envelope: Envelope = server.receive().expect("it arrives");
    assert_eq!(envelope.bytes(), message.as_slice());
}
