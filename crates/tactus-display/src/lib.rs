//! Display support for control-surface hardware.
//!
//! # Two display models, deliberately
//!
//! Control surfaces do not share one display architecture, and an SDK that
//! assumes they do will be wrong for half its devices. Two models are
//! supported, both established by reverse engineering and recorded in
//! `docs/protocol/`:
//!
//! - [`DisplayModel::Pixels`] — the **host composes a raster image** and
//!   transmits it. Used by the Kontrol S-series **MK2**, Maschine, and the
//!   Traktor units. The wire format is fully documented and implemented in
//!   [`encode`].
//!
//! - [`DisplayModel::Pages`] — the **device renders its own UI** from typed
//!   view-models. Used by the Kontrol S-series **MK3**. The host selects one of
//!   four pages and supplies structured content; it never composes pixels, and
//!   consequently never needs to know the panel's resolution.
//!
//! Treating these as one abstraction was tried and rejected. A pixel API
//! cannot express "show the browser page with these list rows", and a page API
//! cannot express "set this rectangle of pixels". The two arms stay distinct.
//!
//! # What this means for the MK3
//!
//! There is **no framebuffer to reverse-engineer** on MK3, and no geometry to
//! recover. That conclusion rests on converging evidence, not absence of
//! effort: the vendor's device table records all three MK3 keyboards with a
//! `0xff`/0×0 sentinel; no MK3 class exists in the vendor's pixel stack; the
//! HID geometry report the pixel stack uses is *not implemented* by real MK3
//! hardware; and the MK3's own protocol vocabulary contains no pixel, bitmap,
//! stride or resolution field at all, while it does contain `renderer_version`
//! — a property of the device, queried by the host.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod encode;
pub mod format;
pub mod page;

pub use encode::{Frame, encode_frame, encode_frame_to_vec, max_encoded_len};
pub use format::PixelFormat;
pub use page::{Page, PageMethod};

use thiserror::Error;

/// Whether a pixel payload is run-length encoded.
///
/// Selected by bit 6 of the vendor format code. It affects only the payload
/// encoding, never the colour layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// Units written back to back with no opcodes.
    None,
    /// Literal/repeat/skip run-length stream. Required for delta updates.
    #[default]
    RunLength,
}

/// How a device's display is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayModel {
    /// Host composes a raster image and transmits it.
    Pixels {
        /// Panel width in pixels.
        width: u32,
        /// Panel height in pixels.
        height: u32,
        /// The panel's wire format.
        format: PixelFormat,
        /// How many displays the device exposes.
        count: u8,
    },
    /// Device renders its own UI from structured view-models supplied by the
    /// host. The host never composes pixels, so no geometry is carried here —
    /// its absence is the point, not an omission.
    Pages,
}

impl DisplayModel {
    /// Whether the host is responsible for composing pixels.
    pub const fn is_host_rendered(&self) -> bool {
        matches!(self, Self::Pixels { .. })
    }
}

/// Errors produced while encoding a display frame.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DisplayError {
    /// A core geometry or surface error.
    #[error(transparent)]
    Core(#[from] tactus_core::CoreError),

    /// The draw rectangle covers no pixels.
    #[error("draw rectangle is empty")]
    EmptyRect,

    /// A 16 bpp format was given an odd x origin or width.
    ///
    /// Two pixels share a 32-bit word, so a draw cannot begin or end mid-word.
    /// Rejected rather than clamped: silently moving a caller's rectangle by a
    /// pixel is a worse failure than refusing it.
    #[error("16 bpp formats need even x and width, got x={x} w={w}")]
    OddGeometry {
        /// The supplied x origin.
        x: u32,
        /// The supplied width.
        w: u32,
    },

    /// The rectangle exceeds the 16-bit coordinates the wire format carries.
    #[error("rect {rect:?} exceeds the 16-bit wire coordinate range")]
    RectTooLargeForWire {
        /// The offending rectangle.
        rect: tactus_core::Rect,
    },

    /// A delta was requested against a surface of different geometry.
    #[error("previous frame geometry differs from the current frame")]
    PreviousGeometryMismatch,

    /// The output buffer could not hold the encoded frame.
    #[error("output buffer holds {got} bytes, need up to {need}")]
    OutputTooSmall {
        /// Bytes available.
        got: usize,
        /// Bytes required in the worst case.
        need: usize,
    },

    /// A source pixel inside the draw rectangle could not be read.
    ///
    /// Indicates an internal inconsistency, since rectangles are validated
    /// against the surface before encoding begins.
    #[error("source pixel ({x},{y}) is unreadable")]
    SourcePixelMissing {
        /// Column.
        x: u32,
        /// Row.
        y: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_defaults_to_run_length() {
        // Delta updates require it, and it is what the vendor driver uses.
        assert_eq!(Compression::default(), Compression::RunLength);
    }

    #[test]
    fn host_rendered_only_for_the_pixel_model() {
        let pixels = DisplayModel::Pixels {
            width: 480,
            height: 272,
            format: PixelFormat::Rgb565,
            count: 2,
        };
        assert!(pixels.is_host_rendered());
        assert!(!DisplayModel::Pages.is_host_rendered());
    }

    #[test]
    fn the_page_model_carries_no_geometry() {
        // Guards the architectural claim: if someone later adds width/height
        // to Pages, this stops compiling and they have to justify it.
        match DisplayModel::Pages {
            DisplayModel::Pages => {}
            DisplayModel::Pixels { .. } => unreachable!(),
        }
    }
}
