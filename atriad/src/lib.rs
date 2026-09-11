//! The display server: one compositor, many client sessions.
//!
//! The loop is the three stages with a transport on each end. Bytes arrive, decode says whether
//! they are a message, resolve turns the handles they name into resources, dispatch decides what
//! the compositor does about it, and whatever the compositor decided goes back out as events.
//!
//! Written as a library rather than only a binary because the properties worth proving — two
//! clients drawing at once, one of them dying without disturbing the other — are properties of
//! this loop, and a test can drive it over a socket pair without spawning anything.

mod present;
mod session;

pub use present::{PresentFailure, Presenter, present_all, present_committed, present_for};
pub use session::{Served, Session, SessionError};

use atria_compositor::{CompositorState, ConnectionLimits, ServerLimits};
use atria_protocol::capability::{Capability, CapabilitySet};

/// What a software-only compositor offers a client.
///
/// Stated here rather than assumed: a capability names something the backend actually enforces,
/// so a server with no GPU path must not advertise one.
#[must_use]
pub fn software_capabilities() -> CapabilitySet {
    CapabilitySet::default_grants().with(Capability::SoftwareShm)
}

/// Build a compositor with the bounds a server runs under.
///
/// The server supports shell control because it implements it. Whether any particular connection
/// may hold it is decided when that connection is admitted. The one set still mixes backend
/// support with authority, which is a split the capability model has yet to make.
#[must_use]
pub fn compositor(server: ServerLimits, connection: ConnectionLimits) -> CompositorState {
    CompositorState::new(
        software_capabilities()
            .with(Capability::ShellControl)
            .with(Capability::ShortcutControl),
        server,
        connection,
    )
}
