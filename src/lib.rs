#![no_std]
#![forbid(unsafe_code)]

//! The user-facing facade for Atria.
//!
//! The facade exposes wire vocabulary and the backend-independent compositor state machine.

pub use atria_compositor as compositor;
pub use atria_protocol as protocol;
