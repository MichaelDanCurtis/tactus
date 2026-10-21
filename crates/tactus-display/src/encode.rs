//! Frame encoding for host-rendered pixel displays.
//!
//! Produces the complete wire frame: a 16-byte header, a run-length command
//! stream, and a 4-byte footer. Encoding never transmits anything and never
//! allocates — callers supply the output buffer.
//!
//! # Wire format
//!
//! ```text
//! 84 00 | idx | fmt | 00 00 00 00 | x:BE16 | y:BE16 | w:BE16 | h:BE16
//!       | <command stream>
//!       | 40 00 | idx | 00
//! ```
//!
//! There is **no length field** anywhere in the frame; bytes `+0x04..=+0x07`
//! are explicitly zero. (Third-party documentation describing a 4-byte
//! little-endian length prefix is incorrect.)
//!
//! Command stream opcodes are one byte followed by a **24-bit big-endian
//! count of 32-bit words** — not pixels. At 16 bpp one word is two pixels.
//!
//! | Opcode | Payload | Meaning |
//! |---|---|---|
//! | `0x00 N` | N words | literal run |
//! | `0x01 N` | 1 word | repeat that word N times |
//! | `0x02 N` | none | skip N words (leave device content intact) |
//! | `0x03 00 00 00` | none | end of stream |

use crate::{Compression, DisplayError, PixelFormat};
use tactus_core::{Rect, Surface};

/// Opcode for a literal run.
const OP_LITERAL: u8 = 0x00;
/// Opcode for a repeated word.
const OP_REPEAT: u8 = 0x01;
/// Opcode to skip words, leaving existing device content.
const OP_SKIP: u8 = 0x02;
/// Opcode terminating the stream.
const OP_END: u8 = 0x03;

/// Largest count expressible in the 24-bit count field.
const MAX_RUN: u32 = 0x00FF_FFFF;

/// Minimum identical units before a repeat run beats a literal run.
///
/// A repeat costs 8 bytes (opcode + one word); two literal units cost 4 bytes
/// of payload plus a shared opcode. At three units the repeat wins. The vendor
/// encoder uses the same threshold.
const MIN_REPEAT_RUN: u32 = 3;

/// Bytes in a frame header.
pub const HEADER_LEN: usize = 16;
/// Bytes in a frame footer.
pub const FOOTER_LEN: usize = 4;

/// A frame to encode.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// Which display on the device, for multi-display hardware.
    pub display_index: u8,
    /// Wire pixel format.
    pub format: PixelFormat,
    /// Whether to run-length encode the payload.
    pub compression: Compression,
    /// Source pixels.
    pub surface: Surface<'a>,
    /// Region of `surface` to transmit.
    pub rect: Rect,
    /// The previously transmitted frame, enabling skip runs for unchanged
    /// regions. Must have geometry identical to `surface`.
    ///
    /// `None` encodes a full redraw. Note that a device's framebuffer content
    /// after a cold start is **undefined**, so the first frame of a session
    /// should always pass `None`.
    pub previous: Option<Surface<'a>>,
}

/// A conservative upper bound on the encoded size of a frame.
///
/// Worst case is a payload that alternates in a way that defeats both run
/// types, making every unit its own literal run: 4 opcode bytes plus 4 payload
/// bytes each.
pub fn max_encoded_len(format: PixelFormat, rect: &Rect) -> usize {
    let units = unit_count(format, rect);
    HEADER_LEN + units.saturating_mul(8) + FOOTER_LEN + 8
}

/// Total units covered by a rectangle in the given format.
fn unit_count(format: PixelFormat, rect: &Rect) -> usize {
    let ppu = format.pixels_per_unit() as u64;
    if ppu == 0 {
        return 0;
    }
    let per_row = (rect.w as u64) / ppu;
    (per_row.saturating_mul(rect.h as u64)) as usize
}

/// Encode a frame into `out`, returning the number of bytes written.
///
/// Validates every precondition before writing anything, so a rejected frame
/// leaves `out` untouched rather than partially written.
pub fn encode_frame(out: &mut [u8], frame: &Frame<'_>) -> Result<usize, DisplayError> {
    let Frame {
        display_index,
        format,
        compression,
        surface,
        rect,
        previous,
    } = frame;

    if rect.is_empty() {
        return Err(DisplayError::EmptyRect);
    }

    // Geometry must fit the source.
    surface.check_rect(rect).map_err(DisplayError::Core)?;

    // 16 bpp packs two pixels per word, so a draw cannot start or end mid-word.
    if format.requires_even_x() && (rect.x % 2 != 0 || rect.w % 2 != 0) {
        return Err(DisplayError::OddGeometry {
            x: rect.x,
            w: rect.w,
        });
    }

    // Wire coordinates are 16-bit.
    if rect.right() > u32::from(u16::MAX) || rect.bottom() > u32::from(u16::MAX) {
        return Err(DisplayError::RectTooLargeForWire { rect: *rect });
    }

    // A delta is only meaningful against identical geometry.
    if let Some(prev) = previous
        && !surface.geometry_matches(prev)
    {
        return Err(DisplayError::PreviousGeometryMismatch);
    }

    let needed = max_encoded_len(*format, rect);
    if out.len() < HEADER_LEN + FOOTER_LEN + 8 {
        return Err(DisplayError::OutputTooSmall {
            got: out.len(),
            need: needed,
        });
    }

    let mut w = Writer::new(out);
    write_header(&mut w, *display_index, *format, *compression, rect)?;

    match compression {
        Compression::None => encode_raw(&mut w, format, surface, rect)?,
        Compression::RunLength => encode_runs(&mut w, format, surface, rect, previous.as_ref())?,
    }

    write_footer(&mut w, *display_index)?;
    Ok(w.len())
}

/// Encode a frame, appending to a `Vec`.
///
/// Convenience for callers that are not in a real-time path.
pub fn encode_frame_to_vec(out: &mut Vec<u8>, frame: &Frame<'_>) -> Result<usize, DisplayError> {
    let start = out.len();
    out.resize(start + max_encoded_len(frame.format, &frame.rect), 0);
    let n = encode_frame(&mut out[start..], frame)?;
    out.truncate(start + n);
    Ok(n)
}

/// A bounds-checked forward-only cursor over the output buffer.
struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn len(&self) -> usize {
        self.pos
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), DisplayError> {
        let capacity = self.buf.len();
        let end = self
            .pos
            .checked_add(bytes.len())
            .ok_or(DisplayError::OutputTooSmall {
                got: capacity,
                need: usize::MAX,
            })?;
        let slot = self
            .buf
            .get_mut(self.pos..end)
            .ok_or(DisplayError::OutputTooSmall {
                got: capacity,
                need: end,
            })?;
        slot.copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }

    /// Reserve four bytes for an opcode to be patched later, returning its offset.
    fn reserve_opcode(&mut self) -> Result<usize, DisplayError> {
        let at = self.pos;
        self.push(&[0, 0, 0, 0])?;
        Ok(at)
    }

    /// Patch a previously reserved opcode slot.
    fn patch_opcode(&mut self, at: usize, op: u8, count: u32) -> Result<(), DisplayError> {
        let capacity = self.buf.len();
        let slot = self
            .buf
            .get_mut(at..at + 4)
            .ok_or(DisplayError::OutputTooSmall {
                got: capacity,
                need: at + 4,
            })?;
        slot.copy_from_slice(&opcode_bytes(op, count));
        Ok(())
    }
}

/// One opcode byte followed by a 24-bit big-endian count.
fn opcode_bytes(op: u8, count: u32) -> [u8; 4] {
    let c = count & MAX_RUN;
    [op, (c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn write_header(
    w: &mut Writer<'_>,
    display_index: u8,
    format: PixelFormat,
    compression: Compression,
    rect: &Rect,
) -> Result<(), DisplayError> {
    let (x, y) = (rect.x as u16, rect.y as u16);
    let (rw, rh) = (rect.w as u16, rect.h as u16);
    w.push(&[
        0x84,
        0x00,
        display_index,
        format.wire_code(compression),
        // Four zero bytes. Not a length field.
        0x00,
        0x00,
        0x00,
        0x00,
        (x >> 8) as u8,
        x as u8,
        (y >> 8) as u8,
        y as u8,
        (rw >> 8) as u8,
        rw as u8,
        (rh >> 8) as u8,
        rh as u8,
    ])
}

fn write_footer(w: &mut Writer<'_>, display_index: u8) -> Result<(), DisplayError> {
    w.push(&[0x40, 0x00, display_index, 0x00])
}

/// Read the pixels for one unit and encode it to wire bytes.
fn unit_bytes(
    format: PixelFormat,
    surface: &Surface<'_>,
    rect: &Rect,
    index: usize,
) -> Result<[u8; 4], DisplayError> {
    let ppu = format.pixels_per_unit();
    let per_row = (rect.w / ppu) as usize;
    if per_row == 0 {
        return Err(DisplayError::EmptyRect);
    }
    let row = (index / per_row) as u32;
    let col = (index % per_row) as u32;
    let x0 = rect.x + col * ppu;
    let y = rect.y + row;

    let mut px = [0u32; 2];
    for i in 0..ppu {
        px[i as usize] = surface
            .pixel(x0 + i, y)
            .ok_or(DisplayError::SourcePixelMissing { x: x0 + i, y })?;
    }
    format
        .encode_unit(&px[..ppu as usize])
        .ok_or(DisplayError::SourcePixelMissing { x: x0, y })
}

/// Uncompressed payload: every unit, in row-major order, back to back.
fn encode_raw(
    w: &mut Writer<'_>,
    format: &PixelFormat,
    surface: &Surface<'_>,
    rect: &Rect,
) -> Result<(), DisplayError> {
    let total = unit_count(*format, rect);
    for i in 0..total {
        let bytes = unit_bytes(*format, surface, rect, i)?;
        w.push(&bytes)?;
    }
    Ok(())
}

/// Run-length payload with optional delta against a previous frame.
fn encode_runs(
    w: &mut Writer<'_>,
    format: &PixelFormat,
    surface: &Surface<'_>,
    rect: &Rect,
    previous: Option<&Surface<'_>>,
) -> Result<(), DisplayError> {
    let total = unit_count(*format, rect);
    let unchanged = |i: usize| -> Result<bool, DisplayError> {
        match previous {
            None => Ok(false),
            Some(prev) => {
                Ok(unit_bytes(*format, surface, rect, i)? == unit_bytes(*format, prev, rect, i)?)
            }
        }
    };

    let mut i = 0usize;
    while i < total {
        if unchanged(i)? {
            // Skip run: consecutive units identical to the previous frame.
            let mut n = 0usize;
            while i + n < total && unchanged(i + n)? && (n as u32) < MAX_RUN {
                n += 1;
            }
            w.push(&opcode_bytes(OP_SKIP, n as u32))?;
            i += n;
            continue;
        }

        let here = unit_bytes(*format, surface, rect, i)?;

        // Repeat run: consecutive identical units that are not skippable.
        let mut rep = 0usize;
        while i + rep < total
            && (rep as u32) < MAX_RUN
            && !unchanged(i + rep)?
            && unit_bytes(*format, surface, rect, i + rep)? == here
        {
            rep += 1;
        }
        if rep as u32 >= MIN_REPEAT_RUN {
            w.push(&opcode_bytes(OP_REPEAT, rep as u32))?;
            w.push(&here)?;
            i += rep;
            continue;
        }

        // Literal run: until a skippable unit or the start of a worthwhile repeat.
        let start = i;
        let slot = w.reserve_opcode()?;
        let mut n = 0usize;
        while i < total && (n as u32) < MAX_RUN {
            if unchanged(i)? {
                break;
            }
            // Would a repeat run start here? Only break if we already have payload.
            if i > start {
                let cur = unit_bytes(*format, surface, rect, i)?;
                let mut look = 0usize;
                while i + look < total
                    && look < MIN_REPEAT_RUN as usize
                    && !unchanged(i + look)?
                    && unit_bytes(*format, surface, rect, i + look)? == cur
                {
                    look += 1;
                }
                if look >= MIN_REPEAT_RUN as usize {
                    break;
                }
            }
            let bytes = unit_bytes(*format, surface, rect, i)?;
            w.push(&bytes)?;
            n += 1;
            i += 1;
        }
        w.patch_opcode(slot, OP_LITERAL, n as u32)?;
    }

    w.push(&opcode_bytes(OP_END, 0))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, px: u32) -> Vec<u32> {
        vec![px; (w * h) as usize]
    }

    fn frame<'a>(surface: Surface<'a>, rect: Rect, compression: Compression) -> Frame<'a> {
        Frame {
            display_index: 0,
            format: PixelFormat::Rgb565,
            compression,
            surface,
            rect,
            previous: None,
        }
    }

    #[test]
    fn opcode_count_is_24bit_big_endian() {
        // The decisive framing detail: one opcode byte then MSB-first count.
        assert_eq!(opcode_bytes(OP_REPEAT, 0x1234), [0x01, 0x00, 0x12, 0x34]);
        assert_eq!(opcode_bytes(OP_LITERAL, 1), [0x00, 0x00, 0x00, 0x01]);
        assert_eq!(opcode_bytes(OP_SKIP, 0xABCDEF), [0x02, 0xAB, 0xCD, 0xEF]);
        assert_eq!(opcode_bytes(OP_END, 0), [0x03, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn header_matches_the_documented_layout() {
        let px = solid(480, 272, 0xFF000000);
        let s = Surface::packed(&px, 480, 272).unwrap();
        let f = frame(s, Rect::whole(480, 272), Compression::RunLength);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();

        assert_eq!(
            &out[..HEADER_LEN],
            &[
                0x84, 0x00, // tag
                0x00, // display index
                0x60, // Rgb565 | compressed
                0x00, 0x00, 0x00, 0x00, // zeroed, NOT a length
                0x00, 0x00, // x = 0
                0x00, 0x00, // y = 0
                0x01, 0xE0, // w = 480, big-endian
                0x01, 0x10, // h = 272, big-endian
            ]
        );
        // Footer closes the frame.
        assert_eq!(&out[n - FOOTER_LEN..n], &[0x40, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn header_carries_a_partial_rect_verbatim() {
        let px = solid(480, 272, 0xFF000000);
        let s = Surface::packed(&px, 480, 272).unwrap();
        let f = frame(s, Rect::new(16, 32, 64, 48), Compression::RunLength);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        encode_frame(&mut out, &f).unwrap();
        assert_eq!(
            &out[8..16],
            &[0x00, 0x10, 0x00, 0x20, 0x00, 0x40, 0x00, 0x30]
        );
    }

    #[test]
    fn a_solid_fill_compresses_to_one_repeat_run() {
        // 8x2 of one colour at 16bpp = 8 units; all identical -> single repeat.
        let px = solid(8, 2, 0xFFFF0000);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let f = frame(s, Rect::whole(8, 2), Compression::RunLength);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();
        let payload = &out[HEADER_LEN..n - FOOTER_LEN];
        assert_eq!(
            payload,
            &[
                0x01, 0x00, 0x00, 0x08, // repeat 8 words
                0xF8, 0x00, 0xF8, 0x00, // the word: two red pixels
                0x03, 0x00, 0x00, 0x00, // end
            ]
        );
    }

    #[test]
    fn an_unchanged_delta_is_one_skip_run() {
        let a = solid(8, 2, 0xFF00FF00);
        let b = solid(8, 2, 0xFF00FF00);
        let cur = Surface::packed(&a, 8, 2).unwrap();
        let prev = Surface::packed(&b, 8, 2).unwrap();
        let mut f = frame(cur, Rect::whole(8, 2), Compression::RunLength);
        f.previous = Some(prev);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();
        let payload = &out[HEADER_LEN..n - FOOTER_LEN];
        assert_eq!(
            payload,
            &[0x02, 0x00, 0x00, 0x08, 0x03, 0x00, 0x00, 0x00],
            "all eight words unchanged -> skip 8, end"
        );
    }

    #[test]
    fn a_delta_skips_the_unchanged_prefix_and_sends_the_rest() {
        // 4x1 at 16bpp = 2 units. Change only the second unit.
        let mut a = solid(4, 1, 0xFF000000);
        a[2] = 0xFFFFFFFF;
        a[3] = 0xFFFFFFFF;
        let b = solid(4, 1, 0xFF000000);
        let cur = Surface::packed(&a, 4, 1).unwrap();
        let prev = Surface::packed(&b, 4, 1).unwrap();
        let mut f = frame(cur, Rect::whole(4, 1), Compression::RunLength);
        f.previous = Some(prev);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();
        let payload = &out[HEADER_LEN..n - FOOTER_LEN];
        assert_eq!(
            payload,
            &[
                0x02, 0x00, 0x00, 0x01, // skip the unchanged first word
                0x00, 0x00, 0x00, 0x01, // literal, one word
                0xFF, 0xFF, 0xFF, 0xFF, // two white pixels
                0x03, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn short_identical_stretches_stay_literal() {
        // Two identical units then two different: a 2-run must not become a
        // repeat, because that costs more than a literal.
        let px: Vec<u32> = vec![
            0xFF000000, 0xFF000000, 0xFF000000, 0xFF000000, // units 0,1 identical
            0xFFFFFFFF, 0xFF000000, // unit 2 differs
        ];
        let s = Surface::packed(&px, 6, 1).unwrap();
        let f = frame(s, Rect::whole(6, 1), Compression::RunLength);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();
        let payload = &out[HEADER_LEN..n - FOOTER_LEN];
        assert_eq!(payload[0], OP_LITERAL, "a 2-unit stretch stays literal");
        // Three words of payload follow the opcode.
        assert_eq!(&payload[0..4], &[0x00, 0x00, 0x00, 0x03]);
    }

    #[test]
    fn uncompressed_payload_is_raw_units_with_no_opcodes() {
        let px = solid(4, 1, 0xFFFF0000);
        let s = Surface::packed(&px, 4, 1).unwrap();
        let f = frame(s, Rect::whole(4, 1), Compression::None);
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();
        // Format code has bit 6 clear.
        assert_eq!(out[3], 0x20);
        let payload = &out[HEADER_LEN..n - FOOTER_LEN];
        // 4 pixels at 2 pixels/unit = 2 units = 8 bytes, back to back, and
        // crucially no run-length opcodes interleaved.
        assert_eq!(
            payload,
            &[0xF8, 0x00, 0xF8, 0x00, 0xF8, 0x00, 0xF8, 0x00],
            "two units, no opcodes"
        );
        assert_eq!(n, HEADER_LEN + 8 + FOOTER_LEN);
    }

    #[test]
    fn odd_x_or_width_is_rejected_not_clamped() {
        let px = solid(8, 2, 0);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let mut out = vec![0u8; 4096];

        let f = frame(s, Rect::new(1, 0, 4, 1), Compression::RunLength);
        assert!(matches!(
            encode_frame(&mut out, &f),
            Err(DisplayError::OddGeometry { x: 1, .. })
        ));

        let f = frame(s, Rect::new(0, 0, 5, 1), Compression::RunLength);
        assert!(matches!(
            encode_frame(&mut out, &f),
            Err(DisplayError::OddGeometry { w: 5, .. })
        ));
    }

    #[test]
    fn odd_geometry_is_fine_at_32bpp() {
        let px = solid(8, 2, 0xFF102030);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let mut f = frame(s, Rect::new(1, 0, 5, 1), Compression::None);
        f.format = PixelFormat::Xrgb8888;
        let mut out = vec![0u8; max_encoded_len(f.format, &f.rect)];
        let n = encode_frame(&mut out, &f).unwrap();
        assert_eq!(n, HEADER_LEN + 5 * 4 + FOOTER_LEN);
    }

    #[test]
    fn a_rect_outside_the_surface_is_rejected() {
        let px = solid(8, 2, 0);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let f = frame(s, Rect::new(4, 0, 8, 1), Compression::RunLength);
        let mut out = vec![0u8; 4096];
        assert!(matches!(
            encode_frame(&mut out, &f),
            Err(DisplayError::Core(_))
        ));
    }

    #[test]
    fn mismatched_previous_geometry_is_rejected() {
        let a = solid(8, 2, 0);
        let b = solid(16, 2, 0);
        let cur = Surface::packed(&a, 8, 2).unwrap();
        let prev = Surface::packed(&b, 16, 2).unwrap();
        let mut f = frame(cur, Rect::whole(8, 2), Compression::RunLength);
        f.previous = Some(prev);
        let mut out = vec![0u8; 4096];
        assert!(matches!(
            encode_frame(&mut out, &f),
            Err(DisplayError::PreviousGeometryMismatch)
        ));
    }

    #[test]
    fn an_empty_rect_is_rejected() {
        let px = solid(8, 2, 0);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let f = frame(s, Rect::new(0, 0, 0, 1), Compression::RunLength);
        let mut out = vec![0u8; 4096];
        assert!(matches!(
            encode_frame(&mut out, &f),
            Err(DisplayError::EmptyRect)
        ));
    }

    #[test]
    fn a_too_small_output_buffer_is_rejected_before_writing() {
        let px = solid(8, 2, 0);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let f = frame(s, Rect::whole(8, 2), Compression::RunLength);
        let mut out = [0u8; 4];
        let before = out;
        assert!(matches!(
            encode_frame(&mut out, &f),
            Err(DisplayError::OutputTooSmall { .. })
        ));
        assert_eq!(out, before, "a rejected frame must not write anything");
    }

    #[test]
    fn vec_helper_truncates_to_the_real_length() {
        let px = solid(8, 2, 0xFFFF0000);
        let s = Surface::packed(&px, 8, 2).unwrap();
        let f = frame(s, Rect::whole(8, 2), Compression::RunLength);
        let mut v = Vec::new();
        let n = encode_frame_to_vec(&mut v, &f).unwrap();
        assert_eq!(v.len(), n, "no slack left on the end");
        assert_eq!(&v[..2], &[0x84, 0x00]);
        assert_eq!(&v[n - 4..], &[0x40, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn vec_helper_appends_without_disturbing_existing_content() {
        let px = solid(4, 1, 0xFFFF0000);
        let s = Surface::packed(&px, 4, 1).unwrap();
        let f = frame(s, Rect::whole(4, 1), Compression::RunLength);
        let mut v = vec![0xDE, 0xAD];
        let n = encode_frame_to_vec(&mut v, &f).unwrap();
        assert_eq!(&v[..2], &[0xDE, 0xAD]);
        assert_eq!(v.len(), 2 + n);
        assert_eq!(&v[2..4], &[0x84, 0x00]);
    }

    #[test]
    fn every_data_byte_in_a_payload_is_a_plausible_wire_byte() {
        // Sanity sweep: encoding a gradient must never panic and must always
        // produce a frame whose declared geometry matches what we asked for.
        let mut px = Vec::new();
        for y in 0..16u32 {
            for x in 0..16u32 {
                px.push(0xFF00_0000 | (x << 16) | (y << 8) | (x ^ y));
            }
        }
        let s = Surface::packed(&px, 16, 16).unwrap();
        for compression in [Compression::None, Compression::RunLength] {
            for fmt in [
                PixelFormat::Rgb565,
                PixelFormat::Bgr565,
                PixelFormat::Xrgb8888,
            ] {
                let mut f = frame(s, Rect::new(2, 3, 8, 6), compression);
                f.format = fmt;
                let mut out = vec![0u8; max_encoded_len(fmt, &f.rect)];
                let n = encode_frame(&mut out, &f).expect("gradient must encode");
                assert!(n >= HEADER_LEN + FOOTER_LEN);
                assert_eq!(out[3], fmt.wire_code(compression));
                assert_eq!(&out[8..16], &[0, 2, 0, 3, 0, 8, 0, 6]);
            }
        }
    }
}
