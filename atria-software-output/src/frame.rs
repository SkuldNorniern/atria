use atria_compositor::Size;

use crate::ValidationError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelLayout {
    bytes_per_pixel: u8,
}

impl PixelLayout {
    pub fn new(bytes_per_pixel: u8) -> Result<Self, ValidationError> {
        if bytes_per_pixel == 0 {
            return Err(ValidationError::ZeroBytesPerPixel);
        }
        Ok(Self { bytes_per_pixel })
    }

    #[must_use]
    pub const fn bytes_per_pixel(self) -> u8 {
        self.bytes_per_pixel
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    size: Size,
    stride: u32,
    layout: PixelLayout,
    bytes: Vec<u8>,
}

impl Frame {
    pub(crate) fn new(size: Size, layout: PixelLayout) -> Result<Self, ValidationError> {
        if size.width == 0 || size.height == 0 {
            return Err(ValidationError::ZeroAreaSurface);
        }
        let stride = u64::from(size.width)
            .checked_mul(u64::from(layout.bytes_per_pixel))
            .ok_or(ValidationError::ArithmeticOverflow)?;
        let byte_len = stride
            .checked_mul(u64::from(size.height))
            .ok_or(ValidationError::ArithmeticOverflow)?;
        let stride = u32::try_from(stride).map_err(|_| ValidationError::ArithmeticOverflow)?;
        let byte_len =
            usize::try_from(byte_len).map_err(|_| ValidationError::ArithmeticOverflow)?;
        if byte_len > isize::MAX as usize {
            return Err(ValidationError::ArithmeticOverflow);
        }
        Ok(Self {
            size,
            stride,
            layout,
            bytes: vec![0; byte_len],
        })
    }

    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    #[must_use]
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    #[must_use]
    pub const fn layout(&self) -> PixelLayout {
        self.layout
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Set every pixel back to nothing, keeping the allocation.
    pub(crate) fn clear(&mut self) {
        self.bytes.fill(0);
    }

    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}
