#![no_std]
#![forbid(unsafe_code)]

//! Allocation-free wire support for ATRIA display protocol draft v0.1.
//!
//! The crate implements the framing and encodings that the draft specifies. It deliberately
//! does not assign numbers or payloads where the draft is silent. See [`opcode::Operation`]
//! for named operations whose numeric opcode remains unspecified.

pub mod capability;
pub mod error;
pub mod message;
pub mod object;
pub mod opcode;
pub mod version;
pub mod wire;

pub use error::{DecodeError, EncodeError};
pub use object::ObjectId;
pub use opcode::Opcode;
