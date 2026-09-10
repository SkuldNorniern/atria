//! The platform boundary. Everything above this sees owned descriptors and slices.

#[cfg(unix)]
pub mod unix;
