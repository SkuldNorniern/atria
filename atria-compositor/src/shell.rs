//! The authority a shell holds, and the window handles it holds it over.
//!
//! A shell is a separate connection from the applications it arranges, so it cannot name their
//! objects: an object identifier is connection-scoped, and the shell has never seen the client's.
//! The compositor mints a handle per window instead, compositor-wide and stable, and the shell
//! names that.
//!
//! That indirection is what makes the shell restartable. A replacement is told every window that
//! exists, by the same handles, and reconstructs the environment from what the compositor still
//! holds — rather than starting from nothing and leaving every application unplaced.

use alloc::string::String;

use atria_protocol::ObjectId;

use crate::model::{ConnectionId, SeatId, Size};

/// A window, as a shell refers to it.
///
/// Compositor-wide rather than connection-scoped, stable for as long as the window exists, and
/// never reused once retired. Reuse would let a shell's in-flight request land on a different
/// window than the one it meant — the same hazard object identifiers avoid with `delete_id`, and
/// the reason a handle is minted rather than derived from anything the client chose.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ToplevelHandle(pub u64);

/// What the compositor tells a shell about one window.
///
/// Everything a shell needs in order to arrange it, and nothing about how it is drawn. A shell
/// reconnecting after a restart is given one of these per live window, which is the whole of what
/// it needs to rebuild an arrangement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToplevelRecord {
    pub handle: ToplevelHandle,
    /// The connection and object behind the handle. The shell never sends these; they are here so
    /// a shell can be told which windows belong together without inventing an application id.
    pub connection: ConnectionId,
    pub object_id: ObjectId,
    pub title: String,
    /// What the compositor last asked it to be, if anything has.
    pub configured: Option<Size>,
}

/// Why a shell's request was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellError {
    /// The connection does not hold the authority to arrange windows.
    ///
    /// Separate from every other refusal because it is the answer to "may this program do this",
    /// and a test can hold the grant back and watch exactly this come out.
    NotGranted,
    /// No window answers to this handle. It was retired, or never existed.
    UnknownHandle { handle: ToplevelHandle },
    /// No seat answers to this name.
    UnknownSeat { seat: SeatId },
}
