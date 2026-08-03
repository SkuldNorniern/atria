use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum DrmError {
    DeviceAbsent {
        card_index: u32,
    },
    OpenDevice {
        errno: i32,
    },
    SetMaster {
        errno: i32,
    },
    GetCapability {
        capability: u64,
        errno: i32,
    },
    EnableAtomicClient {
        errno: i32,
    },
    EnumerateResources {
        errno: i32,
    },
    ResourcesChanged,
    QueryConnector {
        connector_id: u32,
        errno: i32,
    },
    QueryEncoder {
        encoder_id: u32,
        errno: i32,
    },
    NoConnectedConnector,
    RequestedConnectorUnavailable(u32),
    ConnectorHasNoEncoder(u32),
    NoCompatibleCrtc {
        connector_id: u32,
    },
    DumbBuffersUnavailable,
    CreateDumbBuffer {
        buffer_index: usize,
        errno: i32,
    },
    MapDumbBuffer {
        buffer_index: usize,
        errno: i32,
    },
    MappingOffsetOutOfRange {
        buffer_index: usize,
        offset: u64,
    },
    MapMemory {
        buffer_index: usize,
        errno: i32,
    },
    AddFramebuffer {
        buffer_index: usize,
        errno: i32,
    },
    FrameSizeMismatch {
        frame_width: u32,
        frame_height: u32,
        mode_width: u16,
        mode_height: u16,
    },
    FrameLayoutMismatch {
        expected: u8,
        actual: u8,
    },
    FrameBufferTooSmall,
    FlipPending,
    EnumeratePlanes {
        errno: i32,
    },
    QueryPlane {
        plane_id: u32,
        errno: i32,
    },
    NoPrimaryPlane,
    QueryObjectProperties {
        object_id: u32,
        errno: i32,
    },
    QueryProperty {
        property_id: u32,
        errno: i32,
    },
    MissingAtomicProperty {
        object_id: u32,
        property_name: &'static str,
    },
    CreateModeBlob {
        errno: i32,
    },
    InitialModeset {
        errno: i32,
    },
    AtomicTestFailed {
        errno: i32,
    },
    AtomicCommit {
        errno: i32,
    },
    PageFlip {
        errno: i32,
    },
    DisplayAccessLost {
        errno: i32,
    },
    DisplayBusy {
        errno: i32,
    },
    ReadFlipEvent {
        errno: i32,
    },
    InvalidFlipEvent,
    FlipEventMissing,
    ZeroDimension,
    UnsupportedPixelLayout(u8),
    ArithmeticOverflow,
    PitchTooSmall {
        pitch: u32,
        required: u64,
    },
    BufferTooSmall {
        size: u64,
        required: u64,
    },
    NoModes,
    NoPreferredMode,
    RequestedModeUnavailable,
}

impl fmt::Display for DrmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceAbsent { card_index } => {
                write!(formatter, "/dev/dri/card{card_index} is absent")
            }
            Self::OpenDevice { errno } => {
                write!(
                    formatter,
                    "opening the DRM device failed with errno {errno}"
                )
            }
            Self::SetMaster { errno } => write!(
                formatter,
                "becoming DRM master failed with errno {errno}; another client may own the display"
            ),
            Self::GetCapability { capability, errno } => write!(
                formatter,
                "querying DRM capability {capability} failed with errno {errno}"
            ),
            Self::EnableAtomicClient { errno } => write!(
                formatter,
                "enabling DRM atomic client support failed with errno {errno}"
            ),
            Self::EnumerateResources { errno } => {
                write!(
                    formatter,
                    "enumerating DRM resources failed with errno {errno}"
                )
            }
            Self::ResourcesChanged => {
                formatter.write_str("DRM resources kept changing while they were being enumerated")
            }
            Self::QueryConnector {
                connector_id,
                errno,
            } => write!(
                formatter,
                "querying DRM connector {connector_id} failed with errno {errno}"
            ),
            Self::QueryEncoder { encoder_id, errno } => write!(
                formatter,
                "querying DRM encoder {encoder_id} failed with errno {errno}"
            ),
            Self::NoConnectedConnector => {
                formatter.write_str("the DRM device has no connected connector")
            }
            Self::RequestedConnectorUnavailable(connector_id) => write!(
                formatter,
                "requested DRM connector {connector_id} is unavailable or disconnected"
            ),
            Self::ConnectorHasNoEncoder(connector_id) => {
                write!(formatter, "DRM connector {connector_id} has no encoder")
            }
            Self::NoCompatibleCrtc { connector_id } => write!(
                formatter,
                "DRM connector {connector_id} has no encoder compatible with an available CRTC"
            ),
            Self::DumbBuffersUnavailable => {
                formatter.write_str("the DRM driver does not support dumb buffers")
            }
            Self::CreateDumbBuffer {
                buffer_index,
                errno,
            } => write!(
                formatter,
                "creating DRM dumb buffer {buffer_index} failed with errno {errno}"
            ),
            Self::MapDumbBuffer {
                buffer_index,
                errno,
            } => write!(
                formatter,
                "querying the mapping for DRM dumb buffer {buffer_index} failed with errno {errno}"
            ),
            Self::MappingOffsetOutOfRange {
                buffer_index,
                offset,
            } => write!(
                formatter,
                "mapping offset {offset} for DRM dumb buffer {buffer_index} is not representable"
            ),
            Self::MapMemory {
                buffer_index,
                errno,
            } => write!(
                formatter,
                "mapping DRM dumb buffer {buffer_index} failed with errno {errno}"
            ),
            Self::AddFramebuffer {
                buffer_index,
                errno,
            } => write!(
                formatter,
                "registering DRM framebuffer {buffer_index} failed with errno {errno}"
            ),
            Self::FrameSizeMismatch {
                frame_width,
                frame_height,
                mode_width,
                mode_height,
            } => write!(
                formatter,
                "frame size {frame_width}x{frame_height} does not match DRM mode {mode_width}x{mode_height}"
            ),
            Self::FrameLayoutMismatch { expected, actual } => write!(
                formatter,
                "frame uses {actual} bytes per pixel but the DRM sink requires {expected}"
            ),
            Self::FrameBufferTooSmall => {
                formatter.write_str("frame bytes do not cover the declared frame geometry")
            }
            Self::FlipPending => {
                formatter.write_str("a DRM page flip is already awaiting completion")
            }
            Self::EnumeratePlanes { errno } => {
                write!(
                    formatter,
                    "enumerating DRM planes failed with errno {errno}"
                )
            }
            Self::QueryPlane { plane_id, errno } => {
                write!(
                    formatter,
                    "querying DRM plane {plane_id} failed with errno {errno}"
                )
            }
            Self::NoPrimaryPlane => formatter
                .write_str("no primary DRM plane supports the selected CRTC and pixel format"),
            Self::QueryObjectProperties { object_id, errno } => write!(
                formatter,
                "querying properties for DRM object {object_id} failed with errno {errno}"
            ),
            Self::QueryProperty { property_id, errno } => write!(
                formatter,
                "querying DRM property {property_id} failed with errno {errno}"
            ),
            Self::MissingAtomicProperty {
                object_id,
                property_name,
            } => write!(
                formatter,
                "DRM object {object_id} lacks required atomic property {property_name}"
            ),
            Self::CreateModeBlob { errno } => {
                write!(
                    formatter,
                    "creating the DRM mode blob failed with errno {errno}"
                )
            }
            Self::InitialModeset { errno } => {
                write!(
                    formatter,
                    "setting the initial DRM mode failed with errno {errno}"
                )
            }
            Self::AtomicTestFailed { errno } => write!(
                formatter,
                "the test-only DRM atomic commit rejected the configuration with errno {errno}"
            ),
            Self::AtomicCommit { errno } => {
                write!(formatter, "the DRM atomic commit failed with errno {errno}")
            }
            Self::PageFlip { errno } => {
                write!(formatter, "the DRM page flip failed with errno {errno}")
            }
            Self::DisplayAccessLost { errno } => write!(
                formatter,
                "DRM display access was lost with errno {errno}, likely after a VT switch"
            ),
            Self::DisplayBusy { errno } => write!(
                formatter,
                "the DRM display is busy with errno {errno}, likely after ownership changed"
            ),
            Self::ReadFlipEvent { errno } => write!(
                formatter,
                "reading the DRM flip completion event failed with errno {errno}"
            ),
            Self::InvalidFlipEvent => {
                formatter.write_str("the DRM fd returned a malformed event record")
            }
            Self::FlipEventMissing => {
                formatter.write_str("the DRM fd read contained no matching flip completion")
            }
            Self::ZeroDimension => formatter.write_str("DRM buffer dimensions must be non-zero"),
            Self::UnsupportedPixelLayout(bytes) => {
                write!(
                    formatter,
                    "DRM scanout does not support {bytes} bytes per pixel"
                )
            }
            Self::ArithmeticOverflow => {
                formatter.write_str("DRM buffer geometry exceeds representable limits")
            }
            Self::PitchTooSmall { pitch, required } => write!(
                formatter,
                "DRM pitch {pitch} bytes is smaller than the required {required}-byte row"
            ),
            Self::BufferTooSmall { size, required } => write!(
                formatter,
                "DRM buffer has {size} bytes but its geometry requires {required} bytes"
            ),
            Self::NoModes => formatter.write_str("the DRM connector reports no display modes"),
            Self::NoPreferredMode => {
                formatter.write_str("the DRM connector reports no preferred display mode")
            }
            Self::RequestedModeUnavailable => {
                formatter.write_str("the requested DRM display mode is unavailable")
            }
        }
    }
}

impl Error for DrmError {}
