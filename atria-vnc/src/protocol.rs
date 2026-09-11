//! The wire details of RFB 3.8, kept apart from the server that speaks it.

use std::io::{self, Read, Write};

/// What the server announces, and what a client must answer with.
pub const VERSION: &[u8; 12] = b"RFB 003.008\n";

/// Security type 1: none. The framebuffer is served on a loopback socket for looking at a frame,
/// and a password on that would be security theatre rather than security.
pub const SECURITY_NONE: u8 = 1;

/// Client-to-server message numbers, from the specification.
pub mod client_message {
    pub const SET_PIXEL_FORMAT: u8 = 0;
    pub const SET_ENCODINGS: u8 = 2;
    pub const FRAMEBUFFER_UPDATE_REQUEST: u8 = 3;
    pub const KEY_EVENT: u8 = 4;
    pub const POINTER_EVENT: u8 = 5;
    pub const CLIENT_CUT_TEXT: u8 = 6;
}

/// The pixel format the server declares.
///
/// The frame holds four bytes a pixel in red, green, blue, unused order. Declaring
/// little-endian with red at shift 0 and blue at 16 makes a client read those bytes back in the
/// order they are written, so no conversion happens on either side. Getting this pair wrong is
/// how a frame arrives with its red and blue exchanged.
pub fn pixel_format(out: &mut [u8; 16]) {
    out[0] = 32; // bits per pixel
    out[1] = 24; // depth
    out[2] = 0; // big-endian flag: the bytes are little-endian
    out[3] = 1; // true colour
    out[4..6].copy_from_slice(&255_u16.to_be_bytes()); // red max
    out[6..8].copy_from_slice(&255_u16.to_be_bytes()); // green max
    out[8..10].copy_from_slice(&255_u16.to_be_bytes()); // blue max
    out[10] = 0; // red shift
    out[11] = 8; // green shift
    out[12] = 16; // blue shift
    out[13..16].fill(0); // padding
}

/// Read exactly `out.len()` bytes, or fail.
pub fn read_exact(stream: &mut impl Read, out: &mut [u8]) -> io::Result<()> {
    stream.read_exact(out)
}

/// Write a framebuffer update holding one raw rectangle covering the whole frame.
///
/// One rectangle rather than a damage list: the compositor already knows what changed, and
/// sending everything is what keeps this tool simple enough to trust. A frame at 1920x1080 is
/// eight megabytes, which is fine over loopback and would not be over a network.
pub fn write_full_update(
    stream: &mut impl Write,
    width: u16,
    height: u16,
    pixels: &[u8],
) -> io::Result<()> {
    let mut header = [0_u8; 16];
    header[0] = 0; // FramebufferUpdate
    header[1] = 0; // padding
    header[2..4].copy_from_slice(&1_u16.to_be_bytes()); // one rectangle
    header[4..6].copy_from_slice(&0_u16.to_be_bytes()); // x
    header[6..8].copy_from_slice(&0_u16.to_be_bytes()); // y
    header[8..10].copy_from_slice(&width.to_be_bytes());
    header[10..12].copy_from_slice(&height.to_be_bytes());
    header[12..16].copy_from_slice(&0_i32.to_be_bytes()); // raw encoding
    stream.write_all(&header)?;
    stream.write_all(pixels)?;
    stream.flush()
}

/// How many bytes follow the one-byte number of a client message, and whether the length is
/// itself in the payload.
///
/// Every client message has to be consumed whole even when it is ignored, or the next read starts
/// mid-message and every message after it is misread.
pub const fn client_message_length(number: u8) -> Option<usize> {
    match number {
        client_message::SET_PIXEL_FORMAT => Some(19),
        client_message::FRAMEBUFFER_UPDATE_REQUEST => Some(9),
        client_message::KEY_EVENT => Some(7),
        client_message::POINTER_EVENT => Some(5),
        // SetEncodings and ClientCutText carry a count that decides their length.
        _ => None,
    }
}
