//! Turning a decoded wire frame into a [`ClientRequest`].
//!
//! This is the join between `atria-protocol`, which knows how bytes are framed, and the state
//! machine beside it, which knows what a request means. Neither half can do it alone: an opcode
//! is interface-local, and only the object's kind says which interface a frame is addressed to.
//!
//! Nothing here invents wire vocabulary. Every opcode and payload comes from the interface table
//! in `atria-protocol`, and an operation the state machine has no request for is reported as
//! unmodelled rather than guessed at.

use atria_protocol::interface::{Interface, MessageKind, Operation, decode_operation};
use atria_protocol::message::{
    Attach, Bind, Commit, CreateBuffer, CreatePool, DamageBuffer, GetRegistry, NewId,
};
use atria_protocol::wire::{Frame, HandleIndex};
use atria_protocol::{DecodeError, ObjectId, Opcode};

use crate::model::{ClientRequest, ObjectKind, Point, Rect, Size};
use crate::resolve::{HandleResolver, ResolveError, SharedMemory};

/// Which interface an object of this kind answers, when the draw path defines one.
///
/// Seats, sessions, fences and input streams exist in the compositor and have no assigned
/// interface: input and explicit synchronization are deliberately outside the draw path. A kind
/// with no interface cannot be addressed from the wire at all, which is the honest state — better
/// than mapping it to some interface and refusing every opcode.
#[must_use]
pub const fn interface_of(kind: ObjectKind) -> Option<Interface> {
    match kind {
        ObjectKind::Display => Some(Interface::Display),
        ObjectKind::Registry => Some(Interface::Registry),
        ObjectKind::Surface => Some(Interface::Surface),
        ObjectKind::Buffer => Some(Interface::Buffer),
        ObjectKind::Compositor => Some(Interface::Compositor),
        ObjectKind::Shm => Some(Interface::Shm),
        ObjectKind::ShmPool => Some(Interface::ShmPool),
        ObjectKind::Seat | ObjectKind::Session | ObjectKind::Fence | ObjectKind::InputStream => {
            None
        }
    }
}

/// Why a frame could not be bound to a request.
///
/// [`Self::Unmodelled`] is not a client error. The wire defines the operation completely; the
/// state machine has no [`ClientRequest`] for it yet. It is reported so that gap stays visible,
/// and each one becomes a real binding as the state machine grows a request to match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindError {
    /// No operation of this interface has this opcode as a method.
    UnknownOpcode { opcode: Opcode },
    /// The wire defines this operation; the compositor does not model it yet.
    Unmodelled { operation: Operation },
    /// The object's kind has no interface on the draw path, so nothing may be sent to it.
    InterfaceUnassigned { kind: ObjectKind },
    /// The payload did not decode as the operation's layout.
    Payload(DecodeError),
}

impl From<DecodeError> for BindError {
    fn from(error: DecodeError) -> Self {
        Self::Payload(error)
    }
}

/// A request as the wire described it, with handle slots still unresolved.
///
/// The separate stage exists so decoding can be tested against bytes alone: nothing here has
/// consulted a transport, so nothing here can fail for a reason the client is not responsible
/// for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodedRequest {
    CreateRegistry {
        new_id: ObjectId,
    },
    /// The pool's memory is still a slot in the message's handle array.
    CreatePool {
        new_id: ObjectId,
        memory: HandleIndex,
        size: u32,
    },
    CreateBuffer {
        new_id: ObjectId,
        pool: ObjectId,
        offset: u32,
        size: Size,
        stride: u32,
        format: u32,
    },
    CreateSurface {
        new_id: ObjectId,
    },
    Bind {
        name: u32,
        version: u32,
        new_id: ObjectId,
    },
    Attach {
        surface: ObjectId,
        buffer: ObjectId,
        offset: Point,
    },
    Damage {
        surface: ObjectId,
        rect: Rect,
    },
    Commit {
        surface: ObjectId,
    },
    Destroy {
        object: ObjectId,
    },
}

/// Read one frame addressed to an object of `kind`. Performs no I/O and consults no transport.
///
/// The caller supplies the kind because object identity is connection state, which this function
/// deliberately does not hold.
pub fn decode(kind: ObjectKind, frame: &Frame<'_>) -> Result<DecodedRequest, BindError> {
    let object = frame.header.object_id;
    let opcode = frame.header.opcode;
    let Some(interface) = interface_of(kind) else {
        return Err(BindError::InterfaceUnassigned { kind });
    };

    let operation = decode_operation(interface, MessageKind::Method, opcode)
        .map_err(|_| BindError::UnknownOpcode { opcode })?;

    match operation {
        Operation::DisplayGetRegistry => {
            let payload = GetRegistry::decode(frame.payload)?;
            Ok(DecodedRequest::CreateRegistry {
                new_id: payload.new_id,
            })
        }
        Operation::ShmCreatePool => {
            let payload = CreatePool::decode(frame.payload)?;
            Ok(DecodedRequest::CreatePool {
                new_id: payload.new_id,
                memory: payload.memory,
                size: payload.size,
            })
        }
        Operation::ShmPoolCreateBuffer => {
            let payload = CreateBuffer::decode(frame.payload)?;
            Ok(DecodedRequest::CreateBuffer {
                new_id: payload.new_id,
                pool: object,
                offset: payload.offset,
                size: Size {
                    width: payload.width,
                    height: payload.height,
                },
                stride: payload.stride,
                format: payload.format,
            })
        }
        Operation::RegistryBind => {
            let payload = Bind::decode(frame.payload)?;
            Ok(DecodedRequest::Bind {
                name: payload.name,
                version: payload.version,
                new_id: payload.new_id,
            })
        }
        Operation::CompositorCreateSurface => {
            let payload = NewId::decode(frame.payload)?;
            Ok(DecodedRequest::CreateSurface {
                new_id: payload.new_id,
            })
        }
        Operation::SurfaceAttach => {
            let payload = Attach::decode(frame.payload)?;
            Ok(DecodedRequest::Attach {
                surface: object,
                buffer: payload.buffer,
                offset: Point {
                    x: payload.x_offset,
                    y: payload.y_offset,
                },
            })
        }
        Operation::SurfaceDamageBuffer => {
            let payload = DamageBuffer::decode(frame.payload)?;
            Ok(DecodedRequest::Damage {
                surface: object,
                rect: Rect {
                    x: payload.x,
                    y: payload.y,
                    width: payload.width,
                    height: payload.height,
                },
            })
        }
        Operation::SurfaceCommit => {
            // `commit_id` and `configure_serial` decode and are not yet carried into the state
            // machine: it assigns its own commit identifiers, and nothing sends a configure while
            // no shell is attached. Both become fields the day a shell does.
            let _ = Commit::decode(frame.payload)?;
            Ok(DecodedRequest::Commit { surface: object })
        }
        Operation::SurfaceDestroy
        | Operation::BufferDestroy
        | Operation::ShmPoolDestroy
        | Operation::ToplevelDestroy => Ok(DecodedRequest::Destroy { object }),
        // Defined on the wire, with no request in the state machine yet. Each is a decode
        // waiting for the compositor to grow somewhere to put it.
        _ => Err(BindError::Unmodelled { operation }),
    }
}

/// Turn the handle slots a decoded request names into the resources they stand for.
///
/// # Errors
///
/// Returns [`ResolveError`] when a slot is empty, holds the wrong kind of resource, or is too
/// small for what the message says it holds. A request with no handles cannot fail here.
pub fn resolve(
    request: DecodedRequest,
    handles: &mut impl HandleResolver,
) -> Result<ClientRequest, ResolveError> {
    match request {
        DecodedRequest::CreateRegistry { new_id } => Ok(ClientRequest::CreateRegistry { new_id }),
        DecodedRequest::CreateBuffer {
            new_id,
            pool,
            offset,
            size,
            stride,
            format,
        } => Ok(ClientRequest::CreateBuffer {
            new_id,
            pool,
            offset,
            size,
            stride,
            format,
        }),
        DecodedRequest::CreateSurface { new_id } => Ok(ClientRequest::CreateSurface { new_id }),
        DecodedRequest::Bind {
            name,
            version,
            new_id,
        } => Ok(ClientRequest::Bind {
            name,
            version,
            new_id,
        }),
        DecodedRequest::Attach {
            surface,
            buffer,
            offset,
        } => Ok(ClientRequest::Attach {
            surface,
            buffer,
            offset,
            acquire_fence: None,
        }),
        DecodedRequest::Damage { surface, rect } => Ok(ClientRequest::Damage { surface, rect }),
        DecodedRequest::Commit { surface } => Ok(ClientRequest::Commit { surface }),
        DecodedRequest::Destroy { object } => Ok(ClientRequest::Destroy { object }),
        DecodedRequest::CreatePool {
            new_id,
            memory,
            size,
        } => {
            let resource = handles.shared_memory(memory)?;
            // The message states how much of the resource the pool covers, and the resource
            // states how much there is. Believing the message would let a client describe a pool
            // larger than the memory behind it, and every buffer carved from it would be checked
            // against a size that was never true.
            if u64::from(size) > resource.size() {
                return Err(ResolveError::TooSmall {
                    slot: memory.slot(),
                    needed: u64::from(size),
                    actual: resource.size(),
                });
            }
            Ok(ClientRequest::CreatePool {
                new_id,
                memory: SharedMemory::new(resource.id(), u64::from(size)),
            })
        }
    }
}
