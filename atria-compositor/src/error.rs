use core::error::Error;
use core::fmt;

use atria_protocol::ObjectId;
use atria_protocol::capability::Capability;
use atria_protocol::error::ErrorCategory;
use atria_protocol::opcode::Opcode;

use crate::model::{ObjectKind, SurfaceKey};

/// Codes chosen within the category ranges reserved by draft §10.
///
/// The draft assigns only `INVALID_OBJECT` (0x0001). The remaining individual values are
/// unspecified; these stable local assignments use the correct normative ranges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ErrorCode {
    InvalidObject = 0x0001,
    InvalidObjectId = 0x0002,
    ObjectIdAlreadyUsed = 0x0003,
    WrongObjectType = 0x0004,
    UnknownOpcode = 0x0005,
    ConnectionClosed = 0x0006,
    InvalidState = 0x0100,
    UnsupportedCapability = 0x0101,
    QuotaExceeded = 0x0200,
    EventQueueOverflow = 0x0201,
}

impl ErrorCode {
    #[must_use]
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::InvalidObject
            | Self::InvalidObjectId
            | Self::ObjectIdAlreadyUsed
            | Self::WrongObjectType
            | Self::UnknownOpcode
            | Self::ConnectionClosed => ErrorCategory::Protocol,
            Self::InvalidState | Self::UnsupportedCapability => ErrorCategory::Object,
            Self::QuotaExceeded | Self::EventQueueOverflow => ErrorCategory::Resource,
        }
    }
}

/// A checked failure caused by an untrusted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateError {
    InvalidObject {
        object_id: ObjectId,
    },
    InvalidObjectId {
        object_id: ObjectId,
    },
    ObjectIdAlreadyUsed {
        object_id: ObjectId,
    },
    WrongObjectType {
        object_id: ObjectId,
        expected: ObjectKind,
        actual: ObjectKind,
    },
    UnknownOpcode {
        object_id: ObjectId,
        opcode: Opcode,
    },
    ConnectionClosed,
    InvalidState {
        object_id: ObjectId,
    },
    UnsupportedCapability {
        object_id: ObjectId,
        capability: Capability,
    },
    QuotaExceeded {
        object_id: ObjectId,
    },
    /// The connection has more events queued than it may hold, so it is not reading them.
    EventQueueOverflow {
        queued: usize,
    },
}

impl StateError {
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::InvalidObject { .. } => ErrorCode::InvalidObject,
            Self::InvalidObjectId { .. } => ErrorCode::InvalidObjectId,
            Self::ObjectIdAlreadyUsed { .. } => ErrorCode::ObjectIdAlreadyUsed,
            Self::WrongObjectType { .. } => ErrorCode::WrongObjectType,
            Self::UnknownOpcode { .. } => ErrorCode::UnknownOpcode,
            Self::ConnectionClosed => ErrorCode::ConnectionClosed,
            Self::InvalidState { .. } => ErrorCode::InvalidState,
            Self::UnsupportedCapability { .. } => ErrorCode::UnsupportedCapability,
            Self::QuotaExceeded { .. } => ErrorCode::QuotaExceeded,
            Self::EventQueueOverflow { .. } => ErrorCode::EventQueueOverflow,
        }
    }

    #[must_use]
    pub const fn category(self) -> ErrorCategory {
        self.code().category()
    }

    #[must_use]
    pub const fn object_id(self) -> ObjectId {
        match self {
            Self::InvalidObject { object_id }
            | Self::InvalidObjectId { object_id }
            | Self::ObjectIdAlreadyUsed { object_id }
            | Self::WrongObjectType { object_id, .. }
            | Self::UnknownOpcode { object_id, .. }
            | Self::InvalidState { object_id }
            | Self::UnsupportedCapability { object_id, .. }
            | Self::QuotaExceeded { object_id } => object_id,
            Self::ConnectionClosed | Self::EventQueueOverflow { .. } => ObjectId::DISPLAY,
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidObject { object_id } => write!(
                formatter,
                "object {} does not exist on this connection",
                object_id.into_raw()
            ),
            Self::InvalidObjectId { object_id } => write!(
                formatter,
                "object ID {} is reserved and cannot be allocated by a client",
                object_id.into_raw()
            ),
            Self::ObjectIdAlreadyUsed { object_id } => write!(
                formatter,
                "object ID {} is already in use on this connection",
                object_id.into_raw()
            ),
            Self::WrongObjectType {
                object_id,
                expected,
                actual,
            } => write!(
                formatter,
                "object {} is {actual:?}, but this operation requires {expected:?}",
                object_id.into_raw()
            ),
            Self::UnknownOpcode { object_id, opcode } => write!(
                formatter,
                "object {} received undefined opcode {:#06x}",
                object_id.into_raw(),
                opcode.into_raw()
            ),
            Self::ConnectionClosed => formatter.write_str("the compositor connection is closed"),
            Self::InvalidState { object_id } => write!(
                formatter,
                "object {} is not in a state that permits this operation",
                object_id.into_raw()
            ),
            Self::UnsupportedCapability {
                object_id,
                capability,
            } => write!(
                formatter,
                "object {} requires capability `{}`, which was not negotiated",
                object_id.into_raw(),
                capability.name()
            ),
            Self::QuotaExceeded { object_id } => write!(
                formatter,
                "creating object {} would exceed this connection's resource quota",
                object_id.into_raw()
            ),
            Self::EventQueueOverflow { queued } => write!(
                formatter,
                "the connection has {queued} events queued and is not reading them"
            ),
        }
    }
}

impl Error for StateError {}

/// A disagreement between the scene and the objects it names.
///
/// None of these can happen through the protocol: every path that removes a connection, an object
/// or a surface's content is supposed to take the scene entry with it. They exist so that a
/// compositor which has got into one of these states says which one, instead of failing later as
/// a frame that cannot be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneFault {
    /// The scene names a surface on a connection that has gone.
    ConnectionGone(SurfaceKey),
    /// The scene names an object the connection no longer holds.
    ObjectGone(SurfaceKey),
    /// The scene names an object that is not a surface.
    NotASurface(SurfaceKey),
    /// The scene holds a surface that has nothing to show.
    NoContent(SurfaceKey),
    /// The scene stacks a surface it has nowhere to put.
    NoPosition(SurfaceKey),
}

impl fmt::Display for SceneFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionGone(key) => {
                write!(
                    formatter,
                    "{key:?} is in the scene and its connection is gone"
                )
            }
            Self::ObjectGone(key) => write!(formatter, "{key:?} is in the scene and destroyed"),
            Self::NotASurface(key) => {
                write!(formatter, "{key:?} is in the scene and not a surface")
            }
            Self::NoContent(key) => write!(formatter, "{key:?} is in the scene with no content"),
            Self::NoPosition(key) => write!(formatter, "{key:?} is stacked with no position"),
        }
    }
}
