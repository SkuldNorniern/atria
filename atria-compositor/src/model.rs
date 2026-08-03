use alloc::vec::Vec;

use atria_protocol::ObjectId;
use atria_protocol::capability::Capability;
use atria_protocol::opcode::Opcode;

use crate::error::StateError;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConnectionId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SurfaceKey {
    pub connection: ConnectionId,
    pub object_id: ObjectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Display,
    Registry,
    Seat,
    Session,
    Surface,
    Buffer,
    Fence,
    InputStream,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        let Some(right) = i64::from(self.x).checked_add(i64::from(self.width)) else {
            return false;
        };
        let Some(bottom) = i64::from(self.y).checked_add(i64::from(self.height)) else {
            return false;
        };
        let x = i64::from(point.x);
        let y = i64::from(point.y);
        x >= i64::from(self.x) && x < right && y >= i64::from(self.y) && y < bottom
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Damage {
    /// No damage request was supplied. Draft §7 says damage SHOULD be supplied, so this is
    /// valid and conservatively means the full attached buffer.
    Full,
    Rect(Rect),
}

/// The draft says a surface has a role but does not define role names or their encoding.
/// An opaque value preserves single-assignment semantics without inventing policy roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceRole(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferTransport {
    SoftwareShm,
    GpuPrime,
}

impl BufferTransport {
    #[must_use]
    pub const fn capability(self) -> Capability {
        match self {
            Self::SoftwareShm => Capability::SoftwareShm,
            Self::GpuPrime => Capability::GpuPrime,
        }
    }
}

/// Metadata retained after a backend has validated the passed handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferDescriptor {
    pub transport: BufferTransport,
    pub size: Size,
    pub stride: u32,
    pub byte_len: u64,
}

impl BufferDescriptor {
    #[must_use]
    pub fn is_structurally_valid(self) -> bool {
        if self.size.width == 0 || self.size.height == 0 || self.stride == 0 {
            return false;
        }
        u64::from(self.stride)
            .checked_mul(u64::from(self.size.height))
            .is_some_and(|minimum| minimum <= self.byte_len)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferState {
    Available,
    Pending {
        surface: SurfaceKey,
    },
    CompositorHeld {
        surface: SurfaceKey,
        commit: CommitId,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SeatCapabilities(u8);

impl SeatCapabilities {
    const POINTER: u8 = 1;
    const KEYBOARD: u8 = 2;
    const TOUCH: u8 = 4;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn with_pointer(self) -> Self {
        Self(self.0 | Self::POINTER)
    }

    #[must_use]
    pub const fn with_keyboard(self) -> Self {
        Self(self.0 | Self::KEYBOARD)
    }

    #[must_use]
    pub const fn with_touch(self) -> Self {
        Self(self.0 | Self::TOUCH)
    }

    #[must_use]
    pub const fn has_pointer(self) -> bool {
        self.0 & Self::POINTER != 0
    }

    #[must_use]
    pub const fn has_keyboard(self) -> bool {
        self.0 & Self::KEYBOARD != 0
    }

    #[must_use]
    pub const fn has_touch(self) -> bool {
        self.0 & Self::TOUCH != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeatSnapshot {
    pub capabilities: SeatCapabilities,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub seat: Option<ObjectId>,
    pub active: bool,
}

/// A request after wire framing and payload decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientRequest {
    CreateRegistry {
        new_id: ObjectId,
    },
    CreateSurface {
        session: ObjectId,
        new_id: ObjectId,
    },
    ImportBuffer {
        new_id: ObjectId,
        descriptor: BufferDescriptor,
    },
    Attach {
        surface: ObjectId,
        buffer: ObjectId,
        offset: Point,
        acquire_fence: Option<ObjectId>,
    },
    Damage {
        surface: ObjectId,
        rect: Rect,
    },
    Commit {
        surface: ObjectId,
    },
    SetRole {
        surface: ObjectId,
        role: SurfaceRole,
    },
    RequestFrame {
        surface: ObjectId,
    },
    SetRefreshRange {
        surface: ObjectId,
        min_hz: u32,
        max_hz: u32,
    },
    Destroy {
        object: ObjectId,
    },
    Unknown {
        object: ObjectId,
        opcode: Opcode,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventKind {
    Error(StateError),
    CapabilityRevoked(Capability),
    ObjectDestroyed(ObjectKind),
    BufferRelease,
    BufferReleaseWithFence {
        fence: ObjectId,
    },
    FrameDone {
        timestamp_ns: u64,
    },
    FrameDeadline {
        deadline_ns: u64,
        refresh_interval_ns: u64,
    },
    FrameLate,
    KeyboardLeave,
    KeyboardEnter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub connection: ConnectionId,
    pub object_id: ObjectId,
    pub kind: EventKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusEvent {
    KeyboardLeave(SurfaceKey),
    KeyboardEnter(SurfaceKey),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceSnapshot {
    pub role: Option<SurfaceRole>,
    pub buffer: ObjectId,
    pub offset: Point,
    pub damage: Vec<Damage>,
    pub commit: CommitId,
    pub refresh_range: Option<(u32, u32)>,
    pub acquire_fence: Option<ObjectId>,
}
