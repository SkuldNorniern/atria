#![no_std]
#![forbid(unsafe_code)]

//! Allocation-free wire support for ATRIA display protocol draft v0.1.
//!
//! The crate implements the framing and encodings that the draft specifies. It deliberately
//! Every interface, opcode and payload layout is declared once in [`interface`], and the decoder
//! is driven from that table rather than from numbers written down beside it.

pub mod capability;
pub mod error;
pub mod interface;
pub mod message;
pub mod object;
pub mod opcode;
pub mod version;
pub mod wire;

pub use error::{DecodeError, EncodeError};
pub use interface::{Interface, MessageKind, Operation, decode_operation};
pub use object::ObjectId;
pub use opcode::Opcode;
pub use wire::{HandleIndex, HandleKind};
