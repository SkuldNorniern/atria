use atria_software_output::PixelLayout;

use crate::DrmError;

const MODE_TYPE_PREFERRED: u32 = 1 << 3;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_le_bytes([a, b, c, d])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum DrmFormat {
    C8 = fourcc(b'C', b'8', b' ', b' '),
    Rgb565 = fourcc(b'R', b'G', b'1', b'6'),
    Rgb888 = fourcc(b'R', b'G', b'2', b'4'),
    Xrgb8888 = fourcc(b'X', b'R', b'2', b'4'),
}

impl DrmFormat {
    #[must_use]
    pub const fn fourcc(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mode {
    pub width: u16,
    pub height: u16,
    pub refresh_hz: u32,
    pub mode_type: u32,
}

impl Mode {
    #[must_use]
    pub const fn preferred(self) -> bool {
        self.mode_type & MODE_TYPE_PREFERRED != 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ModeSelection {
    #[default]
    Preferred,
    Exact {
        width: u16,
        height: u16,
        refresh_hz: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferGeometry {
    pub row_bytes: u64,
    pub size: u64,
}

pub fn select_mode(modes: &[Mode], selection: ModeSelection) -> Result<usize, DrmError> {
    if modes.is_empty() {
        return Err(DrmError::NoModes);
    }
    match selection {
        ModeSelection::Preferred => modes
            .iter()
            .position(|mode| mode.preferred())
            .ok_or(DrmError::NoPreferredMode),
        ModeSelection::Exact {
            width,
            height,
            refresh_hz,
        } => modes
            .iter()
            .position(|mode| {
                mode.width == width && mode.height == height && mode.refresh_hz == refresh_hz
            })
            .ok_or(DrmError::RequestedModeUnavailable),
    }
}

pub fn buffer_geometry(
    width: u32,
    height: u32,
    bytes_per_pixel: u8,
    pitch: u32,
    buffer_size: u64,
) -> Result<BufferGeometry, DrmError> {
    if width == 0 || height == 0 {
        return Err(DrmError::ZeroDimension);
    }
    let row_bytes = u64::from(width)
        .checked_mul(u64::from(bytes_per_pixel))
        .ok_or(DrmError::ArithmeticOverflow)?;
    if u64::from(pitch) < row_bytes {
        return Err(DrmError::PitchTooSmall {
            pitch,
            required: row_bytes,
        });
    }
    let size = u64::from(pitch)
        .checked_mul(u64::from(height))
        .ok_or(DrmError::ArithmeticOverflow)?;
    if buffer_size < size {
        return Err(DrmError::BufferTooSmall {
            size: buffer_size,
            required: size,
        });
    }
    let _ = usize::try_from(size).map_err(|_| DrmError::ArithmeticOverflow)?;
    if size > isize::MAX as u64 {
        return Err(DrmError::ArithmeticOverflow);
    }
    Ok(BufferGeometry { row_bytes, size })
}

#[must_use]
pub const fn next_buffer_index(scanning_index: usize) -> usize {
    scanning_index ^ 1
}

pub fn drm_format(layout: PixelLayout) -> Result<DrmFormat, DrmError> {
    match layout.bytes_per_pixel() {
        1 => Ok(DrmFormat::C8),
        2 => Ok(DrmFormat::Rgb565),
        3 => Ok(DrmFormat::Rgb888),
        4 => Ok(DrmFormat::Xrgb8888),
        bytes => Err(DrmError::UnsupportedPixelLayout(bytes)),
    }
}
