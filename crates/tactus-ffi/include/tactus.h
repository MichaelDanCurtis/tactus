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
#define TACTUS_ABI_VERSION_MAJOR 0

/**
 * Minor ABI version. Changes when functionality is added compatibly.
 */
#define TACTUS_ABI_VERSION_MINOR 2

/**
 * Buffer size for a feature-report exchange: report id plus payload.
 */
#define TACTUS_FEATURE_BUFFER_LEN 33

/**
 * Payload bytes in a feature report, excluding the report id.
 */
#define TACTUS_FEATURE_PAYLOAD_LEN 32

/**
 * Largest HID input report, including its report id.
 */
#define TACTUS_MAX_INPUT_REPORT_LEN 63

/**
 * Highest accepted light-guide value.
 */
#define TACTUS_LIGHT_GUIDE_MAX 127

/**
 * Keys on the largest model, and so the length
 * [`device::tactus_device_set_light_guide`] needs for an S88.
 */
#define TACTUS_MAX_KEYS 88

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
   * No matching device is attached.
   */
  TactusStatus_DeviceNotFound = -7,
  /**
   * More than one device matched, so the choice would be a guess.
   */
  TactusStatus_AmbiguousDevice = -8,
  /**
   * The device was found but could not be opened — commonly a permission
   * problem, or another process holding it exclusively.
   */
  TactusStatus_OpenFailed = -9,
  /**
   * The SDK refused the operation by policy, not because the platform
   * failed. Retrying will not help; see [`tactus_last_error_message`].
   */
  TactusStatus_Refused = -10,
  /**
   * The platform's HID or MIDI layer reported a failure.
   */
  TactusStatus_IoFailure = -11,
  /**
   * No MIDI port matched the requested role.
   */
  TactusStatus_PortNotFound = -12,
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
 * Which of the keyboard's three MIDI ports to use.
 */
enum TactusPortRole
#if __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // __STDC_VERSION__ >= 202311L
 {
  /**
   * Keys, aftertouch, pedals, wheels — ordinary performance data.
   */
  TactusPortRole_Main = 0,
  /**
   * Host integration: the SysEx command set and channel-16 control changes.
   */
  TactusPortRole_Daw = 1,
  /**
   * External MIDI routed through the instrument.
   */
  TactusPortRole_Ext = 2,
};
#if __STDC_VERSION__ >= 202311L
typedef enum TactusPortRole TactusPortRole;
#else
typedef uint32_t TactusPortRole;
#endif // __STDC_VERSION__ >= 202311L

/**
 * An open keyboard.
 *
 * Opaque. Released — and the HID handle closed — by
 * [`tactus_device_free`].
 */
typedef struct TactusDevice TactusDevice;

/**
 * A snapshot of the attached keyboards.
 *
 * Opaque. Created by [`tactus_device_list_new`], released by
 * [`tactus_device_list_free`]. The strings handed out by
 * [`tactus_device_list_get`] are owned by this list and stay valid until it
 * is freed.
 */
typedef struct TactusDeviceList TactusDeviceList;

/**
 * An open MIDI input port.
 *
 * Opaque. Incoming messages queue as they arrive; drain them with
 * [`tactus_midi_in_recv`]. Released by [`tactus_midi_in_free`].
 */
typedef struct TactusMidiIn TactusMidiIn;

/**
 * An open MIDI output port.
 *
 * Opaque. Released — and the port closed — by [`tactus_midi_out_free`].
 */
typedef struct TactusMidiOut TactusMidiOut;

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
 * One enumerated keyboard.
 *
 * # String lifetimes
 *
 * `usb_serial`, `product_string` and `path` point into the
 * [`TactusDeviceList`] this came from. They are valid until that list is
 * freed, and must not be freed individually. Copy anything you need to keep.
 * A field the platform did not report is null — check before dereferencing.
 *
 * `usb_serial` is **not** the same string as
 * [`tactus_device_serial`]: the USB descriptor carries a short id (observed
 * `99FE7169` on an S88 MK3) while feature report `0xD9` carries a longer one.
 * Both are device-reported, and conflating them produces a device id that
 * silently depends on which call you made.
 */
typedef struct TactusDeviceInfo {
  /**
   * Model, from the USB product id.
   */
  TactusModel model;
  /**
   * USB product id.
   */
  uint16_t product_id;
  /**
   * USB interface number, or `-1` where the platform does not report one.
   */
  int32_t interface_number;
  /**
   * Serial from the USB string descriptor, or null.
   */
  const char *usb_serial;
  /**
   * USB product string, e.g. `KONTROL S88 MK3`, or null.
   */
  const char *product_string;
  /**
   * Opaque platform HID path. Only valid until the device is replugged.
   */
  const char *path;
} TactusDeviceInfo;

/**
 * Copy the diagnostic for this thread's most recent failure into `out`.
 *
 * The message is written NUL-terminated. `out_len`, when non-null, receives
 * the length **excluding** the terminator.
 *
 * Pass a null `out` with a non-null `out_len` to ask how much space is
 * needed; the required capacity *including* the terminator is
 * `*out_len + 1`. A buffer that is too small yields
 * [`TactusStatus::BufferTooSmall`] and sets `out_len` the same way, so one
 * retry with the reported size always succeeds.
 *
 * Meaningful only immediately after a function returned a non-`Ok` status on
 * the same thread. Each device or MIDI entry point clears this first, so a
 * message is never attributable to an earlier call. Functions can fail
 * without recording anything, in which case the message is empty — branch on
 * the status code, and treat this as detail for a human.
 *
 * # Safety
 *
 * `out` must be writable for `out_cap` bytes, or null. `out_len` must be
 * writable, or null.
 */
TactusStatus tactus_last_error_message(char *out, uintptr_t out_cap, uintptr_t *out_len);

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

/**
 * Enumerate attached Kontrol S-series MK3 keyboards.
 *
 * Succeeds with a count of zero when nothing is attached: an empty bus is a
 * normal state, not an error. `*out` receives a list that must be released
 * with [`tactus_device_list_free`].
 *
 * # Safety
 *
 * `out` must be writable.
 */
TactusStatus tactus_device_list_new(struct TactusDeviceList **out);

/**
 * How many keyboards the list holds. Returns `0` for a null list.
 */
uintptr_t tactus_device_list_count(const struct TactusDeviceList *list);

/**
 * Read one entry.
 *
 * # Safety
 *
 * `list` must be a live list and `out` writable. The strings written into
 * `*out` borrow from `list`; see [`TactusDeviceInfo`].
 */
TactusStatus tactus_device_list_get(const struct TactusDeviceList *list,
                                    uintptr_t index,
                                    struct TactusDeviceInfo *out);

/**
 * Release a device list. Passing null is safe and does nothing.
 *
 * # Safety
 *
 * `list` must come from [`tactus_device_list_new`] and must not have been
 * freed already. Any [`TactusDeviceInfo`] strings taken from it are dangling
 * afterwards.
 */
void tactus_device_list_free(struct TactusDeviceList *list);

/**
 * Open the keyboard at `index` in an enumerated list.
 *
 * Preferred over opening by path: the caller never has to handle a platform
 * path string, and the device identity they inspected is the one they get.
 *
 * # Safety
 *
 * `list` must be live and `out` writable.
 */
TactusStatus tactus_device_open_at(const struct TactusDeviceList *list,
                                   uintptr_t index,
                                   struct TactusDevice **out);

/**
 * Open the only attached keyboard.
 *
 * Fails with [`TactusStatus::AmbiguousDevice`] when more than one is present,
 * rather than picking one — with two keyboards attached, a silent choice
 * sends a user's output to the wrong instrument.
 *
 * # Safety
 *
 * `out` must be writable.
 */
TactusStatus tactus_device_open_only(struct TactusDevice **out);

/**
 * Open the single attached keyboard of a given model.
 *
 * # Safety
 *
 * `out` must be writable.
 */
TactusStatus tactus_device_open_model(TactusModel model, struct TactusDevice **out);

/**
 * Close a device and release its handle. Passing null is safe.
 *
 * # Safety
 *
 * `device` must come from one of the `tactus_device_open_*` functions and
 * must not have been freed already.
 */
void tactus_device_free(struct TactusDevice *device);

/**
 * The model of an open device.
 *
 * # Safety
 *
 * `device` must be live and `out` writable.
 */
TactusStatus tactus_device_model(const struct TactusDevice *device, TactusModel *out);

/**
 * Whether this handle was opened exclusively, taking the device away from
 * other processes.
 *
 * Should always report `false`. macOS HID opens are exclusive by default —
 * `hidapi`'s `hid_init()` calls `hid_darwin_set_open_exclusive(1)` — which
 * would silently break a running vendor session, so every context this SDK
 * creates turns that off. This function exists so a host can *check* that on
 * real hardware instead of taking a comment's word for it.
 *
 * # Safety
 *
 * `device` must be live and `out` writable.
 */
TactusStatus tactus_device_is_open_exclusive(const struct TactusDevice *device, bool *out);

/**
 * Read the serial number from feature report `0xD9`.
 *
 * Written NUL-terminated into `out`; `out_len`, when non-null, receives the
 * length excluding the terminator. Pass a null `out` with a non-null
 * `out_len` to ask for the required length first.
 *
 * This is **not** the USB descriptor serial in [`TactusDeviceInfo`]; the two
 * are different device-reported fields.
 *
 * The returned text is **data**: display it, compare it, log it. It is not a
 * path or a command, and nothing in this SDK treats it as one.
 *
 * # Safety
 *
 * `device` must be live. `out` must be writable for `out_cap` bytes or null;
 * `out_len` must be writable or null.
 */
TactusStatus tactus_device_serial(const struct TactusDevice *device,
                                  char *out,
                                  uintptr_t out_cap,
                                  uintptr_t *out_len);

/**
 * Read a feature report.
 *
 * Only the three report ids this firmware declares — `0xD0`, `0xD8`, `0xD9` —
 * are accepted; anything else is [`TactusStatus::Refused`] without reaching
 * the device. That includes `0xF8`, which the vendor's own published
 * report-map table claims and the live descriptor does not declare at all.
 *
 * `out` must hold at least `TACTUS_FEATURE_BUFFER_LEN` bytes. On success
 * `*out_len` receives the byte count, and `out[0]` is the report id the
 * device echoed back.
 *
 * # Safety
 *
 * `device` must be live, `out` writable for `out_cap` bytes, `out_len`
 * writable.
 */
TactusStatus tactus_device_read_feature(const struct TactusDevice *device,
                                        uint8_t report_id,
                                        uint8_t *out,
                                        uintptr_t out_cap,
                                        uintptr_t *out_len);

/**
 * Always fails with [`TactusStatus::Refused`]. Writing feature reports is not
 * supported.
 *
 * This function exists so the refusal is discoverable at the ABI rather than
 * looking like an oversight. `0xD8` and `0xD9` are read-only. `0xD0` is
 * settable and reads back as all zeros on a device with the vendor's software
 * running, so not one of its 32 bytes has a known meaning — writing a guess
 * to a configuration register on somebody's instrument is not a thing this
 * SDK will do.
 *
 * # Safety
 *
 * Arguments are not dereferenced; the call always fails.
 */
TactusStatus tactus_device_write_feature(struct TactusDevice *_device,
                                         uint8_t report_id,
                                         const uint8_t *_data,
                                         uintptr_t _data_len);

/**
 * Wait up to `timeout_ms` for an input report.
 *
 * On success `*got` is `true` and `*out` holds the decoded report; `*got` is
 * `false` when the timeout expired with nothing to read. **A quiet keyboard
 * is not an error** — an idle instrument is the normal case, and reporting it
 * as a failure would make every host's main loop log noise.
 *
 * A report this SDK does not model yet arrives with
 * `kind == TactusInputKind_Unmodelled` and a meaningful `report_id`, rather
 * than being dropped or guessed at.
 *
 * # Safety
 *
 * `device` must be live; `out` and `got` must be writable.
 */
TactusStatus tactus_device_poll_input(const struct TactusDevice *device,
                                      uint32_t timeout_ms,
                                      struct TactusInputReport *out,
                                      bool *got);

/**
 * Set the per-key light guide.
 *
 * `len` must equal the device's key count — 49, 61 or 88 — so a buffer sized
 * for the wrong model is refused rather than silently truncated. Each value
 * is `0..=TACTUS_LIGHT_GUIDE_MAX` (127); an out-of-range value is refused
 * rather than masked, because masking turns a caller's bug into a wrong
 * colour on the hardware and hides it.
 *
 * # Safety
 *
 * `device` must be live and `values` readable for `len` bytes.
 */
TactusStatus tactus_device_set_light_guide(const struct TactusDevice *device,
                                           const uint8_t *values,
                                           uintptr_t len);

/**
 * Turn every light-guide LED off.
 *
 * # Safety
 *
 * `device` must be live.
 */
TactusStatus tactus_device_clear_light_guide(const struct TactusDevice *device);

/**
 * Open a MIDI output port by role.
 *
 * `device_filter` narrows the match by device name — pass `"S88"` to pick one
 * model, or null for no filter. Two matching keyboards yield
 * [`TactusStatus::AmbiguousDevice`] rather than a guess.
 *
 * # Safety
 *
 * `device_filter` must be NUL-terminated or null; `out` must be writable.
 */
TactusStatus tactus_midi_out_open(TactusPortRole role,
                                  const char *device_filter,
                                  struct TactusMidiOut **out);

/**
 * Send raw MIDI bytes.
 *
 * Nothing here validates the message — a caller who has already encoded one
 * with [`crate::tactus_daw_encode`] should not have it re-checked. Use
 * [`tactus_midi_out_send_command`] to have the framing and data-byte
 * validation done for you.
 *
 * # Safety
 *
 * `out` must be live and `bytes` readable for `len` bytes.
 */
TactusStatus tactus_midi_out_send(struct TactusMidiOut *out, const uint8_t *bytes, uintptr_t len);

/**
 * Encode and send a DAW-port SysEx command.
 *
 * `command` is one of the ids in [`crate::tactus_daw_encode`]'s table.
 * Payload bytes must all be `<= 0x7F`; a byte with its status bit set is
 * refused rather than sent, because it would be read as a new status byte and
 * corrupt the rest of the stream.
 *
 * # Safety
 *
 * `out` must be live; `payload` readable for `payload_len` bytes, or null
 * when `payload_len` is zero.
 */
TactusStatus tactus_midi_out_send_command(struct TactusMidiOut *out,
                                          uint8_t command,
                                          uint8_t value,
                                          uint8_t track,
                                          const uint8_t *payload,
                                          uintptr_t payload_len);

/**
 * Encode and send a DAW-port SysEx command carrying text.
 *
 * `text` must be NUL-terminated and 7-bit ASCII. Non-ASCII is refused with
 * [`TactusStatus::NonAsciiText`] rather than mangled: the device's text
 * payloads are one byte per character with no length prefix, so anything
 * above `U+007F` cannot be represented without corrupting the frame.
 * Transliterate before calling.
 *
 * # Safety
 *
 * `out` must be live and `text` NUL-terminated.
 */
TactusStatus tactus_midi_out_send_text(struct TactusMidiOut *out,
                                       uint8_t command,
                                       uint8_t value,
                                       uint8_t track,
                                       const char *text);

/**
 * Close a MIDI output port. Passing null is safe.
 *
 * # Safety
 *
 * `out` must come from [`tactus_midi_out_open`] and must not have been freed
 * already.
 */
void tactus_midi_out_free(struct TactusMidiOut *out);

/**
 * Open a MIDI input port by role.
 *
 * # Safety
 *
 * `device_filter` must be NUL-terminated or null; `out` must be writable.
 */
TactusStatus tactus_midi_in_open(TactusPortRole role,
                                 const char *device_filter,
                                 struct TactusMidiIn **out);

/**
 * Take the next queued MIDI message, waiting up to `timeout_ms`.
 *
 * `*got` is `false` when the timeout expired with nothing queued — a silent
 * port is normal, not an error. On success `*out_len` holds the message
 * length.
 *
 * A message longer than `out_cap` yields [`TactusStatus::BufferTooSmall`]
 * with `*out_len` set to the length needed. **The message is consumed** in
 * that case: it has already left the queue, so size the buffer for the
 * longest SysEx you expect rather than relying on a retry.
 *
 * # Safety
 *
 * `in_port` must be live; `out` writable for `out_cap` bytes; `out_len` and
 * `got` writable.
 */
TactusStatus tactus_midi_in_recv(const struct TactusMidiIn *in_port,
                                 uint32_t timeout_ms,
                                 uint8_t *out,
                                 uintptr_t out_cap,
                                 uintptr_t *out_len,
                                 bool *got);

/**
 * Close a MIDI input port. Passing null is safe.
 *
 * # Safety
 *
 * `in_port` must come from [`tactus_midi_in_open`] and must not have been
 * freed already.
 */
void tactus_midi_in_free(struct TactusMidiIn *in_port);

#endif  /* TACTUS_H */
