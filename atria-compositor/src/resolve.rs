//! Turning the handle slots a decoded request names into the resources they stand for.
//!
//! This is the second of three stages, and it exists so the first can be tested against bytes
//! alone. Decoding says whether a packet is a well-formed message; resolution says whether the
//! handles it names are the resources it claims; dispatch says whether the compositor's state
//! allows the request. Collapsing any two of them makes a malformed packet indistinguishable
//! from a missing resource, and reports a client that sent nothing wrong as a protocol error.
//!
//! Nothing here maps memory. A slot becomes a [`SharedMemory`] — a validated resource with a
//! size — and when and how it is mapped belongs to the backend behind that resource. The
//! compositor never sees a pointer, which is what lets an Artery memory capability and a Unix
//! shared-memory descriptor sit behind one contract.

use atria_protocol::wire::{HandleIndex, HandleKind};

/// Why a handle slot could not become the resource its field requires.
///
/// Distinct from a decode failure because the message was well formed, and from a dispatch
/// failure because the compositor's state was never consulted. A client whose transport dropped
/// a handle has not violated the protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveError {
    /// The message named a slot the transport did not carry a handle in.
    SlotEmpty { slot: u8 },
    /// The slot holds a resource of the wrong kind for the field that named it.
    WrongKind {
        slot: u8,
        expected: HandleKind,
        actual: HandleKind,
    },
    /// The resource is of the right kind but does not permit what the request needs.
    InsufficientRights { slot: u8 },
    /// The resource is too small for what the message says it holds.
    TooSmall { slot: u8, needed: u64, actual: u64 },
}

/// Shared memory a client handed over, validated and not yet mapped.
///
/// Opaque on purpose. The compositor is allowed to know how large it is and to name it; reading
/// or writing it is the backend's, through whatever mapping that platform uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedMemory {
    id: u64,
    size: u64,
}

impl SharedMemory {
    /// Record a resource a backend has already validated.
    #[must_use]
    pub const fn new(id: u64, size: u64) -> Self {
        Self { id, size }
    }

    /// The backend's name for this resource.
    #[must_use]
    pub const fn id(self) -> u64 {
        self.id
    }

    /// Bytes the resource holds. A buffer carved out of it must fit inside this.
    #[must_use]
    pub const fn size(self) -> u64 {
        self.size
    }
}

/// What a transport must answer in order for a decoded request to become a dispatchable one.
///
/// The compositor takes this rather than reaching for a platform: a Unix transport resolves a
/// slot through its ancillary descriptor array, an Artery transport through the capability
/// handles moved with the message, and a test through a table it filled in itself.
pub trait HandleResolver {
    /// The shared memory in `slot`, or why it is not.
    ///
    /// # Errors
    ///
    /// Returns [`ResolveError`] when the slot is empty, holds another kind, or does not permit
    /// what the caller needs.
    fn shared_memory(&self, slot: HandleIndex) -> Result<SharedMemory, ResolveError>;
}
