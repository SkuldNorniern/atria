#![no_std]
#![forbid(unsafe_code)]

//! The user-facing facade for Atria.
//!
//! Only the wire protocol exists at this stage. It is re-exported here so consumers can
//! start with the repository's root package.

pub use atria_protocol as protocol;
