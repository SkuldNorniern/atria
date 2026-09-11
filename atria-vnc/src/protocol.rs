//! The wire details of RFB 3.8, kept apart from the server that speaks it.

use std::borrow::Cow;
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

/// How a viewer wants pixels laid out.
///
/// A viewer picks this, not the server. RFB has no way to refuse one, so the only honest
/// responses are to produce what was asked for or to say the connection cannot be served — and a
/// server that quietly sent its own layout instead would put a picture on screen that is wrong in
/// a way nothing reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    /// What this server declares at the handshake.
    #[must_use]
    pub const fn declared() -> Self {
        Self {
            bits_per_pixel: 32,
            big_endian: false,
            true_colour: true,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 0,
            green_shift: 8,
            blue_shift: 16,
        }
    }

    /// Read the format out of a `SetPixelFormat` body.
    ///
    /// # Errors
    ///
    /// Returns `InvalidData` for a format this server cannot produce: a colour-mapped one, or a
    /// pixel size that is not one, two or four bytes.
    pub fn decode(body: &[u8; 16]) -> io::Result<Self> {
        let format = Self {
            bits_per_pixel: body[0],
            big_endian: body[2] != 0,
            true_colour: body[3] != 0,
            red_max: u16::from_be_bytes([body[4], body[5]]),
            green_max: u16::from_be_bytes([body[6], body[7]]),
            blue_max: u16::from_be_bytes([body[8], body[9]]),
            red_shift: body[10],
            green_shift: body[11],
            blue_shift: body[12],
        };
        if !format.true_colour {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the viewer asked for a colour map, and this server has only true colour",
            ));
        }
        if !matches!(format.bits_per_pixel, 8 | 16 | 32) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the viewer asked for a pixel size this server does not produce",
            ));
        }
        Ok(format)
    }

    /// Bytes one pixel takes on the wire.
    #[must_use]
    pub const fn stride(&self) -> usize {
        (self.bits_per_pixel / 8) as usize
    }

    /// Lay one of the compositor's pixels out as this format wants it.
    pub fn write_into(&self, source: &[u8], out: &mut [u8]) {
        self.write_pixel(source[0], source[1], source[2], out);
    }

    /// Lay one colour out as this format wants it.
    fn write_pixel(&self, red: u8, green: u8, blue: u8, out: &mut [u8]) {
        let scale =
            |value: u8, max: u16| (u32::from(value) * u32::from(max) / 255) & u32::from(max);
        let packed = (scale(red, self.red_max) << self.red_shift)
            | (scale(green, self.green_max) << self.green_shift)
            | (scale(blue, self.blue_max) << self.blue_shift);
        let bytes = packed.to_le_bytes();
        let width = self.stride();
        if self.big_endian {
            for (index, slot) in out.iter_mut().take(width).enumerate() {
                *slot = bytes[width - 1 - index];
            }
        } else {
            out[..width].copy_from_slice(&bytes[..width]);
        }
    }

    /// Convert a frame of this server's own layout into what the viewer asked for.
    ///
    /// Returns the frame untouched when the viewer kept the declared format, which is the common
    /// case and the one worth not copying for.
    #[must_use]
    pub fn convert<'a>(&self, frame: &'a [u8]) -> Cow<'a, [u8]> {
        if *self == Self::declared() {
            return Cow::Borrowed(frame);
        }
        let width = self.stride();
        let mut out = vec![0_u8; frame.len() / 4 * width];
        for (source, slot) in frame.chunks_exact(4).zip(out.chunks_exact_mut(width)) {
            self.write_pixel(source[0], source[1], source[2], slot);
        }
        Cow::Owned(out)
    }
}

/// Read exactly `out.len()` bytes, or fail.
pub fn read_exact(stream: &mut impl Read, out: &mut [u8]) -> io::Result<()> {
    stream.read_exact(out)
}

/// A rectangle of the framebuffer, in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// The side of the squares the frame is compared in.
///
/// Comparing whole rows would send a whole row for one changed pixel; comparing single pixels
/// would produce a rectangle list longer than the pixels it describes. Sixty-four is small enough
/// that a moved window costs about what the window covers, and large enough that the list stays
/// short.
const TILE: usize = 64;

/// Past this share of the frame, one rectangle covering everything is cheaper than the list.
const WHOLESALE_TILES: usize = 2;

/// Which parts of the frame differ from the one before it.
///
/// This is what makes the difference between sending a window and sending the screen. A frame at
/// 1280x720 is three and a half megabytes; a window moving touches a tenth of that, and on
/// anything slower than loopback the rest is the whole of the latency.
///
/// Returns one rectangle covering the frame when there is no previous frame to compare against,
/// or when so much changed that the comparison has stopped paying for itself.
#[must_use]
pub fn changed_regions(
    previous: Option<&[u8]>,
    current: &[u8],
    width: u16,
    height: u16,
) -> Vec<Region> {
    let whole = vec![Region {
        x: 0,
        y: 0,
        width,
        height,
    }];
    let Some(previous) = previous else {
        return whole;
    };
    if previous.len() != current.len() {
        return whole;
    }

    let pixels = usize::from(width);
    let rows = usize::from(height);
    let across = pixels.div_ceil(TILE);
    let down = rows.div_ceil(TILE);
    let mut regions = Vec::new();
    let mut changed = 0_usize;

    for tile_y in 0..down {
        let top = tile_y * TILE;
        let bottom = (top + TILE).min(rows);
        // Runs of adjacent changed tiles become one rectangle, so a window spanning six of them
        // is one rectangle rather than six.
        let mut run: Option<(usize, usize)> = None;
        for tile_x in 0..across {
            let left = tile_x * TILE;
            let right = (left + TILE).min(pixels);
            let differs = (top..bottom).any(|row| {
                let start = (row * pixels + left) * 4;
                let end = (row * pixels + right) * 4;
                previous[start..end] != current[start..end]
            });
            match (differs, run) {
                (true, None) => run = Some((left, right)),
                (true, Some((start, _))) => run = Some((start, right)),
                (false, Some((start, end))) => {
                    changed += (end - start) * (bottom - top);
                    regions.push(region(start, top, end, bottom));
                    run = None;
                }
                (false, None) => {}
            }
        }
        if let Some((start, end)) = run {
            changed += (end - start) * (bottom - top);
            regions.push(region(start, top, end, bottom));
        }
    }

    if changed * WHOLESALE_TILES >= pixels * rows {
        return whole;
    }
    regions
}

fn region(left: usize, top: usize, right: usize, bottom: usize) -> Region {
    Region {
        x: left as u16,
        y: top as u16,
        width: (right - left) as u16,
        height: (bottom - top) as u16,
    }
}

/// Encodings this server can produce, in the order it prefers them.
pub mod encoding {
    /// Every pixel, as it is. Every viewer supports this and no viewer has to ask for it.
    pub const RAW: i32 = 0;
    /// Sixteen-pixel squares, each either one colour or raw.
    ///
    /// A window of flat colour costs a handful of bytes per square instead of a thousand, and
    /// content that is not flat costs one byte more than raw. There is no case where it is worse
    /// by more than that, which is why it is worth having and compression is not yet.
    pub const HEXTILE: i32 = 5;
}

/// The side of a hextile square, fixed by the specification.
const HEXTILE_SIDE: usize = 16;

/// Hextile subencoding bits, from the specification.
const HEXTILE_RAW: u8 = 1;
const HEXTILE_BACKGROUND: u8 = 2;

/// Write one rectangle as hextile squares.
fn write_hextile(
    message: &mut Vec<u8>,
    region: Region,
    frame: &[u8],
    width: u16,
    format: PixelFormat,
) {
    let stride = usize::from(width) * 4;
    let pixel = format.stride();
    let mut encoded = vec![0_u8; pixel];

    let mut y = 0;
    while y < usize::from(region.height) {
        let tall = HEXTILE_SIDE.min(usize::from(region.height) - y);
        let mut x = 0;
        while x < usize::from(region.width) {
            let wide = HEXTILE_SIDE.min(usize::from(region.width) - x);
            let at = |row: usize, column: usize| {
                let start = (usize::from(region.y) + y + row) * stride
                    + (usize::from(region.x) + x + column) * 4;
                &frame[start..start + 4]
            };

            let first = at(0, 0);
            let uniform = (0..tall).all(|row| (0..wide).all(|column| at(row, column) == first));
            if uniform {
                // One colour: the square is its background and nothing else.
                message.push(HEXTILE_BACKGROUND);
                format.write_into(first, &mut encoded);
                message.extend_from_slice(&encoded);
            } else {
                message.push(HEXTILE_RAW);
                for row in 0..tall {
                    for column in 0..wide {
                        format.write_into(at(row, column), &mut encoded);
                        message.extend_from_slice(&encoded);
                    }
                }
            }
            x += HEXTILE_SIDE;
        }
        y += HEXTILE_SIDE;
    }
}

/// Write a framebuffer update holding the given rectangles.
///
/// # Errors
///
/// Returns the platform's error when the viewer cannot be written to.
pub fn write_update(
    stream: &mut impl Write,
    regions: &[Region],
    frame: &[u8],
    width: u16,
    format: PixelFormat,
    hextile: bool,
) -> io::Result<()> {
    let mut message = Vec::with_capacity(4 + regions.len() * 12);
    message.push(0); // FramebufferUpdate
    message.push(0); // padding
    message.extend_from_slice(&(regions.len() as u16).to_be_bytes());

    let stride = usize::from(width) * 4;
    for region in regions {
        message.extend_from_slice(&region.x.to_be_bytes());
        message.extend_from_slice(&region.y.to_be_bytes());
        message.extend_from_slice(&region.width.to_be_bytes());
        message.extend_from_slice(&region.height.to_be_bytes());
        if hextile {
            message.extend_from_slice(&encoding::HEXTILE.to_be_bytes());
            write_hextile(&mut message, *region, frame, width, format);
            continue;
        }
        message.extend_from_slice(&encoding::RAW.to_be_bytes());
        for row in 0..usize::from(region.height) {
            let start = (usize::from(region.y) + row) * stride + usize::from(region.x) * 4;
            let end = start + usize::from(region.width) * 4;
            message.extend_from_slice(&format.convert(&frame[start..end]));
        }
    }
    stream.write_all(&message)?;
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
