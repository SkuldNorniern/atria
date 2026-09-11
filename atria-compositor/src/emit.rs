//! Turning an event the state machine produced into bytes for the client.
//!
//! The mirror of the binding, and it has the same discipline: an event with no assigned wire form
//! is reported rather than dropped. A compositor that silently discards an event it cannot encode
//! leaves a client waiting for something that was decided and never sent, which is the hardest
//! class of bug to find from the outside.

use atria_protocol::error::ErrorCode as WireErrorCode;
use atria_protocol::interface::Operation;
use atria_protocol::message::{
    DisplayError, EncodePayload, FrameDone, GlobalName, NewId, RegistryGlobal, encode_message,
};
use atria_protocol::wire::Encoder;
use atria_protocol::{EncodeError, ObjectId, Opcode};

use crate::error::StateError;
use crate::model::{Event, EventKind};

/// Why an event could not be put on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmitError {
    /// The draw path assigns this event no wire form yet.
    ///
    /// Not a failure of the client or the compositor. Input, explicit synchronization and frame
    /// telemetry are deliberately unassigned, and the events they produce have nowhere to go
    /// until they are. Reported so a server can count them rather than lose them.
    Unassigned,
    /// The event did not fit the buffer offered.
    Encode(EncodeError),
}

impl From<EncodeError> for EmitError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

/// Write one event into `out`, returning how many bytes it occupies.
///
/// # Errors
///
/// Returns [`EmitError::Unassigned`] for an event the draw path has no message for, and
/// [`EmitError::Encode`] when the buffer is too small.
pub fn encode_event(event: &Event, sequence: u32, out: &mut [u8]) -> Result<usize, EmitError> {
    let object = event.object_id;
    match &event.kind {
        EventKind::Error(error) => {
            let payload = DisplayError {
                object_id: error.object_id(),
                code: WireErrorCode::from_raw(error.code() as u32),
                // The wire carries a string, and the compositor has a typed error. Sending the
                // code alone would be enough for a program and useless for a person reading a
                // log, so the name travels with it.
                message: error_name(*error),
            };
            emit(
                ObjectId::DISPLAY,
                Operation::DisplayError,
                sequence,
                &payload,
                out,
            )
        }
        EventKind::IdRetired => {
            let payload = NewId { new_id: object };
            emit(
                ObjectId::DISPLAY,
                Operation::DisplayDeleteId,
                sequence,
                &payload,
                out,
            )
        }
        EventKind::Global {
            name,
            interface,
            version,
        } => {
            let payload = RegistryGlobal {
                name: *name,
                // The interface's own name, from the table that defines it, so a client and the
                // compositor cannot disagree about what a global is called.
                interface: interface.name(),
                version: *version,
            };
            emit(object, Operation::RegistryGlobal, sequence, &payload, out)
        }
        EventKind::GlobalRemove { name } => {
            let payload = GlobalName { name: *name };
            emit(
                object,
                Operation::RegistryGlobalRemove,
                sequence,
                &payload,
                out,
            )
        }
        EventKind::BufferRelease => emit_empty(object, Operation::BufferRelease, sequence, out),
        EventKind::FrameDone { timestamp_ns } => {
            let payload = FrameDone {
                // A frame callback's serial is the client's, and the state machine does not yet
                // carry it. Zero until `surface.frame` is modelled with the serial it was asked
                // with.
                serial: 0,
                timestamp_ns: *timestamp_ns,
            };
            emit(object, Operation::SurfaceFrameDone, sequence, &payload, out)
        }
        // Assigned no message by the draw path: capability revocation and object destruction have
        // no event, and the rest belong to input, explicit synchronization and frame telemetry.
        EventKind::CapabilityRevoked(_)
        | EventKind::ObjectDestroyed(_)
        | EventKind::BufferReleaseWithFence { .. }
        | EventKind::FrameDeadline { .. }
        | EventKind::FrameLate
        | EventKind::KeyboardLeave
        | EventKind::KeyboardEnter => Err(EmitError::Unassigned),
    }
}

fn emit(
    object: ObjectId,
    operation: Operation,
    sequence: u32,
    payload: &impl EncodePayload,
    out: &mut [u8],
) -> Result<usize, EmitError> {
    let opcode = Opcode::from_raw(operation.opcode());
    Ok(encode_message(object, opcode, sequence, payload, out)?)
}

fn emit_empty(
    object: ObjectId,
    operation: Operation,
    sequence: u32,
    out: &mut [u8],
) -> Result<usize, EmitError> {
    struct Empty;
    impl EncodePayload for Empty {
        fn encoded_len(&self) -> Result<usize, EncodeError> {
            Ok(0)
        }
        fn encode(&self, _encoder: &mut Encoder<'_>) -> Result<(), EncodeError> {
            Ok(())
        }
    }
    emit(object, operation, sequence, &Empty, out)
}

/// A short name for an error, so a log says what happened rather than only a number.
const fn error_name(error: StateError) -> &'static str {
    use StateError as E;
    match error {
        E::InvalidObject { .. } => "invalid_object",
        E::InvalidObjectId { .. } => "invalid_object_id",
        E::ObjectIdAlreadyUsed { .. } => "object_id_already_used",
        E::WrongObjectType { .. } => "wrong_object_type",
        E::UnknownOpcode { .. } => "unknown_opcode",
        E::ConnectionClosed => "connection_closed",
        E::InvalidState { .. } => "invalid_state",
        E::UnsupportedCapability { .. } => "unsupported_capability",
        E::QuotaExceeded { .. } => "quota_exceeded",
        E::EventQueueOverflow { .. } => "event_queue_overflow",
    }
}
