//! Carrying Atria messages, and the handles that travel beside them.
//!
//! A transport moves complete messages and resolves the handle slots those messages name. What a
//! handle *is* differs — a file descriptor here, a capability handle on Artery — and that
//! difference is confined to this package. The compositor above it sees validated resources and
//! never a platform representation.
//!
//! The trait exists so a second transport is a second implementation rather than a rewrite.

mod backend;
mod error;
mod memory;

pub use backend::unix::MAX_HANDLES;
pub use error::TransportError;
pub use memory::SharedMemoryStore;

use std::mem::take;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use atria_compositor::{HandleResolver, ResolveError, SharedMemory};
use atria_protocol::wire::{HEADER_SIZE, HandleIndex, HandleKind, MAX_MESSAGE_SIZE};

/// One complete message and the handles that arrived with it.
///
/// Handles are private: a slot is claimed through [`HandleResolver`], which takes each one at
/// most once, so nothing can name a slot twice and receive the same resource.
#[derive(Debug)]
pub struct Envelope<H> {
    bytes: Vec<u8>,
    handles: Vec<Option<H>>,
}

impl<H> Envelope<H> {
    /// Build an envelope from a message and the handles that arrived with it.
    #[must_use]
    pub fn new(bytes: Vec<u8>, handles: Vec<H>) -> Self {
        Self {
            bytes,
            handles: handles.into_iter().map(Some).collect(),
        }
    }

    /// The message's bytes, ready to decode.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Take the message's bytes, leaving the handles behind.
    ///
    /// The two halves of an envelope are independent, and decoding needs the bytes while
    /// resolution needs the handles. Taking the bytes out separates the borrows instead of
    /// forcing a decoded request to own everything it read.
    #[must_use]
    pub fn take_bytes(&mut self) -> Vec<u8> {
        take(&mut self.bytes)
    }

    /// How many handles arrived with this message.
    #[must_use]
    pub fn handle_count(&self) -> usize {
        self.handles.iter().filter(|slot| slot.is_some()).count()
    }

    /// Take the handle in `slot`, leaving the slot empty.
    fn claim(&mut self, slot: u8) -> Option<H> {
        self.handles.get_mut(usize::from(slot))?.take()
    }
}

/// Keeping the shared memory a client handed over, so the compositor need not.
///
/// The compositor holds a [`SharedMemory`] — an identity and a size — and never a platform
/// representation. What backs that identity is this trait's business: a file descriptor on a
/// host, an Artery memory capability natively.
pub trait SharedMemorySource {
    /// What this source adopts. Must be what its transport delivers.
    type Handle;
    /// Why memory could not be adopted or read. Platform detail belongs here, not in the trait.
    type Error;

    /// Take a handle and record how much memory it holds.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the handle does not name memory with a size.
    fn adopt(&mut self, handle: Self::Handle) -> Result<SharedMemory, Self::Error>;

    /// Read `len` bytes from `offset` of the memory an identity names.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the identity is unknown or the region is shorter than asked.
    fn read(&self, memory: SharedMemory, offset: u32, len: usize) -> Result<Vec<u8>, Self::Error>;

    /// Release what an identity named. Returns whether it was held.
    fn release(&mut self, memory: SharedMemory) -> bool;

    /// How many handles this source holds.
    fn len(&self) -> usize;

    /// Whether this source holds nothing.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Moving complete messages between a client and the compositor.
///
/// The handle type is the transport's, not the protocol's. A message names a slot; what arrives
/// in that slot is whatever this platform passes between processes, and nothing above the
/// transport learns which.
pub trait Transport {
    /// What travels beside a message on this platform.
    type Handle;
    /// Where handles that name shared memory are kept once adopted.
    type Memory: SharedMemorySource<Handle = Self::Handle> + Default;

    /// Send one message with the given handles attached.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the peer has gone or the platform refuses.
    fn send(&mut self, bytes: &[u8], handles: &[&Self::Handle]) -> Result<(), TransportError>;

    /// Receive one complete message.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Closed`] when the peer has gone.
    fn receive(&mut self) -> Result<Envelope<Self::Handle>, TransportError>;
}

/// A connection over a `SOCK_SEQPACKET` Unix socket.
pub struct UnixTransport {
    socket: OwnedFd,
}

impl UnixTransport {
    /// Take an already-connected socket. It must be `SOCK_SEQPACKET`, or message boundaries are
    /// not preserved and the framing this transport relies on does not hold.
    #[must_use]
    pub const fn new(socket: OwnedFd) -> Self {
        Self { socket }
    }

    /// Give the socket back, ending the transport without closing the connection.
    #[must_use]
    pub fn into_socket(self) -> OwnedFd {
        self.socket
    }
}

/// Lets a server wait on several clients at once. Readiness is the platform's, so it is exposed
/// on the platform's transport rather than on the trait every transport shares.
impl AsFd for UnixTransport {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

impl Transport for UnixTransport {
    type Handle = OwnedFd;
    type Memory = SharedMemoryStore;

    fn send(&mut self, bytes: &[u8], handles: &[&OwnedFd]) -> Result<(), TransportError> {
        if bytes.len() > MAX_MESSAGE_SIZE {
            return Err(TransportError::MessageTooLarge {
                size: bytes.len(),
                maximum: MAX_MESSAGE_SIZE,
            });
        }
        let borrowed: Vec<_> = handles.iter().map(|handle| handle.as_fd()).collect();
        backend::unix::send(self.socket.as_fd(), bytes, &borrowed)
    }

    fn receive(&mut self) -> Result<Envelope<OwnedFd>, TransportError> {
        let received = backend::unix::receive(self.socket.as_fd(), MAX_MESSAGE_SIZE)?;
        if received.bytes.len() < HEADER_SIZE {
            return Err(TransportError::Truncated {
                size: received.bytes.len(),
            });
        }
        Ok(Envelope::new(received.bytes, received.handles))
    }
}

/// Resolving an envelope's slots against a store that keeps what it adopts.
///
/// The envelope owns the descriptors until a slot is claimed; the store owns them afterwards, for
/// as long as whatever named them is alive. Splitting it this way is what lets the resource
/// outlive the message that delivered it without the compositor ever holding a descriptor.
pub struct EnvelopeResolver<'a, S: SharedMemorySource> {
    envelope: &'a mut Envelope<S::Handle>,
    store: &'a mut S,
}

impl<'a, S: SharedMemorySource> EnvelopeResolver<'a, S> {
    #[must_use]
    pub fn new(envelope: &'a mut Envelope<S::Handle>, store: &'a mut S) -> Self {
        Self { envelope, store }
    }
}

impl<S: SharedMemorySource> HandleResolver for EnvelopeResolver<'_, S> {
    fn shared_memory(&mut self, slot: HandleIndex) -> Result<SharedMemory, ResolveError> {
        let index = slot.slot();
        let handle = self
            .envelope
            .claim(index)
            .ok_or(ResolveError::SlotEmpty { slot: index })?;
        self.store
            .adopt(handle)
            .map_err(|_| ResolveError::WrongKind {
                slot: index,
                expected: HandleKind::SharedMemory,
            })
    }
}
