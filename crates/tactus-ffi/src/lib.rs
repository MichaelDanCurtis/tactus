//! C ABI for the Tactus control-surface SDK.
//!
//! This is the boundary that makes "anyone can write a host" true: a stable C
//! interface, so C, C++, Python, Swift, JavaScript, Max and anything else with
//! an FFI can drive the hardware without speaking Rust.
//!
//! # Contract
//!
//! Every exported function obeys the same rules, so a caller only has to learn
//! them once:
//!
//! - Returns [`TactusStatus`] as `int32_t`. [`TactusStatus::Ok`] is `0`;
//!   every error is negative. Functions that produce a byte count return it
//!   through an out-parameter, never muxed into the return value — so a caller
//!   can never mistake a length for an error code.
//! - **Every pointer is checked for null** before use.
//! - **Every buffer takes an explicit length.** Nothing is assumed
//!   NUL-terminated except where a parameter is documented as a C string.
//! - **No panic can cross the boundary.** Unwinding into C is undefined
//!   behaviour, so every entry point is wrapped in [`std::panic::catch_unwind`]
//!   and a panic becomes [`TactusStatus::Internal`]. The underlying crates are
//!   written to be panic-free; this is defence in depth, not a licence for them
//!   to panic.
//! - Out-parameters are only written on success, so a failed call never leaves
//!   a caller with half-updated state.
//!
//! # Versioning
//!
//! [`tactus_abi_version`] returns a packed integer a caller can check before
//! trusting struct layouts. Bump [`TACTUS_ABI_VERSION_MINOR`] for additions, and
//! [`TACTUS_ABI_VERSION_MAJOR`] for anything that changes an existing layout or
//! signature.

#![deny(missing_docs)]
#![allow(clippy::missing_safety_doc)] // every unsafe fn documents its contract in prose below

use std::cell::RefCell;
use std::ffi::CString;
use std::panic::catch_unwind;
use std::slice;

pub mod device;

use tactus_core::{Rect, Surface};
use tactus_display::{Compression, Frame, PixelFormat, encode_frame, max_encoded_len};
use tactus_hid::{InputReport, Model, encode_light_guide};
use tactus_midi::daw;

/// Major ABI version. Changes when an existing layout or signature changes.
pub const TACTUS_ABI_VERSION_MAJOR: u32 = 0;
/// Minor ABI version. Changes when functionality is added compatibly.
pub const TACTUS_ABI_VERSION_MINOR: u32 = 2;

// These are spelled as literals, not as `tactus_hid::FEATURE_BUFFER_LEN`,
// because cbindgen runs with `parse_deps = false` and silently *omits* any
// constant whose value it cannot evaluate — leaving a header that quietly
// lacks a size this crate's own docs tell C callers to use. The compile-time
// assertions below make the duplication safe: if a protocol constant ever
// moves, this crate stops building rather than shipping a stale header.
//
// They carry the `TACTUS_` prefix because constants reach C as plain
// `#define`s, with no namespace to live in.

/// Buffer size for a feature-report exchange: report id plus payload.
pub const TACTUS_FEATURE_BUFFER_LEN: usize = 33;

/// Payload bytes in a feature report, excluding the report id.
pub const TACTUS_FEATURE_PAYLOAD_LEN: usize = 32;

/// Largest HID input report, including its report id.
pub const TACTUS_MAX_INPUT_REPORT_LEN: usize = 63;

/// Highest accepted light-guide value.
pub const TACTUS_LIGHT_GUIDE_MAX: u8 = 127;

/// Keys on the largest model, and so the length
/// [`device::tactus_device_set_light_guide`] needs for an S88.
pub const TACTUS_MAX_KEYS: usize = 88;

const _: () = assert!(TACTUS_FEATURE_BUFFER_LEN == tactus_hid::FEATURE_BUFFER_LEN);
const _: () = assert!(TACTUS_FEATURE_PAYLOAD_LEN == tactus_hid::FEATURE_PAYLOAD_LEN);
const _: () = assert!(TACTUS_MAX_INPUT_REPORT_LEN == tactus_hid::MAX_INPUT_REPORT_LEN);
const _: () = assert!(TACTUS_LIGHT_GUIDE_MAX == tactus_hid::LIGHT_GUIDE_MAX);
const _: () = assert!(TACTUS_MAX_KEYS == Model::S88.keys());

/// Status returned by every exported function.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TactusStatus {
    /// Success.
    Ok = 0,
    /// A required pointer argument was null.
    NullArgument = -1,
    /// A buffer was too small for the result.
    BufferTooSmall = -2,
    /// An argument was outside its permitted range.
    OutOfRange = -3,
    /// An argument had an invalid or unrecognised value.
    InvalidArgument = -4,
    /// Text contained a character the device cannot represent.
    NonAsciiText = -5,
    /// Geometry was invalid for the requested operation.
    InvalidGeometry = -6,

    /// No matching device is attached.
    DeviceNotFound = -7,
    /// More than one device matched, so the choice would be a guess.
    AmbiguousDevice = -8,
    /// The device was found but could not be opened — commonly a permission
    /// problem, or another process holding it exclusively.
    OpenFailed = -9,
    /// The SDK refused the operation by policy, not because the platform
    /// failed. Retrying will not help; see [`tactus_last_error_message`].
    Refused = -10,
    /// The platform's HID or MIDI layer reported a failure.
    IoFailure = -11,
    /// No MIDI port matched the requested role.
    PortNotFound = -12,

    /// An internal error, including a caught panic. Should never be seen.
    Internal = -100,
}

/// Run an entry point, converting any panic into [`TactusStatus::Internal`].
fn guard<F: FnOnce() -> TactusStatus>(f: F) -> TactusStatus {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(s) => s,
        Err(_) => TactusStatus::Internal,
    }
}

// ---------------------------------------------------------------------------
// Last error message
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Record a diagnostic for the most recent failure on this thread.
///
/// The I/O layer's errors name the value that caused them — which model was
/// not found, which report id was refused, how many candidates were ambiguous.
/// A bare status code would throw all of that away, so it is kept here for a
/// caller to retrieve with [`tactus_last_error_message`].
pub(crate) fn set_last_error(message: impl std::fmt::Display) {
    let text = message.to_string();
    // A device-supplied string (a serial, a port name) could in principle
    // contain a NUL, which `CString` rejects. Substitute rather than lose the
    // message: the diagnostic matters more than the exact bytes, and device
    // strings are data we display, never something we execute.
    let sanitised: Vec<u8> = text
        .into_bytes()
        .into_iter()
        .map(|b| if b == 0 { b'?' } else { b })
        .collect();
    let cstring = CString::new(sanitised).unwrap_or_default();
    LAST_ERROR.with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut() {
            *slot = Some(cstring);
        }
    });
}

/// Drop any recorded diagnostic, so a stale message cannot be mistaken for
/// the result of the call in progress.
pub(crate) fn clear_last_error() {
    LAST_ERROR.with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut() {
            *slot = None;
        }
    });
}

/// Copy the diagnostic for this thread's most recent failure into `out`.
///
/// The message is written NUL-terminated. `out_len`, when non-null, receives
/// the length **excluding** the terminator.
///
/// Pass a null `out` with a non-null `out_len` to ask how much space is
/// needed; the required capacity *including* the terminator is
/// `*out_len + 1`. A buffer that is too small yields
/// [`TactusStatus::BufferTooSmall`] and sets `out_len` the same way, so one
/// retry with the reported size always succeeds.
///
/// Meaningful only immediately after a function returned a non-`Ok` status on
/// the same thread. Each device or MIDI entry point clears this first, so a
/// message is never attributable to an earlier call. Functions can fail
/// without recording anything, in which case the message is empty — branch on
/// the status code, and treat this as detail for a human.
///
/// # Safety
///
/// `out` must be writable for `out_cap` bytes, or null. `out_len` must be
/// writable, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_last_error_message(
    out: *mut core::ffi::c_char,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    guard(|| {
        LAST_ERROR.with(|cell| {
            let slot = match cell.try_borrow() {
                Ok(s) => s,
                Err(_) => return TactusStatus::Internal,
            };
            let bytes = slot.as_ref().map(|c| c.as_bytes()).unwrap_or(&[]);
            let len = bytes.len();

            if !out_len.is_null() {
                unsafe { *out_len = len };
            }
            if out.is_null() {
                // A sizing query is only valid if there is somewhere to put
                // the size.
                return if out_len.is_null() {
                    TactusStatus::NullArgument
                } else {
                    TactusStatus::Ok
                };
            }
            if out_cap < len + 1 {
                return TactusStatus::BufferTooSmall;
            }
            let dst = unsafe { slice::from_raw_parts_mut(out.cast::<u8>(), len + 1) };
            let Some((body, rest)) = dst.split_at_mut_checked(len) else {
                return TactusStatus::Internal;
            };
            body.copy_from_slice(bytes);
            match rest.first_mut() {
                Some(terminator) => {
                    *terminator = 0;
                    TactusStatus::Ok
                }
                None => TactusStatus::Internal,
            }
        })
    })
}

/// The ABI version, packed as `(major << 16) | minor`.
///
/// Call this before trusting any struct layout in this header.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_abi_version() -> u32 {
    (TACTUS_ABI_VERSION_MAJOR << 16) | TACTUS_ABI_VERSION_MINOR
}

/// A human-readable status name, as a static NUL-terminated C string.
///
/// Never returns null; an unrecognised code yields `"unknown"`.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_status_name(status: i32) -> *const core::ffi::c_char {
    let s: &'static [u8] = match status {
        0 => b"ok\0",
        -1 => b"null argument\0",
        -2 => b"buffer too small\0",
        -3 => b"out of range\0",
        -4 => b"invalid argument\0",
        -5 => b"non-ascii text\0",
        -6 => b"invalid geometry\0",
        -7 => b"device not found\0",
        -8 => b"ambiguous device\0",
        -9 => b"open failed\0",
        -10 => b"refused\0",
        -11 => b"io failure\0",
        -12 => b"port not found\0",
        -100 => b"internal error\0",
        _ => b"unknown\0",
    };
    s.as_ptr().cast()
}

// ---------------------------------------------------------------------------
// Device identity
// ---------------------------------------------------------------------------

/// A keyboard model.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TactusModel {
    /// 49-key.
    S49 = 49,
    /// 61-key.
    S61 = 61,
    /// 88-key.
    S88 = 88,
}

impl TactusModel {
    fn to_model(self) -> Model {
        match self {
            Self::S49 => Model::S49,
            Self::S61 => Model::S61,
            Self::S88 => Model::S88,
        }
    }

    fn from_model(m: Model) -> Self {
        match m {
            Model::S49 => Self::S49,
            Model::S61 => Self::S61,
            Model::S88 => Self::S88,
        }
    }
}

/// The USB vendor id these devices use.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_vendor_id() -> u16 {
    tactus_hid::VENDOR_ID
}

/// Identify a model from its USB product id.
///
/// # Safety
/// `out_model` must be a valid writable pointer to a `TactusModel`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_model_from_product_id(
    product_id: u16,
    out_model: *mut TactusModel,
) -> TactusStatus {
    guard(|| {
        if out_model.is_null() {
            return TactusStatus::NullArgument;
        }
        match Model::from_product_id(product_id) {
            Some(m) => {
                unsafe { *out_model = TactusModel::from_model(m) };
                TactusStatus::Ok
            }
            None => TactusStatus::InvalidArgument,
        }
    })
}

/// Number of keys on a model. Also the exact light-guide payload length.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_model_keys(model: TactusModel) -> u32 {
    model.to_model().keys() as u32
}

// ---------------------------------------------------------------------------
// Light guide
// ---------------------------------------------------------------------------

/// Highest legal light-guide value.
///
/// The device declares report `0x82` with a logical range of `0..=127`, not
/// `0..=255` — consistent with a palette index rather than an intensity.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_light_guide_max() -> u8 {
    tactus_hid::LIGHT_GUIDE_MAX
}

/// Encode a light-guide HID output report.
///
/// `values` must hold exactly [`tactus_model_keys`] entries, each
/// `<= `[`tactus_light_guide_max`]. Writes the report id followed by the
/// payload into `out`, and stores the byte count in `out_len`.
///
/// Out-of-range values are **rejected**, not masked: masking would send a
/// wrong colour while hiding the caller's bug.
///
/// # Safety
/// `values` must point to `values_len` readable bytes. `out` must point to
/// `out_cap` writable bytes. `out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_encode_light_guide(
    model: TactusModel,
    values: *const u8,
    values_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    guard(|| {
        if values.is_null() || out.is_null() || out_len.is_null() {
            return TactusStatus::NullArgument;
        }
        let vals = unsafe { slice::from_raw_parts(values, values_len) };
        let buf = unsafe { slice::from_raw_parts_mut(out, out_cap) };
        match encode_light_guide(buf, model.to_model(), vals) {
            Ok(n) => {
                unsafe { *out_len = n };
                TactusStatus::Ok
            }
            Err(tactus_hid::HidError::KeyCountMismatch { .. }) => TactusStatus::InvalidArgument,
            Err(tactus_hid::HidError::LightGuideOutOfRange { .. }) => TactusStatus::OutOfRange,
            Err(tactus_hid::HidError::WrongLength { .. }) => TactusStatus::BufferTooSmall,
            Err(_) => TactusStatus::InvalidArgument,
        }
    })
}

// ---------------------------------------------------------------------------
// HID input
// ---------------------------------------------------------------------------

/// Which variant of [`TactusInputReport`] is populated.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TactusInputKind {
    /// Report `0x01`: `bits[0..40]`, `nibbles`.
    Controls = 0,
    /// Report `0x02`: `bits[0..8]` leading, `continuous`, `wide`,
    /// `bits[8..24]` trailing.
    Analog = 1,
    /// Report `0x03`: `bits[0..8]`, `full_range`, `eleven_bit`, `tail`.
    Wheels = 2,
    /// A report this SDK does not yet model. Only `report_id` is meaningful.
    Unmodelled = 3,
}

/// A decoded HID input report.
///
/// A flat tagged struct rather than a union, so every FFI consumer can read it
/// without layout games. `kind` says which fields are meaningful; the rest are
/// zeroed.
///
/// **Field indices are not control names.** The device's descriptor declares
/// how many controls of what width, but not which physical control each index
/// is — it uses one opaque vendor usage page. Nothing here claims "index 3 is
/// Play", because guessing would be worse than silence.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TactusInputReport {
    /// Which fields below are populated.
    pub kind: TactusInputKind,
    /// The HID report id this came from.
    pub report_id: u8,
    /// Single-bit controls, one byte per control, `0` or `1`.
    pub bits: [u8; 40],
    /// How many entries of `bits` are meaningful.
    pub bit_count: u32,
    /// 4-bit controls (report `0x01`), each `0..=15`.
    pub nibbles: [u8; 2],
    /// Continuous controls (report `0x02`), each `0..=999`.
    pub continuous: [u16; 8],
    /// Wider controls (report `0x02`), each `0..=4095`.
    pub wide: [u16; 4],
    /// Full-range controls (report `0x03`), each `0..=65535`.
    pub full_range: [u16; 3],
    /// 11-bit controls (report `0x03`), each `0..=2047`.
    pub eleven_bit: [u16; 2],
    /// Trailing byte (report `0x03`).
    pub tail: u8,
}

impl TactusInputReport {
    fn zeroed() -> Self {
        Self {
            kind: TactusInputKind::Unmodelled,
            report_id: 0,
            bits: [0; 40],
            bit_count: 0,
            nibbles: [0; 2],
            continuous: [0; 8],
            wide: [0; 4],
            full_range: [0; 3],
            eleven_bit: [0; 2],
            tail: 0,
        }
    }

    /// Flatten a decoded report into the C struct.
    ///
    /// Shared by [`tactus_decode_input`] and
    /// [`device::tactus_device_poll_input`] so there is one layout mapping
    /// rather than two that can drift apart.
    pub(crate) fn from_decoded(report_id: u8, decoded: InputReport) -> Self {
        let mut r = Self::zeroed();
        r.report_id = report_id;
        match decoded {
            InputReport::Controls(c) => {
                r.kind = TactusInputKind::Controls;
                for (i, &b) in c.bits.iter().enumerate() {
                    r.bits[i] = b as u8;
                }
                r.bit_count = 40;
                r.nibbles = c.nibbles;
            }
            InputReport::Analog(a) => {
                r.kind = TactusInputKind::Analog;
                for (i, &b) in a.leading_bits.iter().enumerate() {
                    r.bits[i] = b as u8;
                }
                for (i, &b) in a.trailing_bits.iter().enumerate() {
                    r.bits[8 + i] = b as u8;
                }
                r.bit_count = 24;
                r.continuous = a.continuous;
                r.wide = a.wide;
            }
            InputReport::Wheels(w) => {
                r.kind = TactusInputKind::Wheels;
                for (i, &b) in w.bits.iter().enumerate() {
                    r.bits[i] = b as u8;
                }
                r.bit_count = 8;
                r.full_range = w.full_range;
                r.eleven_bit = w.eleven_bit;
                r.tail = w.tail;
            }
            InputReport::Unmodelled { id, .. } => {
                r.kind = TactusInputKind::Unmodelled;
                r.report_id = id;
            }
        }
        r
    }
}

/// Decode a complete HID input report, **including** its leading report id.
///
/// # Safety
/// `report` must point to `report_len` readable bytes. `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_decode_input(
    report: *const u8,
    report_len: usize,
    out: *mut TactusInputReport,
) -> TactusStatus {
    guard(|| {
        if report.is_null() || out.is_null() {
            return TactusStatus::NullArgument;
        }
        let bytes = unsafe { slice::from_raw_parts(report, report_len) };
        let decoded = match tactus_hid::decode_input(bytes) {
            Ok(d) => d,
            Err(_) => return TactusStatus::InvalidArgument,
        };
        let id = bytes.first().copied().unwrap_or(0);
        let r = TactusInputReport::from_decoded(id, decoded);
        unsafe { *out = r };
        TactusStatus::Ok
    })
}

// ---------------------------------------------------------------------------
// DAW-port MIDI
// ---------------------------------------------------------------------------

/// Decode a relative encoder value into a signed delta, `-64..=63`.
///
/// Values `0x01..=0x3F` are positive; `0x41..=0x7F` are negative, read as
/// 7-bit two's complement.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_daw_decode_relative(value: u8) -> i8 {
    daw::decode_relative(value)
}

/// Encode a DAW-port SysEx command with a raw payload.
///
/// Every payload byte, plus `value` and `track`, must be `<= 0x7F`. A byte
/// with the status bit set would be read as a new MIDI status byte and corrupt
/// the stream, so such input is rejected.
///
/// # Safety
/// `payload` must point to `payload_len` readable bytes, or be null when
/// `payload_len` is 0. `out`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_daw_encode(
    command: u8,
    value: u8,
    track: u8,
    payload: *const u8,
    payload_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    guard(|| {
        if out.is_null() || out_len.is_null() {
            return TactusStatus::NullArgument;
        }
        if payload.is_null() && payload_len != 0 {
            return TactusStatus::NullArgument;
        }
        let Some(cmd) = daw::Command::from_id(command) else {
            return TactusStatus::InvalidArgument;
        };
        let pl = if payload_len == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(payload, payload_len) }
        };
        let buf = unsafe { slice::from_raw_parts_mut(out, out_cap) };
        match daw::encode(buf, cmd, value, track, pl) {
            Ok(n) => {
                unsafe { *out_len = n };
                TactusStatus::Ok
            }
            Err(tactus_midi::MidiError::NotADataByte { .. }) => TactusStatus::OutOfRange,
            Err(tactus_midi::MidiError::OutputTooSmall { .. }) => TactusStatus::BufferTooSmall,
            Err(_) => TactusStatus::InvalidArgument,
        }
    })
}

/// Encode a DAW-port SysEx command whose payload is text.
///
/// `text` is `text_len` bytes of **7-bit ASCII**, not NUL-terminated.
///
/// Non-ASCII is rejected with [`TactusStatus::NonAsciiText`]. The device's text
/// payloads are one byte per character with no length prefix, so a byte
/// `>= 0x80` would be read as a status byte and corrupt the frame. Callers
/// wanting best-effort display should transliterate first — this function will
/// not quietly mangle a name.
///
/// # Safety
/// `text` must point to `text_len` readable bytes. `out`/`out_len` must be
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_daw_encode_text(
    command: u8,
    value: u8,
    track: u8,
    text: *const core::ffi::c_char,
    text_len: usize,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    guard(|| {
        if text.is_null() || out.is_null() || out_len.is_null() {
            return TactusStatus::NullArgument;
        }
        let Some(cmd) = daw::Command::from_id(command) else {
            return TactusStatus::InvalidArgument;
        };
        let bytes = unsafe { slice::from_raw_parts(text.cast::<u8>(), text_len) };
        let Ok(s) = core::str::from_utf8(bytes) else {
            return TactusStatus::NonAsciiText;
        };
        let buf = unsafe { slice::from_raw_parts_mut(out, out_cap) };
        match daw::encode_text(buf, cmd, value, track, s) {
            Ok(n) => {
                unsafe { *out_len = n };
                TactusStatus::Ok
            }
            Err(tactus_midi::MidiError::NonAsciiText { .. }) => TactusStatus::NonAsciiText,
            Err(tactus_midi::MidiError::OutputTooSmall { .. }) => TactusStatus::BufferTooSmall,
            Err(tactus_midi::MidiError::NotADataByte { .. }) => TactusStatus::OutOfRange,
            Err(_) => TactusStatus::InvalidArgument,
        }
    })
}

/// Copy the three handshake messages into `out` as 3 x 3 bytes.
///
/// Order: hello, enable-14-bit-parameters, goodbye.
///
/// # Safety
/// `out` must point to at least 9 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_daw_handshake(out: *mut u8, out_cap: usize) -> TactusStatus {
    guard(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        if out_cap < 9 {
            return TactusStatus::BufferTooSmall;
        }
        let buf = unsafe { slice::from_raw_parts_mut(out, 9) };
        buf[0..3].copy_from_slice(&daw::handshake::HELLO);
        buf[3..6].copy_from_slice(&daw::handshake::ENABLE_14BIT_PARAMS);
        buf[6..9].copy_from_slice(&daw::handshake::GOODBYE);
        TactusStatus::Ok
    })
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

/// A display wire format, for host-rendered (pixel) devices.
///
/// Only formats whose packing has been individually verified are offered.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TactusPixelFormat {
    /// 16 bpp 5-6-5, red first, big-endian in the word.
    Rgb565 = 0,
    /// 16 bpp 5-6-5, blue first.
    Bgr565 = 1,
    /// 32 bpp, wire order `FF, R, G, B`.
    Xrgb8888 = 2,
}

impl TactusPixelFormat {
    fn to_format(self) -> PixelFormat {
        match self {
            Self::Rgb565 => PixelFormat::Rgb565,
            Self::Bgr565 => PixelFormat::Bgr565,
            Self::Xrgb8888 => PixelFormat::Xrgb8888,
        }
    }
}

/// A page on a device-rendered display.
///
/// Values are the device's own wire bytes. Used by MK3, which renders its own
/// UI — such a device is never sent pixels.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TactusPage {
    /// Sound browser.
    Browser = 0,
    /// Mixer.
    Mixer = 1,
    /// Plugin parameters.
    Parameter = 2,
    /// Smart play.
    SmartPlay = 3,
}

/// The single wire byte identifying a page.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_page_wire_byte(page: TactusPage) -> u8 {
    page as u8
}

/// Upper bound on the encoded size of a pixel frame, for buffer sizing.
#[unsafe(no_mangle)]
pub extern "C" fn tactus_display_max_encoded_len(
    format: TactusPixelFormat,
    w: u32,
    h: u32,
) -> usize {
    max_encoded_len(format.to_format(), &Rect::new(0, 0, w, h))
}

/// Encode a pixel display frame.
///
/// `pixels` is `0xAARRGGBB` words, `stride` words per row. The rectangle
/// `(x, y, w, h)` is origin plus extent and must lie inside the surface. When
/// `compress` is non-zero the payload is run-length encoded; `previous` may be
/// a surface of identical geometry to enable skip runs, or null for a full
/// redraw.
///
/// 16 bpp formats require even `x` and `w`, because two pixels share a word.
/// Odd values are rejected rather than adjusted.
///
/// # Safety
/// `pixels` must point to at least `stride * surface_h` readable `uint32_t`.
/// `previous`, when non-null, must satisfy the same. `out`/`out_len` must be
/// writable for `out_cap` bytes.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn tactus_display_encode_frame(
    display_index: u8,
    format: TactusPixelFormat,
    compress: u8,
    pixels: *const u32,
    surface_w: u32,
    surface_h: u32,
    stride: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    previous: *const u32,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    guard(|| {
        if pixels.is_null() || out.is_null() || out_len.is_null() {
            return TactusStatus::NullArgument;
        }
        let effective_stride = stride.max(surface_w) as usize;
        let count = match effective_stride.checked_mul(surface_h as usize) {
            Some(c) => c,
            None => return TactusStatus::InvalidGeometry,
        };
        let src = unsafe { slice::from_raw_parts(pixels, count) };
        let Ok(surface) = Surface::new(src, surface_w, surface_h, stride) else {
            return TactusStatus::InvalidGeometry;
        };
        let prev = if previous.is_null() {
            None
        } else {
            let p = unsafe { slice::from_raw_parts(previous, count) };
            match Surface::new(p, surface_w, surface_h, stride) {
                Ok(s) => Some(s),
                Err(_) => return TactusStatus::InvalidGeometry,
            }
        };
        let frame = Frame {
            display_index,
            format: format.to_format(),
            compression: if compress != 0 {
                Compression::RunLength
            } else {
                Compression::None
            },
            surface,
            rect: Rect::new(x, y, w, h),
            previous: prev,
        };
        let buf = unsafe { slice::from_raw_parts_mut(out, out_cap) };
        match encode_frame(buf, &frame) {
            Ok(n) => {
                unsafe { *out_len = n };
                TactusStatus::Ok
            }
            Err(tactus_display::DisplayError::OutputTooSmall { .. }) => {
                TactusStatus::BufferTooSmall
            }
            Err(tactus_display::DisplayError::OddGeometry { .. })
            | Err(tactus_display::DisplayError::EmptyRect)
            | Err(tactus_display::DisplayError::RectTooLargeForWire { .. })
            | Err(tactus_display::DisplayError::PreviousGeometryMismatch)
            | Err(tactus_display::DisplayError::Core(_)) => TactusStatus::InvalidGeometry,
            Err(_) => TactusStatus::Internal,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_packs_major_and_minor() {
        let v = tactus_abi_version();
        assert_eq!(v >> 16, TACTUS_ABI_VERSION_MAJOR);
        assert_eq!(v & 0xFFFF, TACTUS_ABI_VERSION_MINOR);
    }

    #[test]
    fn status_names_are_never_null_and_always_terminated() {
        for code in [0, -1, -2, -3, -4, -5, -6, -100, 12345] {
            let p = tactus_status_name(code);
            assert!(!p.is_null());
            let s = unsafe { core::ffi::CStr::from_ptr(p) };
            assert!(!s.to_bytes().is_empty(), "code {code} has a name");
        }
    }

    #[test]
    fn every_pointer_argument_is_null_checked() {
        // The single most important property of this layer: a null from a
        // careless caller must produce an error, not a segfault.
        let mut len = 0usize;
        let mut model = TactusModel::S49;
        let mut rep = TactusInputReport::zeroed();
        let mut buf = [0u8; 256];

        unsafe {
            assert_eq!(
                tactus_model_from_product_id(0x2120, core::ptr::null_mut()),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_encode_light_guide(
                    TactusModel::S88,
                    core::ptr::null(),
                    88,
                    buf.as_mut_ptr(),
                    buf.len(),
                    &mut len
                ),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_decode_input(core::ptr::null(), 7, &mut rep),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_daw_encode(
                    0x48,
                    0,
                    0,
                    core::ptr::null(),
                    4,
                    buf.as_mut_ptr(),
                    256,
                    &mut len
                ),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_daw_handshake(core::ptr::null_mut(), 9),
                TactusStatus::NullArgument
            );
            // A null payload with zero length is legitimate.
            assert_eq!(
                tactus_daw_encode(
                    0x40,
                    1,
                    0,
                    core::ptr::null(),
                    0,
                    buf.as_mut_ptr(),
                    256,
                    &mut len
                ),
                TactusStatus::Ok
            );
            let _ = &mut model;
        }
    }

    #[test]
    fn model_identity_round_trips_and_rejects_strangers() {
        let mut m = TactusModel::S49;
        unsafe {
            assert_eq!(
                tactus_model_from_product_id(0x2120, &mut m),
                TactusStatus::Ok
            );
        }
        assert_eq!(m, TactusModel::S88);
        assert_eq!(tactus_model_keys(m), 88);
        // A Traktor S4 MK3 shares the vendor but is a different device.
        unsafe {
            assert_eq!(
                tactus_model_from_product_id(0x1720, &mut m),
                TactusStatus::InvalidArgument
            );
        }
    }

    #[test]
    fn light_guide_rejects_out_of_range_through_the_abi() {
        let mut values = [0u8; 88];
        values[10] = 200;
        let mut out = [0u8; 128];
        let mut len = 0usize;
        unsafe {
            assert_eq!(
                tactus_encode_light_guide(
                    TactusModel::S88,
                    values.as_ptr(),
                    values.len(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut len
                ),
                TactusStatus::OutOfRange
            );
            // In range succeeds and reports the length.
            values[10] = 127;
            assert_eq!(
                tactus_encode_light_guide(
                    TactusModel::S88,
                    values.as_ptr(),
                    values.len(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut len
                ),
                TactusStatus::Ok
            );
        }
        assert_eq!(len, 89);
        assert_eq!(out[0], 0x82);
    }

    #[test]
    fn out_params_are_untouched_on_failure() {
        // A failed call must not leave the caller with half-updated state.
        let values = [0u8; 10]; // wrong key count for an S88
        let mut out = [0u8; 128];
        let mut len = 0xDEAD_BEEFusize;
        unsafe {
            let rc = tactus_encode_light_guide(
                TactusModel::S88,
                values.as_ptr(),
                values.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut len,
            );
            assert_ne!(rc, TactusStatus::Ok);
        }
        assert_eq!(len, 0xDEAD_BEEF, "out_len must be untouched on failure");
    }

    #[test]
    fn input_decoding_tags_the_variant_and_reports_bit_count() {
        let mut rep = TactusInputReport::zeroed();
        let mut r = vec![0x01u8];
        r.extend_from_slice(&[0b0000_0001, 0, 0, 0, 0, 0x5A]);
        unsafe {
            assert_eq!(
                tactus_decode_input(&r[0], r.len(), &mut rep),
                TactusStatus::Ok
            );
        }
        assert_eq!(rep.kind, TactusInputKind::Controls);
        assert_eq!(rep.report_id, 0x01);
        assert_eq!(rep.bit_count, 40);
        assert_eq!(rep.bits[0], 1);
        assert_eq!(rep.nibbles, [0x0A, 0x05]);
    }

    #[test]
    fn an_unmodelled_report_is_tagged_not_rejected() {
        let mut rep = TactusInputReport::zeroed();
        let mut r = vec![0xAAu8];
        r.extend_from_slice(&[0u8; 62]);
        unsafe {
            assert_eq!(
                tactus_decode_input(&r[0], r.len(), &mut rep),
                TactusStatus::Ok
            );
        }
        assert_eq!(rep.kind, TactusInputKind::Unmodelled);
        assert_eq!(rep.report_id, 0xAA);
    }

    #[test]
    fn daw_text_rejects_non_ascii_with_a_distinct_code() {
        let text = "Café";
        let mut out = [0u8; 64];
        let mut len = 0usize;
        unsafe {
            assert_eq!(
                tactus_daw_encode_text(
                    0x48,
                    0,
                    0,
                    text.as_ptr().cast(),
                    text.len(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut len
                ),
                TactusStatus::NonAsciiText,
                "a caller must be able to distinguish this from a generic error"
            );
        }
    }

    #[test]
    fn daw_encode_rejects_an_unknown_command_id() {
        let mut out = [0u8; 64];
        let mut len = 0usize;
        unsafe {
            // 0x4A is absent from NI's table.
            assert_eq!(
                tactus_daw_encode(
                    0x4A,
                    0,
                    0,
                    core::ptr::null(),
                    0,
                    out.as_mut_ptr(),
                    64,
                    &mut len
                ),
                TactusStatus::InvalidArgument
            );
        }
    }

    #[test]
    fn handshake_matches_the_protocol_crate() {
        let mut out = [0u8; 9];
        unsafe {
            assert_eq!(tactus_daw_handshake(out.as_mut_ptr(), 9), TactusStatus::Ok);
        }
        assert_eq!(&out[0..3], &[0xBF, 0x01, 0x04]);
        assert_eq!(&out[3..6], &[0xBF, 0x06, 0x01]);
        assert_eq!(&out[6..9], &[0xBF, 0x02, 0x00]);
        // A short buffer is refused.
        let mut small = [0u8; 8];
        unsafe {
            assert_eq!(
                tactus_daw_handshake(small.as_mut_ptr(), 8),
                TactusStatus::BufferTooSmall
            );
        }
    }

    #[test]
    fn relative_decoding_crosses_the_abi_unchanged() {
        assert_eq!(tactus_daw_decode_relative(0x01), 1);
        assert_eq!(tactus_daw_decode_relative(0x7F), -1);
        assert_eq!(tactus_daw_decode_relative(0x40), -64);
    }

    #[test]
    fn pages_expose_their_recovered_wire_bytes() {
        assert_eq!(tactus_page_wire_byte(TactusPage::Browser), 0);
        assert_eq!(tactus_page_wire_byte(TactusPage::Mixer), 1);
        assert_eq!(tactus_page_wire_byte(TactusPage::Parameter), 2);
        assert_eq!(tactus_page_wire_byte(TactusPage::SmartPlay), 3);
    }

    #[test]
    fn display_frame_encodes_through_the_abi() {
        let px = [0xFFFF_0000u32; 8 * 2];
        let mut out = vec![0u8; tactus_display_max_encoded_len(TactusPixelFormat::Rgb565, 8, 2)];
        let mut len = 0usize;
        unsafe {
            assert_eq!(
                tactus_display_encode_frame(
                    0,
                    TactusPixelFormat::Rgb565,
                    1,
                    px.as_ptr(),
                    8,
                    2,
                    8,
                    0,
                    0,
                    8,
                    2,
                    core::ptr::null(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut len
                ),
                TactusStatus::Ok
            );
        }
        assert_eq!(&out[..2], &[0x84, 0x00]);
        assert_eq!(out[3], 0x60, "Rgb565 compressed");
    }

    #[test]
    fn display_frame_rejects_odd_geometry_at_16bpp() {
        let px = [0u32; 8 * 2];
        let mut out = vec![0u8; 4096];
        let mut len = 0usize;
        unsafe {
            assert_eq!(
                tactus_display_encode_frame(
                    0,
                    TactusPixelFormat::Rgb565,
                    1,
                    px.as_ptr(),
                    8,
                    2,
                    8,
                    1,
                    0,
                    4,
                    1,
                    core::ptr::null(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut len
                ),
                TactusStatus::InvalidGeometry
            );
        }
    }
}
