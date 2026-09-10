//! Turning a decoded wire frame into a [`ClientRequest`].
//!
//! This is the join between `atria-protocol`, which knows how bytes are framed, and the state
//! machine beside it, which knows what a request means. Neither half can do it alone: an opcode
//! is interface-local, and only the object's kind says which interface a frame is addressed to.
//!
//! Nothing here invents wire vocabulary. Where draft v0.1 names an operation but assigns it no
//! opcode, or assigns an opcode but specifies no payload, the binding reports that rather than
//! choosing a layout — a guess here becomes the format every client is written against.

use atria_protocol::message::{AttachWithFence, GetRegistry};
use atria_protocol::opcode::{Interface, MessageKind, Operation, decode_operation};
use atria_protocol::wire::Frame;
use atria_protocol::{DecodeError, Opcode};

use crate::model::{ClientRequest, ObjectKind, Point};

/// Which interface an object of this kind answers.
#[must_use]
pub const fn interface_of(kind: ObjectKind) -> Interface {
    match kind {
        ObjectKind::Display => Interface::Display,
        ObjectKind::Registry => Interface::Registry,
        ObjectKind::Seat => Interface::Seat,
        ObjectKind::Session => Interface::Session,
        ObjectKind::Surface => Interface::Surface,
        ObjectKind::Buffer => Interface::Buffer,
        ObjectKind::Fence => Interface::Fence,
        ObjectKind::InputStream => Interface::InputStream,
    }
}

/// Why a frame could not be bound to a request.
///
/// [`Self::PayloadUnspecified`] is not a client error. It says the draft assigned this operation
/// a number and never said what its payload contains, so no implementation can encode or decode
/// it. It is reported so that gap is visible as a gap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindError {
    /// No operation of this interface has this opcode as a method.
    UnknownOpcode { opcode: Opcode },
    /// Draft v0.1 assigns this operation an opcode but no payload.
    PayloadUnspecified { operation: Operation },
    /// The payload did not decode as the operation's specified layout.
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
    let interface = interface_of(kind);

    let operation = decode_operation(interface, MessageKind::Method, opcode)
        .map_err(|_| BindError::UnknownOpcode { opcode })?;

    match operation {
        Operation::DisplayGetRegistry => {
            let payload = GetRegistry::decode(frame.payload)?;
            Ok(ClientRequest::CreateRegistry {
                new_id: payload.new_id,
            })
        }
        Operation::SurfaceAttachWithFence => {
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
        // Numbered by §13, with no payload stated anywhere in the draft.
        Operation::SurfaceAttach
        | Operation::SurfaceDamage
        | Operation::SurfaceCommit
        | Operation::SurfaceSetOpaqueRegion
        | Operation::SurfaceSetRefreshRange
        | Operation::SurfaceDestroy => Err(BindError::PayloadUnspecified { operation }),
        // `surface.frame` never reaches here: §13 numbers it without saying whether a client
        // sends it or receives it, so `decode_operation` does not admit it as a method and the
        // frame is refused above as an unknown opcode. Inferring the direction from what other
        // protocols do is exactly the guess this module exists to avoid.
        _ => Err(BindError::UnknownOpcode { opcode }),
    }
}
