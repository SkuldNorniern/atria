//! Interface operations and the opcode assignments present in the draft.

use crate::DecodeError;

/// A wire opcode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Opcode(u16);

impl Opcode {
    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn into_raw(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn namespace(self) -> OpcodeNamespace {
        match self.0 {
            0x0000..=0x00ff => OpcodeNamespace::Core,
            0x0100..=0x01ff => OpcodeNamespace::FrameScheduling,
            0x0250..=0x027f => OpcodeNamespace::LinuxPlatform,
            0x0280..=0x02ff => OpcodeNamespace::ArteryPlatform,
            0x0200..=0x024f => OpcodeNamespace::FirstPartyExtension,
            0x0300..=0x03ff => OpcodeNamespace::FutureCore,
            0x0400..=0xefff => OpcodeNamespace::ThirdPartyExtension,
            0xf000..=0xffff => OpcodeNamespace::InternalDebug,
        }
    }

    /// Rejects the namespace that draft §13 forbids on untrusted client input.
    pub const fn validate_from_untrusted_client(self) -> Result<(), DecodeError> {
        if matches!(self.namespace(), OpcodeNamespace::InternalDebug) {
            Err(DecodeError::ForbiddenClientOpcode { opcode: self })
        } else {
            Ok(())
        }
    }
}

impl From<u16> for Opcode {
    fn from(value: u16) -> Self {
        Self::from_raw(value)
    }
}

impl From<Opcode> for u16 {
    fn from(value: Opcode) -> Self {
        value.into_raw()
    }
}

/// Namespace ranges assigned by draft §13.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpcodeNamespace {
    Core,
    FrameScheduling,
    FirstPartyExtension,
    LinuxPlatform,
    ArteryPlatform,
    FutureCore,
    ThirdPartyExtension,
    InternalDebug,
}

/// Core interfaces named by draft §4.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interface {
    Display,
    Registry,
    Seat,
    Session,
    Surface,
    Buffer,
    Fence,
    InputStream,
}

/// Whether an operation is a request, event, or left unstated by the draft.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageKind {
    Method,
    Event,
    Unspecified,
}

/// Every operation named for the core objects in §4, plus operations assigned by §§10, 13,
/// and 17. The latter sections add operations absent from §4; keeping them here exposes that
/// draft inconsistency without discarding assigned wire vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    DisplaySync,
    DisplayGetRegistry,
    DisplayGetError,
    DisplayError,
    DisplayCompositorFault,
    RegistryGlobal,
    RegistryGlobalRemove,
    SeatGetPointer,
    SeatGetKeyboard,
    SeatGetTouch,
    SeatRelease,
    SeatCapabilities,
    SeatName,
    SessionCreateSurface,
    SessionSetFocusPolicy,
    SessionDestroy,
    SessionRestore,
    SessionActive,
    SessionInactive,
    SessionDestroyed,
    SurfaceAttach,
    SurfaceAttachWithFence,
    SurfaceDamage,
    SurfaceSetOpaqueRegion,
    SurfaceCommit,
    SurfaceFrame,
    SurfaceSetRefreshRange,
    SurfaceDestroy,
    SurfaceEnter,
    SurfaceLeave,
    SurfaceConfigure,
    SurfaceClose,
    BufferDestroy,
    BufferRelease,
    BufferReleaseWithFence,
    FenceSignal,
    FenceWait,
    FenceMerge,
    FenceDestroy,
    InputPointerMotion,
    InputPointerButton,
    InputKey,
    InputTouchDown,
    InputTouchUp,
    InputTouchMotion,
    InputAxis,
}

impl Operation {
    #[must_use]
    pub const fn interface(self) -> Interface {
        match self {
            Self::DisplaySync
            | Self::DisplayGetRegistry
            | Self::DisplayGetError
            | Self::DisplayError
            | Self::DisplayCompositorFault => Interface::Display,
            Self::RegistryGlobal | Self::RegistryGlobalRemove => Interface::Registry,
            Self::SeatGetPointer
            | Self::SeatGetKeyboard
            | Self::SeatGetTouch
            | Self::SeatRelease
            | Self::SeatCapabilities
            | Self::SeatName => Interface::Seat,
            Self::SessionCreateSurface
            | Self::SessionSetFocusPolicy
            | Self::SessionDestroy
            | Self::SessionRestore
            | Self::SessionActive
            | Self::SessionInactive
            | Self::SessionDestroyed => Interface::Session,
            Self::SurfaceAttach
            | Self::SurfaceAttachWithFence
            | Self::SurfaceDamage
            | Self::SurfaceSetOpaqueRegion
            | Self::SurfaceCommit
            | Self::SurfaceFrame
            | Self::SurfaceSetRefreshRange
            | Self::SurfaceDestroy
            | Self::SurfaceEnter
            | Self::SurfaceLeave
            | Self::SurfaceConfigure
            | Self::SurfaceClose => Interface::Surface,
            Self::BufferDestroy | Self::BufferRelease | Self::BufferReleaseWithFence => {
                Interface::Buffer
            }
            Self::FenceSignal | Self::FenceWait | Self::FenceMerge | Self::FenceDestroy => {
                Interface::Fence
            }
            Self::InputPointerMotion
            | Self::InputPointerButton
            | Self::InputKey
            | Self::InputTouchDown
            | Self::InputTouchUp
            | Self::InputTouchMotion
            | Self::InputAxis => Interface::InputStream,
        }
    }

    #[must_use]
    pub const fn kind(self) -> MessageKind {
        match self {
            Self::DisplaySync
            | Self::DisplayGetRegistry
            | Self::DisplayGetError
            | Self::SeatGetPointer
            | Self::SeatGetKeyboard
            | Self::SeatGetTouch
            | Self::SeatRelease
            | Self::SessionCreateSurface
            | Self::SessionSetFocusPolicy
            | Self::SessionDestroy
            | Self::SessionRestore
            | Self::SurfaceAttach
            | Self::SurfaceAttachWithFence
            | Self::SurfaceDamage
            | Self::SurfaceSetOpaqueRegion
            | Self::SurfaceCommit
            | Self::SurfaceSetRefreshRange
            | Self::SurfaceDestroy
            | Self::BufferDestroy
            | Self::FenceSignal
            | Self::FenceWait
            | Self::FenceMerge
            | Self::FenceDestroy => MessageKind::Method,
            Self::DisplayError
            | Self::DisplayCompositorFault
            | Self::RegistryGlobal
            | Self::RegistryGlobalRemove
            | Self::SeatCapabilities
            | Self::SeatName
            | Self::SessionActive
            | Self::SessionInactive
            | Self::SessionDestroyed
            | Self::SurfaceEnter
            | Self::SurfaceLeave
            | Self::SurfaceConfigure
            | Self::SurfaceClose
            | Self::BufferRelease
            | Self::BufferReleaseWithFence
            | Self::InputPointerMotion
            | Self::InputPointerButton
            | Self::InputKey
            | Self::InputTouchDown
            | Self::InputTouchUp
            | Self::InputTouchMotion
            | Self::InputAxis => MessageKind::Event,
            // §13 assigns `surface.frame` a number but does not state whether it is a method or
            // event, and §4 does not list it. Do not infer direction from other protocols.
            Self::SurfaceFrame => MessageKind::Unspecified,
        }
    }

    /// Numeric assignment, when one exists in draft §§13 or 17.
    #[must_use]
    pub const fn opcode(self) -> Option<Opcode> {
        let raw = match self {
            Self::DisplayError => 0x0000,
            Self::DisplayGetRegistry => 0x0001,
            Self::RegistryGlobal => 0x0000,
            Self::SurfaceAttach => 0x0010,
            Self::SurfaceAttachWithFence => 0x0011,
            Self::SurfaceDamage => 0x0012,
            Self::SurfaceCommit => 0x0013,
            Self::SurfaceFrame => 0x0014,
            Self::SurfaceSetOpaqueRegion => 0x0015,
            Self::SurfaceSetRefreshRange => 0x0016,
            Self::SurfaceDestroy => 0x0017,
            Self::BufferReleaseWithFence => 0x0001,
            _ => return None,
        };
        Some(Opcode::from_raw(raw))
    }
}

/// Decodes only opcode assignments actually made by the draft.
pub fn decode_operation(
    interface: Interface,
    kind: MessageKind,
    opcode: Opcode,
) -> Result<Operation, DecodeError> {
    const OPERATIONS: &[Operation] = &[
        Operation::DisplayError,
        Operation::DisplayGetRegistry,
        Operation::RegistryGlobal,
        Operation::SurfaceAttach,
        Operation::SurfaceAttachWithFence,
        Operation::SurfaceDamage,
        Operation::SurfaceCommit,
        Operation::SurfaceFrame,
        Operation::SurfaceSetOpaqueRegion,
        Operation::SurfaceSetRefreshRange,
        Operation::SurfaceDestroy,
        Operation::BufferReleaseWithFence,
    ];

    for operation in OPERATIONS {
        if operation.interface() == interface
            && operation.kind() == kind
            && operation.opcode() == Some(opcode)
        {
            return Ok(*operation);
        }
    }
    Err(DecodeError::UnknownOpcode {
        interface,
        kind,
        opcode,
    })
}
