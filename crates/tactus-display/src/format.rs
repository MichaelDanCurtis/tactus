//! Wire pixel formats.
//!
//! Every variant here was recovered from a vendor binary and verified by
//! reading the dispatch arithmetic. Formats that were *observed to exist* but
//! whose packing was not individually verified are deliberately **absent** — a
//! plausible-looking guess here corrupts a framebuffer, and the SDK must not
//! offer one.

use crate::Compression;
use tactus_core::argb_channels;

/// A display wire format.
///
/// Numeric discriminants are the vendor's own format codes with bit 6 (the
/// "compressed" flag) cleared. That flag selects run-length encoding and has
/// no effect on colour, so it is represented separately by [`Compression`]
/// rather than doubling this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PixelFormat {
    /// 16 bpp 5-6-5, red first, big-endian within the 16-bit value.
    /// Two pixels per 32-bit word; the lower-x pixel occupies the first two
    /// bytes on the wire. Vendor code `0x20` plain / `0x60` compressed.
    Rgb565 = 0x20,
    /// 16 bpp 5-6-5, blue first. Otherwise identical to [`Self::Rgb565`].
    /// Vendor code `0x21` / `0x61`.
    Bgr565 = 0x21,
    /// 32 bpp, one pixel per word, wire order `FF, R, G, B`. Source alpha is
    /// discarded and replaced by a constant `0xFF`.
    /// Vendor code `0x10` / `0x50`.
    Xrgb8888 = 0x10,
}

impl PixelFormat {
    /// The vendor format code for a frame header. Compressed frames set bit 6.
    pub const fn wire_code(self, compression: Compression) -> u8 {
        let base = self as u8;
        match compression {
            Compression::None => base,
            Compression::RunLength => base | 0x40,
        }
    }

    /// How many source pixels one 32-bit wire word carries.
    pub const fn pixels_per_unit(self) -> u32 {
        match self {
            Self::Rgb565 | Self::Bgr565 => 2,
            Self::Xrgb8888 => 1,
        }
    }

    /// Bits per pixel on the wire.
    pub const fn bits_per_pixel(self) -> u32 {
        match self {
            Self::Rgb565 | Self::Bgr565 => 16,
            Self::Xrgb8888 => 32,
        }
    }

    /// Whether this format constrains a draw rectangle's x origin and width to
    /// even values.
    ///
    /// True for 16 bpp, because two pixels share a word and the encoder cannot
    /// emit half a word. The vendor driver rejects odd values outright rather
    /// than clamping; so do we, because silently shifting a user's rectangle
    /// by a pixel is worse than refusing it.
    pub const fn requires_even_x(self) -> bool {
        self.pixels_per_unit() == 2
    }

    /// Encode one unit (one 32-bit wire word) from source pixels.
    ///
    /// `pixels` must hold exactly [`Self::pixels_per_unit`] entries in
    /// `0xAARRGGBB` form, ordered left to right. Returns the four wire bytes in
    /// transmission order, or `None` if the wrong count was supplied — keeping
    /// the function total.
    pub fn encode_unit(self, pixels: &[u32]) -> Option<[u8; 4]> {
        match self {
            Self::Xrgb8888 => {
                let [px] = pixels else { return None };
                let (_a, r, g, b) = argb_channels(*px);
                Some([0xFF, r, g, b])
            }
            Self::Rgb565 | Self::Bgr565 => {
                let [lo_x, hi_x] = pixels else { return None };
                let a = self.encode_565(*lo_x);
                let b = self.encode_565(*hi_x);
                Some([a[0], a[1], b[0], b[1]])
            }
        }
    }

    /// Pack one pixel as 5-6-5, big-endian within the 16-bit value.
    ///
    /// Byte 0 is the high byte. Transcribed from the vendor's arithmetic:
    /// `byte0 = (C5 & 0xF8) | (G >> 5)`,
    /// `byte1 = ((G << 3) & 0xE0) | (C3 >> 3)`,
    /// where `C5`/`C3` are red/blue, swapped for the BGR variant.
    #[inline]
    pub const fn encode_565(self, px: u32) -> [u8; 2] {
        let (_a, r, g, b) = argb_channels(px);
        let (first, last) = match self {
            Self::Bgr565 => (b, r),
            _ => (r, b),
        };
        [(first & 0xF8) | (g >> 5), ((g << 3) & 0xE0) | (last >> 3)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_codes_match_the_vendor_values() {
        assert_eq!(PixelFormat::Rgb565.wire_code(Compression::None), 0x20);
        assert_eq!(PixelFormat::Rgb565.wire_code(Compression::RunLength), 0x60);
        assert_eq!(PixelFormat::Bgr565.wire_code(Compression::None), 0x21);
        assert_eq!(PixelFormat::Bgr565.wire_code(Compression::RunLength), 0x61);
        assert_eq!(PixelFormat::Xrgb8888.wire_code(Compression::None), 0x10);
        assert_eq!(
            PixelFormat::Xrgb8888.wire_code(Compression::RunLength),
            0x50
        );
    }

    #[test]
    fn rgb565_packs_big_endian_within_the_word() {
        // Pure red: R5=0x1F -> 0xF800
        assert_eq!(PixelFormat::Rgb565.encode_565(0xFFFF0000), [0xF8, 0x00]);
        // Pure green: G6=0x3F -> 0x07E0
        assert_eq!(PixelFormat::Rgb565.encode_565(0xFF00FF00), [0x07, 0xE0]);
        // Pure blue: B5=0x1F -> 0x001F
        assert_eq!(PixelFormat::Rgb565.encode_565(0xFF0000FF), [0x00, 0x1F]);
        assert_eq!(PixelFormat::Rgb565.encode_565(0xFFFFFFFF), [0xFF, 0xFF]);
        assert_eq!(PixelFormat::Rgb565.encode_565(0xFF000000), [0x00, 0x00]);
    }

    #[test]
    fn bgr565_is_rgb565_with_red_and_blue_swapped() {
        let red = 0xFFFF0000;
        let blue = 0xFF0000FF;
        assert_eq!(
            PixelFormat::Bgr565.encode_565(red),
            PixelFormat::Rgb565.encode_565(blue)
        );
        assert_eq!(
            PixelFormat::Bgr565.encode_565(blue),
            PixelFormat::Rgb565.encode_565(red)
        );
        // Green is unaffected by the swap.
        let green = 0xFF00FF00;
        assert_eq!(
            PixelFormat::Bgr565.encode_565(green),
            PixelFormat::Rgb565.encode_565(green)
        );
    }

    #[test]
    fn green_spans_six_bits_not_five() {
        // 565 gives green one more bit than red or blue.
        assert_ne!(
            PixelFormat::Rgb565.encode_565(0xFF000400),
            PixelFormat::Rgb565.encode_565(0xFF000000),
            "green's 6th bit must survive"
        );
        assert_eq!(
            PixelFormat::Rgb565.encode_565(0xFF010000),
            PixelFormat::Rgb565.encode_565(0xFF000000),
            "red's low 3 bits are discarded"
        );
    }

    #[test]
    fn xrgb8888_forces_opaque_and_discards_source_alpha() {
        let transparent = PixelFormat::Xrgb8888.encode_unit(&[0x00112233]).unwrap();
        assert_eq!(transparent, [0xFF, 0x11, 0x22, 0x33]);
        let opaque = PixelFormat::Xrgb8888.encode_unit(&[0xFF112233]).unwrap();
        assert_eq!(transparent, opaque);
    }

    #[test]
    fn sixteen_bpp_unit_places_lower_x_pixel_first() {
        let unit = PixelFormat::Rgb565
            .encode_unit(&[0xFFFF0000, 0xFF0000FF])
            .unwrap();
        assert_eq!(&unit[0..2], &[0xF8, 0x00], "lower-x pixel first");
        assert_eq!(&unit[2..4], &[0x00, 0x1F]);
    }

    #[test]
    fn encode_unit_is_total_on_wrong_pixel_count() {
        assert!(PixelFormat::Rgb565.encode_unit(&[0]).is_none());
        assert!(PixelFormat::Rgb565.encode_unit(&[0, 0, 0]).is_none());
        assert!(PixelFormat::Xrgb8888.encode_unit(&[]).is_none());
        assert!(PixelFormat::Xrgb8888.encode_unit(&[0, 0]).is_none());
    }

    #[test]
    fn unit_geometry_is_self_consistent() {
        for f in [
            PixelFormat::Rgb565,
            PixelFormat::Bgr565,
            PixelFormat::Xrgb8888,
        ] {
            assert_eq!(
                f.pixels_per_unit() * f.bits_per_pixel(),
                32,
                "a unit is always one 32-bit word"
            );
            assert_eq!(f.requires_even_x(), f.pixels_per_unit() == 2);
        }
    }
}
