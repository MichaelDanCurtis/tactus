//! Device-neutral core types for the Tactus control-surface SDK.
//!
//! Nothing in this crate is specific to any vendor or device. Vendor protocol
//! lives in the transport and device crates; this is the shared vocabulary.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use thiserror::Error;

/// Errors produced by core operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    /// A rectangle extended outside the surface it was applied to.
    #[error("rect {rect:?} does not fit within surface {surface_w}x{surface_h}")]
    RectOutOfBounds {
        /// The offending rectangle.
        rect: Rect,
        /// Surface width in pixels.
        surface_w: u32,
        /// Surface height in pixels.
        surface_h: u32,
    },
    /// A surface was constructed with a pixel buffer too small for its geometry.
    #[error("buffer holds {got} pixels but {stride}x{height} geometry needs {need}")]
    BufferTooSmall {
        /// Pixels actually supplied.
        got: usize,
        /// Pixels required.
        need: usize,
        /// Row stride in pixels.
        stride: u32,
        /// Height in rows.
        height: u32,
    },
    /// A surface dimension was zero.
    #[error("surface dimensions must be non-zero, got {width}x{height}")]
    ZeroSizedSurface {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
}

/// An axis-aligned rectangle expressed as **origin plus extent**.
///
/// Origin-plus-extent rather than two corners, because that is what every
/// display wire format encountered so far uses; converting at the boundary
/// would invite off-by-one errors in the one place they are least visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge, in pixels from the surface origin.
    pub x: u32,
    /// Top edge, in pixels from the surface origin.
    pub y: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl Rect {
    /// Construct a rectangle.
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// A rectangle covering a whole surface of the given size.
    pub const fn whole(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            w: width,
            h: height,
        }
    }

    /// True when the rectangle encloses no pixels.
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Number of pixels the rectangle covers.
    ///
    /// Uses saturating arithmetic: an absurd rectangle yields a saturated
    /// count rather than wrapping to a small one, which would be the
    /// dangerous failure.
    pub const fn pixel_count(&self) -> u64 {
        (self.w as u64).saturating_mul(self.h as u64)
    }

    /// Right edge, exclusive. Saturating.
    pub const fn right(&self) -> u32 {
        self.x.saturating_add(self.w)
    }

    /// Bottom edge, exclusive. Saturating.
    pub const fn bottom(&self) -> u32 {
        self.y.saturating_add(self.h)
    }

    /// True when this rectangle lies entirely within a surface of the given size.
    pub const fn fits_within(&self, width: u32, height: u32) -> bool {
        self.right() <= width && self.bottom() <= height
    }
}

/// A read-only 32-bit source surface, one `0xAARRGGBB` word per pixel.
///
/// Host-side pixels are always 32-bit regardless of what a device's wire
/// format is; format conversion happens during encoding, not here.
#[derive(Debug, Clone, Copy)]
pub struct Surface<'a> {
    pixels: &'a [u32],
    stride: u32,
    width: u32,
    height: u32,
}

impl<'a> Surface<'a> {
    /// Wrap a pixel buffer.
    ///
    /// `stride` is the distance between the starts of consecutive rows, in
    /// pixels, and may exceed `width` for padded buffers.
    pub fn new(pixels: &'a [u32], width: u32, height: u32, stride: u32) -> Result<Self, CoreError> {
        if width == 0 || height == 0 {
            return Err(CoreError::ZeroSizedSurface { width, height });
        }
        let effective_stride = stride.max(width);
        // Saturating rather than wrapping: an absurd geometry must report a
        // requirement no buffer can satisfy, so the check below rejects it.
        // Wrapping here would compute a small requirement and let an
        // out-of-bounds surface through.
        let need = (effective_stride as usize).saturating_mul(height as usize);
        if pixels.len() < need {
            return Err(CoreError::BufferTooSmall {
                got: pixels.len(),
                need,
                stride: effective_stride,
                height,
            });
        }
        Ok(Self {
            pixels,
            stride: effective_stride,
            width,
            height,
        })
    }

    /// Wrap a tightly packed buffer, inferring stride from width.
    pub fn packed(pixels: &'a [u32], width: u32, height: u32) -> Result<Self, CoreError> {
        Self::new(pixels, width, height, width)
    }

    /// Surface width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Surface height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Row stride in pixels.
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    /// Fetch one pixel as `0xAARRGGBB`, or `None` if out of bounds.
    ///
    /// Total by construction: an out-of-range coordinate yields `None` rather
    /// than panicking, because encoders walk rectangles supplied by callers.
    pub fn pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = (y as usize)
            .checked_mul(self.stride as usize)?
            .checked_add(x as usize)?;
        self.pixels.get(idx).copied()
    }

    /// Whether a rectangle lies entirely inside this surface.
    pub const fn contains_rect(&self, rect: &Rect) -> bool {
        rect.fits_within(self.width, self.height)
    }

    /// Validate a rectangle against this surface.
    pub fn check_rect(&self, rect: &Rect) -> Result<(), CoreError> {
        if self.contains_rect(rect) {
            Ok(())
        } else {
            Err(CoreError::RectOutOfBounds {
                rect: *rect,
                surface_w: self.width,
                surface_h: self.height,
            })
        }
    }

    /// True when both surfaces have identical geometry, so a delta between
    /// them is meaningful.
    pub const fn geometry_matches(&self, other: &Surface<'_>) -> bool {
        self.width == other.width && self.height == other.height
    }
}

/// Split a `0xAARRGGBB` word into its channels.
///
/// Returned as `(a, r, g, b)`.
#[inline]
pub const fn argb_channels(px: u32) -> (u8, u8, u8, u8) {
    (
        (px >> 24) as u8,
        (px >> 16) as u8,
        (px >> 8) as u8,
        px as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_origin_plus_extent_semantics() {
        let r = Rect::new(10, 20, 30, 40);
        assert_eq!(r.right(), 40);
        assert_eq!(r.bottom(), 60);
        assert_eq!(r.pixel_count(), 1200);
        assert!(!r.is_empty());
        assert!(Rect::new(0, 0, 0, 5).is_empty());
    }

    #[test]
    fn rect_arithmetic_saturates_rather_than_wrapping() {
        // A wrapping right() would report a huge rect as tiny, which is the
        // failure that silently corrupts a framebuffer.
        let r = Rect::new(u32::MAX, u32::MAX, u32::MAX, u32::MAX);
        assert_eq!(r.right(), u32::MAX);
        assert_eq!(r.bottom(), u32::MAX);
        assert_eq!(r.pixel_count(), (u32::MAX as u64) * (u32::MAX as u64));
        assert!(!r.fits_within(100, 100));
    }

    #[test]
    fn whole_covers_exactly_the_surface() {
        let r = Rect::whole(480, 272);
        assert!(r.fits_within(480, 272));
        assert!(!r.fits_within(479, 272));
        assert!(!r.fits_within(480, 271));
    }

    #[test]
    fn surface_rejects_undersized_buffer() {
        let px = [0u32; 10];
        let err = Surface::packed(&px, 4, 4).unwrap_err();
        assert!(matches!(err, CoreError::BufferTooSmall { .. }));
    }

    #[test]
    fn surface_rejects_zero_dimensions() {
        let px = [0u32; 16];
        assert!(matches!(
            Surface::packed(&px, 0, 4),
            Err(CoreError::ZeroSizedSurface { .. })
        ));
        assert!(matches!(
            Surface::packed(&px, 4, 0),
            Err(CoreError::ZeroSizedSurface { .. })
        ));
    }

    #[test]
    fn surface_honours_stride_when_reading() {
        // 2x2 image inside a 4-pixel-wide buffer.
        let px: [u32; 8] = [1, 2, 99, 99, 3, 4, 99, 99];
        let s = Surface::new(&px, 2, 2, 4).unwrap();
        assert_eq!(s.pixel(0, 0), Some(1));
        assert_eq!(s.pixel(1, 0), Some(2));
        assert_eq!(s.pixel(0, 1), Some(3));
        assert_eq!(s.pixel(1, 1), Some(4));
        // Padding is not reachable through the logical width.
        assert_eq!(s.pixel(2, 0), None);
    }

    #[test]
    fn pixel_fetch_is_total_out_of_bounds() {
        let px = [0u32; 16];
        let s = Surface::packed(&px, 4, 4).unwrap();
        assert_eq!(s.pixel(4, 0), None);
        assert_eq!(s.pixel(0, 4), None);
        assert_eq!(s.pixel(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn check_rect_reports_the_offending_geometry() {
        let px = [0u32; 16];
        let s = Surface::packed(&px, 4, 4).unwrap();
        assert!(s.check_rect(&Rect::new(0, 0, 4, 4)).is_ok());
        match s.check_rect(&Rect::new(2, 2, 4, 4)) {
            Err(CoreError::RectOutOfBounds {
                surface_w,
                surface_h,
                ..
            }) => {
                assert_eq!((surface_w, surface_h), (4, 4));
            }
            other => panic!("expected RectOutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn geometry_match_gates_delta_encoding() {
        let a = [0u32; 16];
        let b = [0u32; 16];
        let c = [0u32; 32];
        let sa = Surface::packed(&a, 4, 4).unwrap();
        let sb = Surface::packed(&b, 4, 4).unwrap();
        let sc = Surface::packed(&c, 8, 4).unwrap();
        assert!(sa.geometry_matches(&sb));
        assert!(!sa.geometry_matches(&sc));
    }

    #[test]
    fn argb_split_is_correct() {
        assert_eq!(argb_channels(0xAABBCCDD), (0xAA, 0xBB, 0xCC, 0xDD));
        assert_eq!(argb_channels(0x00000000), (0, 0, 0, 0));
        assert_eq!(argb_channels(0xFFFFFFFF), (0xFF, 0xFF, 0xFF, 0xFF));
    }
}
