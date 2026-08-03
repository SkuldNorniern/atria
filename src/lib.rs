#![no_std]
#![forbid(unsafe_code)]

//! The user-facing facade for Atria.
//!
//! The facade exposes wire vocabulary, compositor state, and software output.

pub use atria_compositor as compositor;
pub use atria_protocol as protocol;
pub use atria_software_output as software_output;
