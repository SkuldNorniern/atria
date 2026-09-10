#![no_std]
#![forbid(unsafe_code)]

//! Backend-independent ATRIA compositor protocol state.
//!
//! This crate consumes already-decoded requests. It owns no sockets, descriptors, pixels,
//! input devices, renderer, or display backend. A backend validates/imports an external
//! buffer handle first, then supplies its inert [`BufferDescriptor`] here.

extern crate alloc;

mod binding;
mod error;
mod model;
mod registry;
mod resolve;
mod state;

pub use binding::{BindError, DecodedRequest, decode, interface_of, resolve};
pub use error::{ErrorCode, StateError};
pub use model::{
    BufferDescriptor, BufferState, BufferTransport, ClientRequest, CommitId, ConnectionId, Damage,
    Event, EventKind, FocusEvent, ObjectKind, Point, Rect, SeatCapabilities, SeatSnapshot,
    SessionSnapshot, Size, SurfaceKey, SurfaceRole, SurfaceSnapshot,
};
pub use registry::{ObjectRegistry, Teardown};
pub use resolve::{HandleResolver, ResolveError, SharedMemory};
pub use state::{CompositorState, ConnectionLimits, NegotiationError, ServerLimits};
