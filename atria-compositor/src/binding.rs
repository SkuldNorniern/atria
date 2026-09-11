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
    Attach, Bind, Commit, CommitText, CreateBuffer, CreatePool, CursorArea, DamageBuffer,
    GetRegistry, GetToplevel, GlobalName, NewId, Preedit, SeatHandle, SetTitle, ShellConfigure,
    ShellHandle, ShellPlace, ShortcutName, ShortcutRegistration, SizeHint, TextPurpose,
};
use atria_protocol::wire::{Frame, HandleIndex};
use atria_protocol::{DecodeError, ObjectId, Opcode};

use atria_protocol::message::{MAX_TEXT_BYTES, MAX_TITLE_BYTES};

use crate::model::{
    Chord, ChordMatch, ClientRequest, ObjectKind, Point, Rect, SeatId, Size, TextBuffer, TitleText,
};
use crate::resolve::{HandleResolver, ResolveError, SharedMemory};
use atria_protocol::key::PhysicalKey;

use crate::shell::ToplevelHandle;

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
        ObjectKind::Shell => Some(Interface::Shell),
        ObjectKind::Toplevel => Some(Interface::Toplevel),
        ObjectKind::ShmPool => Some(Interface::ShmPool),
        ObjectKind::Output => Some(Interface::Output),
        ObjectKind::ShellControl => Some(Interface::ShellControl),
        ObjectKind::Pointer => Some(Interface::Pointer),
        ObjectKind::Keyboard => Some(Interface::Keyboard),
        ObjectKind::Shortcuts => Some(Interface::Shortcuts),
        ObjectKind::TextInput => Some(Interface::TextInput),
        ObjectKind::InputMethod => Some(Interface::InputMethod),
        ObjectKind::Seat => Some(Interface::Seat),
        ObjectKind::Session | ObjectKind::Fence | ObjectKind::InputStream => None,
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
pub enum DecodedRequest<'a> {
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
    GetToplevel {
        surface: ObjectId,
        new_id: ObjectId,
    },
    /// Borrowed from the frame, so decoding allocates nothing. Resolution is what copies it into
    /// the bounded title the compositor keeps.
    SetTitle {
        toplevel: ObjectId,
        title: &'a str,
    },
    GetPointer {
        seat: ObjectId,
        new_id: ObjectId,
    },
    GetKeyboard {
        seat: ObjectId,
        new_id: ObjectId,
    },
    RegisterShortcut {
        manager: ObjectId,
        shortcut: u32,
        seat: SeatId,
        chord: Chord,
    },
    UnregisterShortcut {
        manager: ObjectId,
        shortcut: u32,
    },
    EnableText {
        text_input: ObjectId,
        purpose: TextPurpose,
    },
    DisableText {
        text_input: ObjectId,
    },
    SetCursorArea {
        text_input: ObjectId,
        area: Rect,
    },
    SetPreedit {
        method: ObjectId,
        text: &'a str,
        cursor_begin: i32,
        cursor_end: i32,
    },
    CommitText {
        method: ObjectId,
        text: &'a str,
    },
    TextDone {
        method: ObjectId,
        serial: u32,
    },
    /// A shell's request, carrying a compositor-wide handle rather than an object it could name.
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
    ShellGrab {
        control: ObjectId,
        seat: SeatId,
        handle: ToplevelHandle,
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
    },
    Damage {
        surface: ObjectId,
        rect: Rect,
    },
    Commit {
        surface: ObjectId,
    },
    RequestFrame {
        surface: ObjectId,
        serial: u32,
    },
    Destroy {
        object: ObjectId,
    },
}

/// Read one frame addressed to an object of `kind`. Performs no I/O and consults no transport.
///
/// The caller supplies the kind because object identity is connection state, which this function
/// deliberately does not hold.
pub fn decode<'a>(kind: ObjectKind, frame: &Frame<'a>) -> Result<DecodedRequest<'a>, BindError> {
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
        Operation::ShellGetToplevel => {
            let payload = GetToplevel::decode(frame.payload)?;
            Ok(DecodedRequest::GetToplevel {
                surface: payload.surface,
                new_id: payload.new_id,
            })
        }
        Operation::ToplevelSetTitle => {
            let payload = SetTitle::decode(frame.payload)?;
            Ok(DecodedRequest::SetTitle {
                toplevel: object,
                title: payload.title,
            })
        }
        Operation::SeatGetPointer => {
            let payload = NewId::decode(frame.payload)?;
            Ok(DecodedRequest::GetPointer {
                seat: object,
                new_id: payload.new_id,
            })
        }
        Operation::SeatGetKeyboard => {
            let payload = NewId::decode(frame.payload)?;
            Ok(DecodedRequest::GetKeyboard {
                seat: object,
                new_id: payload.new_id,
            })
        }
        Operation::ShortcutsRegister => {
            let payload = ShortcutRegistration::decode(frame.payload)?;
            let mode = ChordMatch::from_raw(payload.mode).ok_or(DecodeError::SizeOverflow)?;
            Ok(DecodedRequest::RegisterShortcut {
                manager: object,
                shortcut: payload.shortcut,
                seat: SeatId(payload.seat),
                chord: Chord {
                    trigger: PhysicalKey::from_usage(payload.trigger),
                    modifiers: payload.modifiers,
                    mode,
                },
            })
        }
        Operation::TextInputEnable => {
            let payload = GlobalName::decode(frame.payload)?;
            let purpose = TextPurpose::from_raw(payload.name).ok_or(DecodeError::SizeOverflow)?;
            Ok(DecodedRequest::EnableText {
                text_input: object,
                purpose,
            })
        }
        Operation::TextInputDisable => Ok(DecodedRequest::DisableText { text_input: object }),
        Operation::TextInputSetCursorArea => {
            let payload = CursorArea::decode(frame.payload)?;
            Ok(DecodedRequest::SetCursorArea {
                text_input: object,
                area: Rect {
                    x: payload.x,
                    y: payload.y,
                    width: payload.width,
                    height: payload.height,
                },
            })
        }
        Operation::InputMethodSetPreedit => {
            let payload = Preedit::decode(frame.payload)?;
            Ok(DecodedRequest::SetPreedit {
                method: object,
                text: payload.text,
                cursor_begin: payload.cursor_begin,
                cursor_end: payload.cursor_end,
            })
        }
        Operation::InputMethodCommit => {
            let payload = CommitText::decode(frame.payload)?;
            Ok(DecodedRequest::CommitText {
                method: object,
                text: payload.text,
            })
        }
        Operation::InputMethodDone => {
            let payload = GlobalName::decode(frame.payload)?;
            Ok(DecodedRequest::TextDone {
                method: object,
                serial: payload.name,
            })
        }
        Operation::ShortcutsUnregister => {
            let payload = ShortcutName::decode(frame.payload)?;
            Ok(DecodedRequest::UnregisterShortcut {
                manager: object,
                shortcut: payload.shortcut,
            })
        }
        Operation::ShellControlConfigure => {
            let payload = ShellConfigure::decode(frame.payload)?;
            Ok(DecodedRequest::ShellConfigure {
                control: object,
                handle: ToplevelHandle(payload.handle),
                size: Size {
                    width: payload.width,
                    height: payload.height,
                },
                state: payload.state,
            })
        }
        Operation::ShellControlPlace => {
            let payload = ShellPlace::decode(frame.payload)?;
            Ok(DecodedRequest::ShellPlace {
                control: object,
                handle: ToplevelHandle(payload.handle),
                position: Point {
                    x: payload.x,
                    y: payload.y,
                },
            })
        }
        Operation::ShellControlRaise => {
            let payload = ShellHandle::decode(frame.payload)?;
            Ok(DecodedRequest::ShellRaise {
                control: object,
                handle: ToplevelHandle(payload.handle),
            })
        }
        Operation::ShellControlFocus => {
            let payload = SeatHandle::decode(frame.payload)?;
            Ok(DecodedRequest::ShellFocus {
                control: object,
                seat: SeatId(payload.seat),
                handle: ToplevelHandle(payload.handle),
            })
        }
        Operation::ShellControlClose => {
            let payload = ShellHandle::decode(frame.payload)?;
            Ok(DecodedRequest::ShellClose {
                control: object,
                handle: ToplevelHandle(payload.handle),
            })
        }
        Operation::ShellControlGrab => {
            let payload = SeatHandle::decode(frame.payload)?;
            Ok(DecodedRequest::ShellGrab {
                control: object,
                seat: SeatId(payload.seat),
                handle: ToplevelHandle(payload.handle),
            })
        }
        Operation::ToplevelSetMinSize => {
            let payload = SizeHint::decode(frame.payload)?;
            Ok(DecodedRequest::SetMinSize {
                toplevel: object,
                size: Size {
                    width: payload.width,
                    height: payload.height,
                },
            })
        }
        Operation::ToplevelSetMaxSize => {
            let payload = SizeHint::decode(frame.payload)?;
            Ok(DecodedRequest::SetMaxSize {
                toplevel: object,
                size: Size {
                    width: payload.width,
                    height: payload.height,
                },
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
        Operation::SurfaceFrame => {
            let payload = GlobalName::decode(frame.payload)?;
            Ok(DecodedRequest::RequestFrame {
                surface: object,
                serial: payload.name,
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
    request: DecodedRequest<'_>,
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
        DecodedRequest::GetToplevel { surface, new_id } => {
            Ok(ClientRequest::GetToplevel { surface, new_id })
        }
        DecodedRequest::SetTitle { toplevel, title } => {
            // The bound is enforced where the value is made. A longer title is refused here
            // rather than truncated, because a truncated title is a wrong title.
            let title = TitleText::new(title).ok_or(ResolveError::TitleTooLong {
                bytes: title.len(),
                maximum: MAX_TITLE_BYTES,
            })?;
            Ok(ClientRequest::SetTitle { toplevel, title })
        }
        DecodedRequest::GetPointer { seat, new_id } => {
            Ok(ClientRequest::GetPointer { seat, new_id })
        }
        DecodedRequest::GetKeyboard { seat, new_id } => {
            Ok(ClientRequest::GetKeyboard { seat, new_id })
        }
        DecodedRequest::RegisterShortcut {
            manager,
            shortcut,
            seat,
            chord,
        } => Ok(ClientRequest::RegisterShortcut {
            manager,
            shortcut,
            seat,
            chord,
        }),
        DecodedRequest::UnregisterShortcut { manager, shortcut } => {
            Ok(ClientRequest::UnregisterShortcut { manager, shortcut })
        }
        DecodedRequest::EnableText {
            text_input,
            purpose,
        } => Ok(ClientRequest::EnableText {
            text_input,
            purpose,
        }),
        DecodedRequest::DisableText { text_input } => Ok(ClientRequest::DisableText { text_input }),
        DecodedRequest::SetCursorArea { text_input, area } => {
            Ok(ClientRequest::SetCursorArea { text_input, area })
        }
        DecodedRequest::SetPreedit {
            method,
            text,
            cursor_begin,
            cursor_end,
        } => Ok(ClientRequest::SetPreedit {
            method,
            text: TextBuffer::new(text).ok_or(ResolveError::TextTooLong {
                bytes: text.len(),
                maximum: MAX_TEXT_BYTES,
            })?,
            cursor_begin,
            cursor_end,
        }),
        DecodedRequest::CommitText { method, text } => Ok(ClientRequest::CommitText {
            method,
            text: TextBuffer::new(text).ok_or(ResolveError::TextTooLong {
                bytes: text.len(),
                maximum: MAX_TEXT_BYTES,
            })?,
        }),
        DecodedRequest::TextDone { method, serial } => {
            Ok(ClientRequest::TextDone { method, serial })
        }
        DecodedRequest::ShellConfigure {
            control,
            handle,
            size,
            state,
        } => Ok(ClientRequest::ShellConfigure {
            control,
            handle,
            size,
            state,
        }),
        DecodedRequest::ShellPlace {
            control,
            handle,
            position,
        } => Ok(ClientRequest::ShellPlace {
            control,
            handle,
            position,
        }),
        DecodedRequest::ShellRaise { control, handle } => {
            Ok(ClientRequest::ShellRaise { control, handle })
        }
        DecodedRequest::ShellFocus {
            control,
            seat,
            handle,
        } => Ok(ClientRequest::ShellFocus {
            control,
            seat,
            handle,
        }),
        DecodedRequest::ShellClose { control, handle } => {
            Ok(ClientRequest::ShellClose { control, handle })
        }
        DecodedRequest::ShellGrab {
            control,
            seat,
            handle,
        } => Ok(ClientRequest::ShellGrab {
            control,
            seat,
            handle,
        }),
        DecodedRequest::SetMinSize { toplevel, size } => {
            Ok(ClientRequest::SetMinSize { toplevel, size })
        }
        DecodedRequest::SetMaxSize { toplevel, size } => {
            Ok(ClientRequest::SetMaxSize { toplevel, size })
        }
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
        DecodedRequest::RequestFrame { surface, serial } => {
            Ok(ClientRequest::RequestFrame { surface, serial })
        }
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
