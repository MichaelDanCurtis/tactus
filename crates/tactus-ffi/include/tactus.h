/* Generated from Rust by cbindgen. Do not edit by hand. */

#ifndef TACTUS_H
#define TACTUS_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Major ABI version. Changes when an existing layout or signature changes.
 */
#define ABI_VERSION_MAJOR 0

/**
 * Minor ABI version. Changes when functionality is added compatibly.
 */
#define ABI_VERSION_MINOR 1

/**
 * Status returned by every exported function.
 */
enum TactusStatus
#if __STDC_VERSION__ >= 202311L
  : int32_t
#endif // __STDC_VERSION__ >= 202311L
 {
  /**
   * Success.
   */
  TactusStatus_Ok = 0,
  /**
   * A required pointer argument was null.
   */
  TactusStatus_NullArgument = -1,
  /**
   * A buffer was too small for the result.
   */
  TactusStatus_BufferTooSmall = -2,
  /**
   * An argument was outside its permitted range.
   */
  TactusStatus_OutOfRange = -3,
  /**
   * An argument had an invalid or unrecognised value.
   */
  TactusStatus_InvalidArgument = -4,
  /**
   * Text contained a character the device cannot represent.
   */
  TactusStatus_NonAsciiText = -5,
  /**
   * Geometry was invalid for the requested operation.
   */
  TactusStatus_InvalidGeometry = -6,
  /**
   * An internal error, including a caught panic. Should never be seen.
   */
  TactusStatus_Internal = -100,
};
#if __STDC_VERSION__ >= 202311L
typedef enum TactusStatus TactusStatus;
#else
typedef int32_t TactusStatus;
#endif // __STDC_VERSION__ >= 202311L

/**
 * A keyboard model.
 */
enum TactusModel
#if __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // __STDC_VERSION__ >= 202311L
 {
  /**
   * 49-key.
   */
  TactusModel_S49 = 49,
  /**
   * 61-key.
   */
  TactusModel_S61 = 61,
  /**
   * 88-key.
   */
  TactusModel_S88 = 88,
};
#if __STDC_VERSION__ >= 202311L
typedef enum TactusModel TactusModel;
#else
typedef uint32_t TactusModel;
#endif // __STDC_VERSION__ >= 202311L

/**
 * Which variant of [`TactusInputReport`] is populated.
 */
enum TactusInputKind
#if __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // __STDC_VERSION__ >= 202311L
 {
  /**
   * Report `0x01`: `bits[0..40]`, `nibbles`.
   */
  TactusInputKind_Controls = 0,
  /**
   * Report `0x02`: `bits[0..8]` leading, `continuous`, `wide`,
   * `bits[8..24]` trailing.
   */
  TactusInputKind_Analog = 1,
  /**
   * Report `0x03`: `bits[0..8]`, `full_range`, `eleven_bit`, `tail`.
   */
  TactusInputKind_Wheels = 2,
  /**
   * A report this SDK does not yet model. Only `report_id` is meaningful.
   */
  TactusInputKind_Unmodelled = 3,
};
#if __STDC_VERSION__ >= 202311L
typedef enum TactusInputKind TactusInputKind;
#else
typedef uint32_t TactusInputKind;
#endif // __STDC_VERSION__ >= 202311L

/**
 * A page on a device-rendered display.
 *
 * Values are the device's own wire bytes. Used by MK3, which renders its own
 * UI — such a device is never sent pixels.
 */
enum TactusPage
#if __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // __STDC_VERSION__ >= 202311L
 {
  /**
   * Sound browser.
   */
  TactusPage_Browser = 0,
  /**
   * Mixer.
   */
  TactusPage_Mixer = 1,
  /**
   * Plugin parameters.
   */
  TactusPage_Parameter = 2,
  /**
   * Smart play.
   */
  TactusPage_SmartPlay = 3,
};
#if __STDC_VERSION__ >= 202311L
typedef enum TactusPage TactusPage;
#else
typedef uint32_t TactusPage;
#endif // __STDC_VERSION__ >= 202311L

/**
 * A display wire format, for host-rendered (pixel) devices.
 *
 * Only formats whose packing has been individually verified are offered.
 */
enum TactusPixelFormat
#if __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // __STDC_VERSION__ >= 202311L
 {
  /**
   * 16 bpp 5-6-5, red first, big-endian in the word.
   */
  TactusPixelFormat_Rgb565 = 0,
  /**
   * 16 bpp 5-6-5, blue first.
   */
  TactusPixelFormat_Bgr565 = 1,
  /**
   * 32 bpp, wire order `FF, R, G, B`.
   */
  TactusPixelFormat_Xrgb8888 = 2,
};
#if __STDC_VERSION__ >= 202311L
typedef enum TactusPixelFormat TactusPixelFormat;
#else
typedef uint32_t TactusPixelFormat;
#endif // __STDC_VERSION__ >= 202311L

/**
 * A decoded HID input report.
 *
 * A flat tagged struct rather than a union, so every FFI consumer can read it
 * without layout games. `kind` says which fields are meaningful; the rest are
 * zeroed.
 *
 * **Field indices are not control names.** The device's descriptor declares
 * how many controls of what width, but not which physical control each index
 * is — it uses one opaque vendor usage page. Nothing here claims "index 3 is
 * Play", because guessing would be worse than silence.
 */
typedef struct TactusInputReport {
  /**
   * Which fields below are populated.
   */
  TactusInputKind kind;
  /**
   * The HID report id this came from.
   */
  uint8_t report_id;
  /**
   * Single-bit controls, one byte per control, `0` or `1`.
   */
  uint8_t bits[40];
  /**
   * How many entries of `bits` are meaningful.
   */
  uint32_t bit_count;
  /**
   * 4-bit controls (report `0x01`), each `0..=15`.
   */
  uint8_t nibbles[2];
  /**
   * Continuous controls (report `0x02`), each `0..=999`.
   */
  uint16_t continuous[8];
  /**
   * Wider controls (report `0x02`), each `0..=4095`.
   */
  uint16_t wide[4];
  /**
   * Full-range controls (report `0x03`), each `0..=65535`.
   */
  uint16_t full_range[3];
  /**
   * 11-bit controls (report `0x03`), each `0..=2047`.
   */
  uint16_t eleven_bit[2];
  /**
   * Trailing byte (report `0x03`).
   */
  uint8_t tail;
} TactusInputReport;

/**
 * The ABI version, packed as `(major << 16) | minor`.
 *
 * Call this before trusting any struct layout in this header.
 */
uint32_t tactus_abi_version(void);

/**
 * A human-readable status name, as a static NUL-terminated C string.
 *
 * Never returns null; an unrecognised code yields `"unknown"`.
 */
const char *tactus_status_name(int32_t status);

/**
 * The USB vendor id these devices use.
 */
uint16_t tactus_vendor_id(void);

/**
 * Identify a model from its USB product id.
 *
 * # Safety
 * `out_model` must be a valid writable pointer to a `TactusModel`, or null.
 */
TactusStatus tactus_model_from_product_id(uint16_t product_id, TactusModel *out_model);

/**
 * Number of keys on a model. Also the exact light-guide payload length.
 */
uint32_t tactus_model_keys(TactusModel model);

/**
 * Highest legal light-guide value.
 *
 * The device declares report `0x82` with a logical range of `0..=127`, not
 * `0..=255` — consistent with a palette index rather than an intensity.
 */
uint8_t tactus_light_guide_max(void);

/**
 * Encode a light-guide HID output report.
 *
 * `values` must hold exactly [`tactus_model_keys`] entries, each
 * `<= `[`tactus_light_guide_max`]. Writes the report id followed by the
 * payload into `out`, and stores the byte count in `out_len`.
 *
 * Out-of-range values are **rejected**, not masked: masking would send a
 * wrong colour while hiding the caller's bug.
 *
 * # Safety
 * `values` must point to `values_len` readable bytes. `out` must point to
 * `out_cap` writable bytes. `out_len` must be writable.
 */
TactusStatus tactus_encode_light_guide(TactusModel model,
                                       const uint8_t *values,
                                       uintptr_t values_len,
                                       uint8_t *out,
                                       uintptr_t out_cap,
                                       uintptr_t *out_len);

/**
 * Decode a complete HID input report, **including** its leading report id.
 *
 * # Safety
 * `report` must point to `report_len` readable bytes. `out` must be writable.
 */
TactusStatus tactus_decode_input(const uint8_t *report,
                                 uintptr_t report_len,
                                 struct TactusInputReport *out);

/**
 * Decode a relative encoder value into a signed delta, `-64..=63`.
 *
 * Values `0x01..=0x3F` are positive; `0x41..=0x7F` are negative, read as
 * 7-bit two's complement.
 */
int8_t tactus_daw_decode_relative(uint8_t value);

/**
 * Encode a DAW-port SysEx command with a raw payload.
 *
 * Every payload byte, plus `value` and `track`, must be `<= 0x7F`. A byte
 * with the status bit set would be read as a new MIDI status byte and corrupt
 * the stream, so such input is rejected.
 *
 * # Safety
 * `payload` must point to `payload_len` readable bytes, or be null when
 * `payload_len` is 0. `out`/`out_len` must be writable.
 */
TactusStatus tactus_daw_encode(uint8_t command,
                               uint8_t value,
                               uint8_t track,
                               const uint8_t *payload,
                               uintptr_t payload_len,
                               uint8_t *out,
                               uintptr_t out_cap,
                               uintptr_t *out_len);

/**
 * Encode a DAW-port SysEx command whose payload is text.
 *
 * `text` is `text_len` bytes of **7-bit ASCII**, not NUL-terminated.
 *
 * Non-ASCII is rejected with [`TactusStatus::NonAsciiText`]. The device's text
 * payloads are one byte per character with no length prefix, so a byte
 * `>= 0x80` would be read as a status byte and corrupt the frame. Callers
 * wanting best-effort display should transliterate first — this function will
 * not quietly mangle a name.
 *
 * # Safety
 * `text` must point to `text_len` readable bytes. `out`/`out_len` must be
 * writable.
 */
TactusStatus tactus_daw_encode_text(uint8_t command,
                                    uint8_t value,
                                    uint8_t track,
                                    const char *text,
                                    uintptr_t text_len,
                                    uint8_t *out,
                                    uintptr_t out_cap,
                                    uintptr_t *out_len);

/**
 * Copy the three handshake messages into `out` as 3 x 3 bytes.
 *
 * Order: hello, enable-14-bit-parameters, goodbye.
 *
 * # Safety
 * `out` must point to at least 9 writable bytes.
 */
TactusStatus tactus_daw_handshake(uint8_t *out, uintptr_t out_cap);

/**
 * The single wire byte identifying a page.
 */
uint8_t tactus_page_wire_byte(TactusPage page);

/**
 * Upper bound on the encoded size of a pixel frame, for buffer sizing.
 */
uintptr_t tactus_display_max_encoded_len(TactusPixelFormat format, uint32_t w, uint32_t h);

/**
 * Encode a pixel display frame.
 *
 * `pixels` is `0xAARRGGBB` words, `stride` words per row. The rectangle
 * `(x, y, w, h)` is origin plus extent and must lie inside the surface. When
 * `compress` is non-zero the payload is run-length encoded; `previous` may be
 * a surface of identical geometry to enable skip runs, or null for a full
 * redraw.
 *
 * 16 bpp formats require even `x` and `w`, because two pixels share a word.
 * Odd values are rejected rather than adjusted.
 *
 * # Safety
 * `pixels` must point to at least `stride * surface_h` readable `uint32_t`.
 * `previous`, when non-null, must satisfy the same. `out`/`out_len` must be
 * writable for `out_cap` bytes.
 */
TactusStatus tactus_display_encode_frame(uint8_t display_index,
                                         TactusPixelFormat format,
                                         uint8_t compress,
                                         const uint32_t *pixels,
                                         uint32_t surface_w,
                                         uint32_t surface_h,
                                         uint32_t stride,
                                         uint32_t x,
                                         uint32_t y,
                                         uint32_t w,
                                         uint32_t h,
                                         const uint32_t *previous,
                                         uint8_t *out,
                                         uintptr_t out_cap,
                                         uintptr_t *out_len);

#endif  /* TACTUS_H */
