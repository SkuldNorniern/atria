#![no_std]
#![forbid(unsafe_code)]

//! Backend-independent ATRIA compositor protocol state.
//!
//! This crate consumes already-decoded requests. It owns no sockets, descriptors, pixels,
//! input devices, renderer, or display backend. A backend validates/imports an external
//! buffer handle first, then supplies its inert [`BufferDescriptor`] here.

extern crate alloc;

mod binding;
mod emit;
mod error;
mod model;
mod output;
mod registry;
mod resolve;
mod state;

pub use binding::{BindError, DecodedRequest, decode, interface_of, resolve};
pub use emit::{EmitError, encode_event};
pub use error::{ErrorCode, StateError};
pub use model::{
    BufferDescriptor, BufferState, BufferTransport, ClientRequest, CommitId, ConnectionId, Damage,
    Event, EventKind, FORMAT_ARGB8888, FORMAT_RGB565, FORMAT_XRGB8888, FocusEvent, ObjectKind,
    Point, Rect, SeatCapabilities, SeatSnapshot, SessionSnapshot, Size, SurfaceKey, SurfaceRole,
    SurfaceSnapshot, pixel_format_is_known,
};
pub use output::{
    IdentitySource, MAX_SCALE_TERM, OutputIdentity, OutputInfo, OutputSet, TopologyDelta,
};
pub use registry::{ObjectRegistry, Teardown};
pub use resolve::{HandleResolver, ResolveError, SharedMemory};
pub use state::{CompositorState, ConnectionLimits, NegotiationError, ServerLimits};
