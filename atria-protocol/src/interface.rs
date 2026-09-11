//! The interface table: the one place an operation's interface, opcode and payload are stated.
//!
//! Everything else derives from here. Opcodes are not written down beside the code that uses
//! them, message sizes are computed rather than asserted, and the decoder is driven by the same
//! table the specification is checked against. A number that appears in exactly one place cannot
//! drift from itself.
//!
//! Two invariants are enforced by the tests beside this module rather than by convention:
//! an operation's opcode is its position in its interface's list, which is §12.2's dense-from-zero
//! rule made mechanical; and every operation lists the interface and kind it is filed under, so a
//! table entry cannot be filed in one place and describe another.

use crate::wire::{HEADER_SIZE, HandleKind};
use crate::{DecodeError, Opcode};

/// One field of a payload, in wire order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    /// A connection-scoped object identifier. Zero is null.
    Object,
    /// A slot in the message's handle array, carrying what must be in it.
    Handle(HandleKind),
    U32,
    I32,
    U64,
    /// Length-prefixed UTF-8, padded to four bytes. Makes a message variable-length.
    String,
    /// A length-prefixed run of 32-bit values. Makes a message variable-length.
    Array,
}

impl Field {
    /// Bytes this field occupies, or `None` when it is variable-length.
    #[must_use]
    pub const fn size(self) -> Option<usize> {
        match self {
            Self::Object | Self::Handle(_) | Self::U32 | Self::I32 => Some(4),
            Self::U64 => Some(8),
            Self::String | Self::Array => None,
        }
    }
}

/// Which interface an operation belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interface {
    Display,
    Registry,
    Compositor,
    Shm,
    ShmPool,
    Buffer,
    Surface,
    Shell,
    Toplevel,
    Output,
    /// The authority a shell holds over every window, separate from the role factory an
    /// application uses. Two interfaces because they are two powers: an application gives its own
    /// surface window semantics, a shell arranges everyone's.
    ShellControl,
    /// A coherent input and routing domain. Not "the mouse and keyboard": a workstation with two
    /// people at it has two seats, and so does a television with a remote and a gamepad.
    Seat,
    /// One seat's pointing device, as the client it is over sees it.
    Pointer,
    /// One seat's keyboard, as the client holding focus sees it.
    Keyboard,
    /// Claiming key chords. Separate from the keyboard because it is a different power: being
    /// told that one chord fired, rather than seeing everything typed.
    Shortcuts,
}

/// Whether an operation travels from the client or to it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageKind {
    Method,
    Event,
}

/// Everything the wire needs to know about one operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationSpec {
    pub interface: Interface,
    pub kind: MessageKind,
    pub opcode: u16,
    pub name: &'static str,
    pub payload: &'static [Field],
}

impl OperationSpec {
    /// Total message size including the header, or `None` when the payload is variable-length.
    #[must_use]
    pub const fn message_size(&self) -> Option<usize> {
        let mut total = HEADER_SIZE;
        let mut index = 0;
        while index < self.payload.len() {
            match self.payload[index].size() {
                Some(size) => total += size,
                None => return None,
            }
            index += 1;
        }
        Some(total)
    }
}

/// Every operation of every interface, named for the interface it belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    DisplaySync,
    DisplayGetRegistry,
    DisplayError,
    DisplayDeleteId,
    DisplaySyncDone,

    RegistryBind,
    RegistryGlobal,
    RegistryGlobalRemove,

    CompositorCreateSurface,

    ShmCreatePool,
    ShmFormat,

    ShmPoolCreateBuffer,
    ShmPoolResize,
    ShmPoolDestroy,

    BufferDestroy,
    BufferRelease,

    SurfaceDestroy,
    SurfaceAttach,
    SurfaceDamageBuffer,
    SurfaceCommit,
    SurfaceFrame,
    SurfaceEnter,
    SurfaceLeave,
    SurfaceFrameDone,

    ShellGetToplevel,

    ToplevelDestroy,
    ToplevelSetTitle,
    ToplevelSetMinSize,
    ToplevelSetMaxSize,
    ToplevelConfigure,
    ToplevelClose,

    OutputIdentity,
    OutputGeometry,
    OutputMode,
    OutputScale,
    OutputDone,
    ShellControlConfigure,
    ShellControlPlace,
    ShellControlRaise,
    ShellControlFocus,
    ShellControlClose,
    ShellControlToplevel,
    ShellControlToplevelGone,
    ShellControlFocusChanged,
    ShellControlSnapshotDone,
    ShellControlInteraction,
    ShellControlGrab,
    ShellControlGrabMotion,
    ShellControlGrabEnd,
    SeatGetPointer,
    PointerDestroy,
    PointerEnter,
    PointerLeave,
    PointerMotion,
    PointerButton,
    PointerAxis,
    SeatGetKeyboard,
    KeyboardDestroy,
    KeyboardEnter,
    KeyboardLeave,
    KeyboardKey,
    KeyboardModifiers,
    ShortcutsRegister,
    ShortcutsUnregister,
    ShortcutsTriggered,
}

/// Shorthand for one table row.
const fn spec(
    interface: Interface,
    kind: MessageKind,
    opcode: u16,
    name: &'static str,
    payload: &'static [Field],
) -> OperationSpec {
    OperationSpec {
        interface,
        kind,
        opcode,
        name,
        payload,
    }
}

impl Operation {
    /// The table row for this operation. Exhaustive, so an operation cannot exist without one.
    #[must_use]
    pub const fn spec(self) -> OperationSpec {
        use Field::{Array, I32, Object, String, U32, U64};
        use Interface as I;
        use MessageKind::{Event, Method};

        match self {
            Self::DisplaySync => spec(I::Display, Method, 0, "sync", &[U32]),
            Self::DisplayGetRegistry => spec(I::Display, Method, 1, "get_registry", &[Object]),
            Self::DisplayError => spec(I::Display, Event, 0, "error", &[Object, U32, String]),
            Self::DisplayDeleteId => spec(I::Display, Event, 1, "delete_id", &[Object]),
            Self::DisplaySyncDone => spec(I::Display, Event, 2, "sync_done", &[U32]),

            Self::RegistryBind => spec(I::Registry, Method, 0, "bind", &[U32, U32, Object]),
            Self::RegistryGlobal => spec(I::Registry, Event, 0, "global", &[U32, String, U32]),
            Self::RegistryGlobalRemove => spec(I::Registry, Event, 1, "global_remove", &[U32]),

            Self::CompositorCreateSurface => {
                spec(I::Compositor, Method, 0, "create_surface", &[Object])
            }

            Self::ShmCreatePool => spec(
                I::Shm,
                Method,
                0,
                "create_pool",
                &[Object, Field::Handle(HandleKind::SharedMemory), U32],
            ),
            Self::ShmFormat => spec(I::Shm, Event, 0, "format", &[U32]),

            Self::ShmPoolCreateBuffer => spec(
                I::ShmPool,
                Method,
                0,
                "create_buffer",
                &[Object, U32, U32, U32, U32, U32],
            ),
            Self::ShmPoolResize => spec(I::ShmPool, Method, 1, "resize", &[U32]),
            Self::ShmPoolDestroy => spec(I::ShmPool, Method, 2, "destroy", &[]),

            Self::BufferDestroy => spec(I::Buffer, Method, 0, "destroy", &[]),
            Self::BufferRelease => spec(I::Buffer, Event, 0, "release", &[]),

            Self::SurfaceDestroy => spec(I::Surface, Method, 0, "destroy", &[]),
            Self::SurfaceAttach => spec(I::Surface, Method, 1, "attach", &[Object, I32, I32]),
            Self::SurfaceDamageBuffer => spec(
                I::Surface,
                Method,
                2,
                "damage_buffer",
                &[I32, I32, U32, U32],
            ),
            Self::SurfaceCommit => spec(I::Surface, Method, 3, "commit", &[U64, U32]),
            Self::SurfaceFrame => spec(I::Surface, Method, 4, "frame", &[U32]),
            Self::SurfaceEnter => spec(I::Surface, Event, 0, "enter", &[Object]),
            Self::SurfaceLeave => spec(I::Surface, Event, 1, "leave", &[Object]),
            Self::SurfaceFrameDone => spec(I::Surface, Event, 2, "frame_done", &[U32, U64]),

            Self::ShellGetToplevel => spec(I::Shell, Method, 0, "get_toplevel", &[Object, Object]),

            Self::ToplevelDestroy => spec(I::Toplevel, Method, 0, "destroy", &[]),
            Self::ToplevelSetTitle => spec(I::Toplevel, Method, 1, "set_title", &[String]),
            Self::ToplevelSetMinSize => spec(I::Toplevel, Method, 2, "set_min_size", &[U32, U32]),
            Self::ToplevelSetMaxSize => spec(I::Toplevel, Method, 3, "set_max_size", &[U32, U32]),
            Self::ToplevelConfigure => {
                spec(I::Toplevel, Event, 0, "configure", &[U32, U32, U32, U32])
            }
            Self::ToplevelClose => spec(I::Toplevel, Event, 1, "close", &[]),

            Self::OutputIdentity => spec(I::Output, Event, 0, "identity", &[U64, U64]),
            Self::OutputGeometry => {
                spec(I::Output, Event, 1, "geometry", &[I32, I32, U32, U32, U32])
            }
            Self::OutputMode => spec(I::Output, Event, 2, "mode", &[U32, U32, U32]),
            Self::OutputScale => spec(I::Output, Event, 3, "scale", &[U32, U32]),
            Self::OutputDone => spec(I::Output, Event, 4, "done", &[]),
            Self::ShellControlConfigure => spec(
                I::ShellControl,
                Method,
                0,
                "configure",
                &[U64, U32, U32, U32],
            ),
            Self::ShellControlPlace => spec(I::ShellControl, Method, 1, "place", &[U64, I32, I32]),
            Self::ShellControlRaise => spec(I::ShellControl, Method, 2, "raise", &[U64]),
            Self::ShellControlFocus => spec(I::ShellControl, Method, 3, "focus", &[U64, U64]),
            Self::ShellControlClose => spec(I::ShellControl, Method, 4, "close", &[U64]),
            // One event for a window the shell is being told about, whether it existed before the
            // shell attached or appeared after. The snapshot boundary is what separates those, so
            // a second event carrying the same fields would say nothing the boundary does not.
            Self::ShellControlToplevel => {
                spec(I::ShellControl, Event, 0, "toplevel", &[U64, String])
            }
            Self::ShellControlToplevelGone => {
                spec(I::ShellControl, Event, 1, "toplevel_gone", &[U64])
            }
            // Focus is a seat's, not the system's. Two people at one machine hold two focuses,
            // and a model with one would have to be replaced rather than extended.
            Self::ShellControlFocusChanged => {
                spec(I::ShellControl, Event, 2, "focus_changed", &[U64, U64])
            }
            Self::ShellControlSnapshotDone => spec(I::ShellControl, Event, 3, "snapshot_done", &[]),
            // What the person did, not what they typed. A shell implementing click-to-focus needs
            // to know a window was pressed; it does not need to see the characters going into it.
            Self::ShellControlInteraction => spec(
                I::ShellControl,
                Event,
                4,
                "interaction",
                &[U64, U64, U32, U32, I32, I32],
            ),
            // Where the pointer went while the shell held it. Sent only to a shell that asked,
            // and only until the button comes up: a shell that could watch the pointer whenever
            // it liked would be global input observation under another name.
            Self::ShellControlGrabMotion => {
                spec(I::ShellControl, Event, 5, "grab_motion", &[U64, I32, I32])
            }
            Self::ShellControlGrabEnd => spec(I::ShellControl, Event, 6, "grab_end", &[U64]),
            Self::ShellControlGrab => spec(I::ShellControl, Method, 5, "grab", &[U64, U64]),
            Self::SeatGetPointer => spec(I::Seat, Method, 0, "get_pointer", &[Object]),
            Self::PointerDestroy => spec(I::Pointer, Method, 0, "destroy", &[]),
            // Every pointer event carries the epoch its routing belonged to. When continuity
            // breaks — a device leaves, a grab is cancelled, routing is rebuilt — the epoch
            // advances, and a client can tell a stale event from a current one rather than
            // treating an old release as the state of the world.
            Self::PointerEnter => {
                spec(I::Pointer, Event, 0, "enter", &[U32, Object, I32, I32, U32])
            }
            Self::PointerLeave => spec(I::Pointer, Event, 1, "leave", &[U32, Object, U32]),
            Self::PointerMotion => spec(I::Pointer, Event, 2, "motion", &[U64, I32, I32, U32]),
            Self::PointerButton => spec(I::Pointer, Event, 3, "button", &[U32, U64, U32, U32, U32]),
            Self::PointerAxis => spec(I::Pointer, Event, 4, "axis", &[U64, U32, I32, U32]),
            Self::SeatGetKeyboard => spec(I::Seat, Method, 1, "get_keyboard", &[Object]),
            Self::KeyboardDestroy => spec(I::Keyboard, Method, 0, "destroy", &[]),
            // Focus carries what is already held down. A client given focus mid-chord and told
            // only about later releases would believe keys were up that are not.
            // Carries the keys already held: a client given focus mid-chord and told only about
            // later releases would believe keys were up that are not.
            Self::KeyboardEnter => spec(
                I::Keyboard,
                Event,
                0,
                "enter",
                &[U32, Object, U32, U32, Array],
            ),
            Self::KeyboardLeave => spec(I::Keyboard, Event, 1, "leave", &[U32, Object, U32]),
            // The key is a physical position, not a character. What a position means depends on a
            // layout the compositor does not own, and text arrives by its own route.
            Self::KeyboardKey => spec(I::Keyboard, Event, 2, "key", &[U32, U64, U32, U32, U32]),
            Self::KeyboardModifiers => spec(
                I::Keyboard,
                Event,
                3,
                "modifiers",
                &[U32, U32, U32, U32, U32],
            ),
            // A chord is a physical position and the modifiers held with it. The holder names the
            // chord; it is never told about keys it did not name.
            Self::ShortcutsRegister => spec(
                I::Shortcuts,
                Method,
                0,
                "register",
                &[U32, U64, U32, U32, U32],
            ),
            Self::ShortcutsUnregister => spec(I::Shortcuts, Method, 1, "unregister", &[U32]),
            Self::ShortcutsTriggered => spec(
                I::Shortcuts,
                Event,
                0,
                "triggered",
                &[U32, U64, U32, U64, U32],
            ),
        }
    }

    #[must_use]
    pub const fn interface(self) -> Interface {
        self.spec().interface
    }

    #[must_use]
    pub const fn kind(self) -> MessageKind {
        self.spec().kind
    }

    #[must_use]
    pub const fn opcode(self) -> u16 {
        self.spec().opcode
    }
}

impl Interface {
    /// The interface's name on the wire, as the registry advertises it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Display => "atria_display",
            Self::Registry => "atria_registry",
            Self::Compositor => "atria_compositor",
            Self::Shm => "atria_shm",
            Self::ShmPool => "atria_shm_pool",
            Self::Buffer => "atria_buffer",
            Self::Surface => "atria_surface",
            Self::Shell => "atria_shell",
            Self::Toplevel => "atria_toplevel",
            Self::Output => "atria_output",
            Self::ShellControl => "atria_shell_control",
            Self::Seat => "atria_seat",
            Self::Pointer => "atria_pointer",
            Self::Keyboard => "atria_keyboard",
            Self::Shortcuts => "atria_shortcuts",
        }
    }

    /// This interface's methods, in opcode order.
    #[must_use]
    pub const fn methods(self) -> &'static [Operation] {
        use Operation as O;
        match self {
            Self::Display => &[O::DisplaySync, O::DisplayGetRegistry],
            Self::Registry => &[O::RegistryBind],
            Self::Compositor => &[O::CompositorCreateSurface],
            Self::Shm => &[O::ShmCreatePool],
            Self::ShmPool => &[O::ShmPoolCreateBuffer, O::ShmPoolResize, O::ShmPoolDestroy],
            Self::Buffer => &[O::BufferDestroy],
            Self::Surface => &[
                O::SurfaceDestroy,
                O::SurfaceAttach,
                O::SurfaceDamageBuffer,
                O::SurfaceCommit,
                O::SurfaceFrame,
            ],
            Self::Shell => &[O::ShellGetToplevel],
            Self::Toplevel => &[
                O::ToplevelDestroy,
                O::ToplevelSetTitle,
                O::ToplevelSetMinSize,
                O::ToplevelSetMaxSize,
            ],
            Self::Output => &[],
            Self::Seat => &[O::SeatGetPointer, O::SeatGetKeyboard],
            Self::Pointer => &[O::PointerDestroy],
            Self::Keyboard => &[O::KeyboardDestroy],
            Self::Shortcuts => &[O::ShortcutsRegister, O::ShortcutsUnregister],
            Self::ShellControl => &[
                O::ShellControlConfigure,
                O::ShellControlPlace,
                O::ShellControlRaise,
                O::ShellControlFocus,
                O::ShellControlClose,
                O::ShellControlGrab,
            ],
        }
    }

    /// This interface's events, in opcode order.
    #[must_use]
    pub const fn events(self) -> &'static [Operation] {
        use Operation as O;
        match self {
            Self::Display => &[O::DisplayError, O::DisplayDeleteId, O::DisplaySyncDone],
            Self::Registry => &[O::RegistryGlobal, O::RegistryGlobalRemove],
            Self::Compositor | Self::ShmPool | Self::Shell => &[],
            Self::Shm => &[O::ShmFormat],
            Self::Buffer => &[O::BufferRelease],
            Self::Surface => &[O::SurfaceEnter, O::SurfaceLeave, O::SurfaceFrameDone],
            Self::Toplevel => &[O::ToplevelConfigure, O::ToplevelClose],
            Self::Seat => &[],
            Self::Shortcuts => &[O::ShortcutsTriggered],
            Self::Keyboard => &[
                O::KeyboardEnter,
                O::KeyboardLeave,
                O::KeyboardKey,
                O::KeyboardModifiers,
            ],
            Self::Pointer => &[
                O::PointerEnter,
                O::PointerLeave,
                O::PointerMotion,
                O::PointerButton,
                O::PointerAxis,
            ],
            Self::ShellControl => &[
                O::ShellControlToplevel,
                O::ShellControlToplevelGone,
                O::ShellControlFocusChanged,
                O::ShellControlSnapshotDone,
                O::ShellControlInteraction,
                O::ShellControlGrabMotion,
                O::ShellControlGrabEnd,
            ],
            Self::Output => &[
                O::OutputIdentity,
                O::OutputGeometry,
                O::OutputMode,
                O::OutputScale,
                O::OutputDone,
            ],
        }
    }
}

/// The operation an interface defines at this opcode, for this direction.
///
/// Derived from the table, so an opcode is decodable exactly when the table lists it: there is
/// no second place where a number could be recognised or forgotten.
pub fn decode_operation(
    interface: Interface,
    kind: MessageKind,
    opcode: Opcode,
) -> Result<Operation, DecodeError> {
    let list = match kind {
        MessageKind::Method => interface.methods(),
        MessageKind::Event => interface.events(),
    };
    list.get(usize::from(opcode.into_raw()))
        .copied()
        .ok_or(DecodeError::UnknownOpcode {
            interface,
            kind,
            opcode,
        })
}

/// Every interface the draw path defines.
pub const INTERFACES: &[Interface] = &[
    Interface::Display,
    Interface::Registry,
    Interface::Compositor,
    Interface::Shm,
    Interface::ShmPool,
    Interface::Buffer,
    Interface::Surface,
    Interface::Shell,
    Interface::Toplevel,
    Interface::Output,
    Interface::ShellControl,
    Interface::Seat,
    Interface::Pointer,
    Interface::Keyboard,
    Interface::Shortcuts,
];

/// The states a toplevel may be told it is in.
///
/// A closed set, and which bits exist is a property of the bound interface version rather than a
/// band reserved forever — §12.4. A later version widens the mask, and a peer bound at an earlier
/// one never receives the wider value because §12.5 gates the operation carrying it.
pub mod toplevel_state {
    pub const ACTIVATED: u32 = 1 << 0;
    pub const MAXIMIZED: u32 = 1 << 1;
    pub const FULLSCREEN: u32 = 1 << 2;
    pub const RESIZING: u32 = 1 << 3;
    pub const SUSPENDED: u32 = 1 << 4;
    pub const TILED_LEFT: u32 = 1 << 5;
    pub const TILED_RIGHT: u32 = 1 << 6;
    pub const TILED_TOP: u32 = 1 << 7;
    pub const TILED_BOTTOM: u32 = 1 << 8;

    /// Bits `atria_toplevel` version 1 defines. A value with any other bit set is refused.
    pub const VALID_V1: u32 = ACTIVATED
        | MAXIMIZED
        | FULLSCREEN
        | RESIZING
        | SUSPENDED
        | TILED_LEFT
        | TILED_RIGHT
        | TILED_TOP
        | TILED_BOTTOM;

    /// Bits defined by a given interface version.
    ///
    /// A function rather than one constant, because the rule is "bits not defined by the bound
    /// version must be zero" and a caller has to be able to ask for the version it bound.
    #[must_use]
    pub const fn valid_mask(version: u32) -> u32 {
        match version {
            0 => 0,
            _ => VALID_V1,
        }
    }
}
