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
use atria_protocol::message::{AttachWithFence, GetRegistry};
use atria_protocol::wire::Frame;
use atria_protocol::{DecodeError, Opcode};

use crate::model::{ClientRequest, ObjectKind, Point};

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

/// Bind one frame addressed to an object of `kind` to the request it carries.
///
/// The caller supplies the kind because object identity is connection state, which this function
/// deliberately does not hold.
pub fn request_from_frame(kind: ObjectKind, frame: &Frame<'_>) -> Result<ClientRequest, BindError> {
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
            Ok(ClientRequest::CreateRegistry {
                new_id: payload.new_id,
            })
        }
        Operation::SurfaceAttach => {
            let payload = AttachWithFence::decode(frame.payload)?;
            Ok(ClientRequest::Attach {
                surface: object,
                buffer: payload.buffer_id,
                offset: Point {
                    x: payload.x_offset,
                    y: payload.y_offset,
                },
                // The slot is resolved by the transport, which holds the handle array. Until one
                // exists there is nothing to resolve it against, so the fence is not yet bound.
                acquire_fence: None,
            })
        }
        // Defined on the wire, with no request in the state machine yet. Each is a decode
        // waiting for the compositor to grow somewhere to put it.
        _ => Err(BindError::Unmodelled { operation }),
    }
}
