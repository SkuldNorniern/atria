use alloc::vec::Vec;

use alloc::string::String;
use atria_protocol::ObjectId;
use atria_protocol::capability::Capability;

use atria_protocol::interface::Interface;
use atria_protocol::key::{Modifiers, PhysicalKey};
use atria_protocol::message::MAX_TITLE_BYTES;
use atria_protocol::opcode::Opcode;

use crate::output::IdentitySource;
use crate::resolve::SharedMemory;
use crate::shell::ToplevelHandle;

/// An input and routing domain. Not "the mouse and keyboard": focus belongs to a seat, so one
/// machine with two people at it has two.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SeatId(pub u64);

/// What a person did to a window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionKind {
    /// A pointer button went down over it. This is what click-to-focus is built from.
    PointerPress,
}

impl InteractionKind {
    /// The wire's number for this kind.
    #[must_use]
    pub const fn into_raw(self) -> u32 {
        match self {
            Self::PointerPress => 0,
        }
    }
}

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
    /// The surface factory a client binds from the registry.
    Compositor,
    /// The shared-memory factory a client binds from the registry.
    Shm,
    /// The window role factory a client binds from the registry.
    Shell,
    /// A surface given window semantics.
    Toplevel,
    ShmPool,
    Seat,
    Session,
    Surface,
    Buffer,
    Fence,
    InputStream,
    /// The authority to arrange every window, bound from the registry by a shell.
    ShellControl,
    /// One seat's pointing device, as a client sees it.
    Pointer,
    /// One seat's keyboard, as the client holding focus sees it.
    Keyboard,
    /// A display, bound from the registry. One global per display, because a client learns which
    /// displays exist the same way it learns everything else exists.
    Output,
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

/// A window title, bounded at the length the protocol permits.
///
/// A newtype rather than a `String` so the bound is enforced where the value is made, not wherever
/// a caller remembers to check. A title is text the compositor retains for as long as the window
/// exists, which makes it a resource a client can grow.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TitleText(String);

impl TitleText {
    /// Take a title, refusing one longer than the protocol permits.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        (text.len() <= MAX_TITLE_BYTES).then(|| Self(String::from(text)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Shared-memory pixel formats the draft names, and the only values `create_buffer` accepts.
///
/// Numbered here rather than inferred from a bytes-per-pixel count: a format is a layout and a
/// colour interpretation, and two formats of one width are not interchangeable.
pub const FORMAT_XRGB8888: u32 = 0;
pub const FORMAT_ARGB8888: u32 = 1;
pub const FORMAT_RGB565: u32 = 2;

/// Whether the compositor knows what this format means.
///
/// An unknown value is refused rather than defaulted, per §12.4: guessing a layout would let a
/// client and the compositor disagree about what the same bytes are.
#[must_use]
pub const fn pixel_format_is_known(format: u32) -> bool {
    matches!(format, FORMAT_XRGB8888 | FORMAT_ARGB8888 | FORMAT_RGB565)
}

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
    /// Take a global the registry advertised, at an identifier the client chooses.
    Bind {
        name: u32,
        version: u32,
        new_id: ObjectId,
    },
    /// Adopt a client's shared memory, already resolved from the handle slot that named it.
    CreatePool {
        new_id: ObjectId,
        memory: SharedMemory,
    },
    /// Carve a buffer out of a pool the connection already holds.
    CreateBuffer {
        new_id: ObjectId,
        pool: ObjectId,
        offset: u32,
        size: Size,
        stride: u32,
        format: u32,
    },
    /// The session is the connection's, not the request's: the wire creates a surface through
    /// the compositor global, which names none.
    CreateSurface {
        new_id: ObjectId,
    },
    ImportBuffer {
        new_id: ObjectId,
        descriptor: BufferDescriptor,
    },
    /// A shell asking a window to adopt a size and states. The handle is compositor-wide, because
    /// a shell has never seen the object identifiers of the clients it arranges.
    ShellConfigure {
        control: ObjectId,
        handle: ToplevelHandle,
        size: Size,
        state: u32,
    },
    ShellPlace {
        control: ObjectId,
        handle: ToplevelHandle,
        position: Point,
    },
    ShellRaise {
        control: ObjectId,
        handle: ToplevelHandle,
    },
    ShellFocus {
        control: ObjectId,
        seat: SeatId,
        handle: ToplevelHandle,
    },
    ShellClose {
        control: ObjectId,
        handle: ToplevelHandle,
    },
    /// A shell asking to hold the pointer until the button that started it comes up. Bounded by
    /// the button: holding it indefinitely would be watching everything a person did.
    ShellGrab {
        control: ObjectId,
        seat: SeatId,
        handle: ToplevelHandle,
    },
    /// Ask a seat for its pointing device.
    GetPointer {
        seat: ObjectId,
        new_id: ObjectId,
    },
    /// Ask a seat for its keyboard.
    GetKeyboard {
        seat: ObjectId,
        new_id: ObjectId,
    },
    /// Give a surface window semantics. A surface may take one role.
    GetToplevel {
        surface: ObjectId,
        new_id: ObjectId,
    },
    SetTitle {
        toplevel: ObjectId,
        title: TitleText,
    },
    SetMinSize {
        toplevel: ObjectId,
        size: Size,
    },
    SetMaxSize {
        toplevel: ObjectId,
        size: Size,
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
    /// Ask to be told when the next frame is presented. The serial is the client's own, echoed
    /// back in `frame_done`, so a client with several outstanding requests knows which answered.
    RequestFrame {
        surface: ObjectId,
        serial: u32,
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
    /// The identifier is retired and the client may allocate it again.
    IdRetired,
    /// The compositor asks a toplevel to adopt a size and a set of states.
    Configure {
        serial: u32,
        size: Size,
        state: u32,
    },
    /// The compositor asks a toplevel to go away. A request, not an instruction.
    Close,
    /// A global exists, and what interface and version it offers.
    Global {
        name: u32,
        interface: Interface,
        version: u32,
    },
    /// A global has gone. Objects bound from it are inert.
    GlobalRemove {
        name: u32,
    },
    BufferRelease,
    BufferReleaseWithFence {
        fence: ObjectId,
    },
    FrameDone {
        serial: u32,
        timestamp_ns: u64,
    },
    FrameDeadline {
        deadline_ns: u64,
        refresh_interval_ns: u64,
    },
    FrameLate,
    KeyboardLeave,
    KeyboardEnter,
    /// This surface now has the seat's keys, and these are already held.
    KeyFocusGained {
        serial: u32,
        surface: ObjectId,
        modifiers: Modifiers,
        epoch: u32,
        held: Vec<PhysicalKey>,
    },
    /// This surface no longer has the seat's keys.
    KeyFocusLost {
        serial: u32,
        surface: ObjectId,
        epoch: u32,
    },
    /// A key changed state, named by its physical position.
    Key {
        serial: u32,
        time_ns: u64,
        key: PhysicalKey,
        pressed: bool,
        epoch: u32,
    },
    /// Which modifiers are held.
    KeyModifiers {
        modifiers: Modifiers,
        epoch: u32,
    },
    /// The pointer came over this surface, at a point inside it.
    PointerEnter {
        serial: u32,
        surface: ObjectId,
        position: Point,
        epoch: u32,
    },
    /// The pointer is no longer over this surface.
    PointerLeave {
        serial: u32,
        surface: ObjectId,
        epoch: u32,
    },
    PointerMotion {
        time_ns: u64,
        position: Point,
        epoch: u32,
    },
    PointerButton {
        serial: u32,
        time_ns: u64,
        button: u32,
        pressed: bool,
        epoch: u32,
    },
    /// A person did something to a window. Not the input itself: a shell needs to know a window
    /// was pressed, not what is typed into it.
    ShellInteraction {
        seat: SeatId,
        handle: ToplevelHandle,
        serial: u32,
        kind: InteractionKind,
        /// Where in the window it happened, in the window's own coordinates.
        position: Point,
    },
    /// Where the pointer went while the shell held it.
    ShellGrabMotion {
        seat: SeatId,
        position: Point,
    },
    /// The shell no longer holds the pointer.
    ShellGrabEnd {
        seat: SeatId,
    },
    /// Which display a bound output object names.
    OutputIdentity {
        identity: u128,
    },
    /// Where the display sits, how big it physically is, and how its identity was derived.
    OutputGeometry {
        position: Point,
        physical_millimetres: Size,
        identity_source: IdentitySource,
    },
    /// The resolution and refresh the display is running.
    OutputMode {
        size: Size,
        refresh_millihertz: u32,
    },
    /// The display's scale as an exact ratio.
    OutputScale {
        numerator: u32,
        denominator: u32,
    },
    /// A window a shell is told about. The same event before and after the snapshot boundary,
    /// which is what separates "already there" from "just appeared".
    ShellToplevel {
        handle: ToplevelHandle,
        title: String,
    },
    /// A window a shell was told about has gone.
    ShellToplevelGone {
        handle: ToplevelHandle,
    },
    /// Keyboard focus moved on a seat. A null handle means nothing on that seat holds it.
    ShellFocusChanged {
        seat: SeatId,
        handle: ToplevelHandle,
    },
    /// Everything before this is the state as it stood when the shell attached.
    ShellSnapshotDone,
    /// Everything above describes one consistent state of the display.
    OutputDone,
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
