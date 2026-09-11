//! Serving composed frames over the remote framebuffer protocol.
//!
//! This exists so a person can look at what the compositor produced. Every visual claim until now
//! has been an assertion about bytes in a buffer; this is what makes one confirmable by eye.
//!
//! Deliberately the simplest server that is correct: RFB 3.8, no authentication, raw encoding, one
//! client at a time. Compression and incremental updates are optimisations, and an optimisation in
//! the thing you check your work with is a place for a bug to hide.

mod protocol;
mod server;

pub use server::{VncError, VncSink};
