//! C ABI for platform device I/O — opening real hardware, reading input,
//! driving the light guide, and talking MIDI.
//!
//! The rest of this crate exposes pure protocol functions: give them bytes,
//! they give you meaning. This module is the part that touches somebody's
//! instrument, so it adds three rules to the crate-wide contract.
//!
//! # Handles
//!
//! [`TactusDevice`], [`TactusDeviceList`], [`TactusMidiOut`] and
//! [`TactusMidiIn`] are **opaque**. A caller holds a pointer and passes it
//! back; the layout is deliberately not in the header, so adding a field is
//! not an ABI break.
//!
//! Every handle has exactly one `_free` function, and freeing is what closes
//! the underlying device. Passing null to a `_free` is safe and does nothing,
//! so cleanup paths need no null checks. Using a handle after freeing it is
//! undefined behaviour — the usual C rule, stated because the consequence here
//! is a live USB handle.
//!
//! A handle must not be used from two threads at once. Nothing here locks, and
//! the HID and MIDI backends underneath make no such promise.
//!
//! # Diagnostics
//!
//! The I/O layer's errors name the value that caused them — which model was
//! absent, which report id was refused, how many devices were ambiguous. A
//! status code cannot carry that, so every entry point in this module records
//! the message for [`crate::tactus_last_error_message`] and clears it on
//! entry. Checking a status tells a caller what happened; the message tells
//! their user what to do about it.
//!
//! # What is deliberately absent
//!
//! There is no function here that writes a feature report, and none that sends
//! display or page data. Those are not omissions to be filled in later by a
//! caller who knows better: `0xD0` is settable configuration with unknown
//! semantics, and nothing about a host-side MK3 framebuffer is verified.
//! [`tactus_device_write_feature`] exists only to refuse, so a caller who
//! looks for it finds the reason rather than silence.

use std::ffi::{CStr, CString};
use std::slice;
use std::time::Duration;

use tactus_core::CoreError;
use tactus_hid::{HidError, Model};
use tactus_io::{Device, DeviceInfo, IoError, MidiIn, MidiOut, list_devices};
use tactus_midi::{MidiError, PortRole, daw::Command};

use crate::{
    TactusInputReport, TactusModel, TactusStatus, clear_last_error, guard, set_last_error,
};

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Map an [`IoError`] to a status code.
///
/// The grouping is by *what a caller would do differently*, not one code per
/// variant: a missing device and a missing model both mean "plug something
/// in", while a refusal and a transport failure call for opposite responses —
/// one must never be retried, the other may succeed on a retry. The specific
/// value that failed stays in the message.
fn status_of(error: &IoError) -> TactusStatus {
    match error {
        IoError::NoDevice { .. } | IoError::ModelNotFound { .. } => TactusStatus::DeviceNotFound,
        IoError::AmbiguousDevice { .. } => TactusStatus::AmbiguousDevice,
        IoError::Open { .. } | IoError::MidiConnect { .. } => TactusStatus::OpenFailed,

        // Policy, not failure. Retrying is pointless by construction.
        IoError::UndeclaredFeatureReport { .. } | IoError::FeatureWriteRefused { .. } => {
            TactusStatus::Refused
        }

        IoError::FeatureBufferTooSmall { .. } => TactusStatus::BufferTooSmall,

        IoError::HidInit { .. }
        | IoError::MidiInit { .. }
        | IoError::Hid { .. }
        | IoError::MidiPortInfo { .. }
        | IoError::MidiSend { .. }
        | IoError::ShortWrite { .. }
        | IoError::FeatureReportTooShort { .. }
        | IoError::FeatureReportIdMismatch { .. }
        | IoError::SerialNotAscii { .. }
        | IoError::SerialEmpty { .. } => TactusStatus::IoFailure,

        IoError::Protocol(e) => match e {
            HidError::LightGuideOutOfRange { .. } => TactusStatus::OutOfRange,
            HidError::WrongLength { .. }
            | HidError::WrongReportId { .. }
            | HidError::KeyCountMismatch { .. } => TactusStatus::InvalidArgument,
        },
        IoError::Midi(e) => match e {
            MidiError::NonAsciiText { .. } => TactusStatus::NonAsciiText,
            MidiError::OutputTooSmall { .. } => TactusStatus::BufferTooSmall,
            MidiError::PortNotFound { .. } => TactusStatus::PortNotFound,
            MidiError::AmbiguousPort { .. } => TactusStatus::AmbiguousDevice,
            MidiError::NotADataByte { .. } => TactusStatus::InvalidArgument,
        },
        IoError::Core(e) => match e {
            CoreError::BufferTooSmall { .. } => TactusStatus::BufferTooSmall,
            CoreError::RectOutOfBounds { .. } | CoreError::ZeroSizedSurface { .. } => {
                TactusStatus::InvalidGeometry
            }
        },

        IoError::Internal { .. } => TactusStatus::Internal,

        // `IoError` is `#[non_exhaustive]`, so a variant added later lands
        // here. It maps to the generic I/O bucket rather than `Internal`: a
        // new platform failure is not an invariant violation in this crate,
        // and labelling it one would send a user hunting for a bug that is
        // really a USB cable. The message still carries the specifics.
        _ => TactusStatus::IoFailure,
    }
}

/// Record an error's own message, then return its status code.
fn fail(error: &IoError) -> TactusStatus {
    set_last_error(error);
    status_of(error)
}

/// Record a message for a failure this module detects itself, with no
/// [`IoError`] behind it.
fn fail_with(status: TactusStatus, message: &str) -> TactusStatus {
    set_last_error(message);
    status
}

/// Wrap an entry point: clear any stale diagnostic, then run under the
/// crate's panic guard.
fn entry<F: FnOnce() -> TactusStatus>(f: F) -> TactusStatus {
    guard(|| {
        clear_last_error();
        f()
    })
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

/// A snapshot of the attached keyboards.
///
/// Opaque. Created by [`tactus_device_list_new`], released by
/// [`tactus_device_list_free`]. The strings handed out by
/// [`tactus_device_list_get`] are owned by this list and stay valid until it
/// is freed.
pub struct TactusDeviceList {
    entries: Vec<ListEntry>,
}

struct ListEntry {
    info: DeviceInfo,
    /// NUL-terminated copies, because the platform hands us Rust strings and
    /// a C caller needs terminators. Stored so the pointers we lend out have
    /// an owner with a lifetime the caller can reason about.
    usb_serial: Option<CString>,
    product_string: Option<CString>,
}

/// One enumerated keyboard.
///
/// # String lifetimes
///
/// `usb_serial`, `product_string` and `path` point into the
/// [`TactusDeviceList`] this came from. They are valid until that list is
/// freed, and must not be freed individually. Copy anything you need to keep.
/// A field the platform did not report is null — check before dereferencing.
///
/// `usb_serial` is **not** the same string as
/// [`tactus_device_serial`]: the USB descriptor carries a short id (observed
/// `99FE7169` on an S88 MK3) while feature report `0xD9` carries a longer one.
/// Both are device-reported, and conflating them produces a device id that
/// silently depends on which call you made.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TactusDeviceInfo {
    /// Model, from the USB product id.
    pub model: TactusModel,
    /// USB product id.
    pub product_id: u16,
    /// USB interface number, or `-1` where the platform does not report one.
    pub interface_number: i32,
    /// Serial from the USB string descriptor, or null.
    pub usb_serial: *const core::ffi::c_char,
    /// USB product string, e.g. `KONTROL S88 MK3`, or null.
    pub product_string: *const core::ffi::c_char,
    /// Opaque platform HID path. Only valid until the device is replugged.
    pub path: *const core::ffi::c_char,
}

fn model_to_ffi(model: Model) -> TactusModel {
    match model {
        Model::S49 => TactusModel::S49,
        Model::S61 => TactusModel::S61,
        Model::S88 => TactusModel::S88,
    }
}

fn model_from_ffi(model: TactusModel) -> Model {
    match model {
        TactusModel::S49 => Model::S49,
        TactusModel::S61 => Model::S61,
        TactusModel::S88 => Model::S88,
    }
}

/// Enumerate attached Kontrol S-series MK3 keyboards.
///
/// Succeeds with a count of zero when nothing is attached: an empty bus is a
/// normal state, not an error. `*out` receives a list that must be released
/// with [`tactus_device_list_free`].
///
/// # Safety
///
/// `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_list_new(out: *mut *mut TactusDeviceList) -> TactusStatus {
    entry(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        let infos = match list_devices() {
            Ok(v) => v,
            Err(e) => return fail(&e),
        };
        let entries = infos
            .into_iter()
            .map(|info| ListEntry {
                usb_serial: info
                    .usb_serial
                    .as_deref()
                    .and_then(|s| CString::new(s).ok()),
                product_string: info
                    .product_string
                    .as_deref()
                    .and_then(|s| CString::new(s).ok()),
                info,
            })
            .collect();
        let boxed = Box::new(TactusDeviceList { entries });
        unsafe { *out = Box::into_raw(boxed) };
        TactusStatus::Ok
    })
}

/// How many keyboards the list holds. Returns `0` for a null list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_list_count(list: *const TactusDeviceList) -> usize {
    if list.is_null() {
        return 0;
    }
    let list = unsafe { &*list };
    list.entries.len()
}

/// Read one entry.
///
/// # Safety
///
/// `list` must be a live list and `out` writable. The strings written into
/// `*out` borrow from `list`; see [`TactusDeviceInfo`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_list_get(
    list: *const TactusDeviceList,
    index: usize,
    out: *mut TactusDeviceInfo,
) -> TactusStatus {
    entry(|| {
        if list.is_null() || out.is_null() {
            return TactusStatus::NullArgument;
        }
        let list = unsafe { &*list };
        let Some(e) = list.entries.get(index) else {
            return fail_with(
                TactusStatus::OutOfRange,
                "device index past the end of the list",
            );
        };
        let info = TactusDeviceInfo {
            model: model_to_ffi(e.info.model),
            product_id: e.info.product_id,
            interface_number: e.info.interface_number,
            usb_serial: e
                .usb_serial
                .as_ref()
                .map_or(core::ptr::null(), |c| c.as_ptr()),
            product_string: e
                .product_string
                .as_ref()
                .map_or(core::ptr::null(), |c| c.as_ptr()),
            path: e.info.path.as_ptr(),
        };
        unsafe { *out = info };
        TactusStatus::Ok
    })
}

/// Release a device list. Passing null is safe and does nothing.
///
/// # Safety
///
/// `list` must come from [`tactus_device_list_new`] and must not have been
/// freed already. Any [`TactusDeviceInfo`] strings taken from it are dangling
/// afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_list_free(list: *mut TactusDeviceList) {
    if list.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(list) });
}

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

/// An open keyboard.
///
/// Opaque. Released — and the HID handle closed — by
/// [`tactus_device_free`].
pub struct TactusDevice {
    inner: Device,
}

/// Open the keyboard at `index` in an enumerated list.
///
/// Preferred over opening by path: the caller never has to handle a platform
/// path string, and the device identity they inspected is the one they get.
///
/// # Safety
///
/// `list` must be live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_open_at(
    list: *const TactusDeviceList,
    index: usize,
    out: *mut *mut TactusDevice,
) -> TactusStatus {
    entry(|| {
        if list.is_null() || out.is_null() {
            return TactusStatus::NullArgument;
        }
        let list = unsafe { &*list };
        let Some(e) = list.entries.get(index) else {
            return fail_with(
                TactusStatus::OutOfRange,
                "device index past the end of the list",
            );
        };
        match Device::open(&e.info) {
            Ok(inner) => {
                unsafe { *out = Box::into_raw(Box::new(TactusDevice { inner })) };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Open the only attached keyboard.
///
/// Fails with [`TactusStatus::AmbiguousDevice`] when more than one is present,
/// rather than picking one — with two keyboards attached, a silent choice
/// sends a user's output to the wrong instrument.
///
/// # Safety
///
/// `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_open_only(out: *mut *mut TactusDevice) -> TactusStatus {
    entry(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        match Device::open_only() {
            Ok(inner) => {
                unsafe { *out = Box::into_raw(Box::new(TactusDevice { inner })) };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Open the single attached keyboard of a given model.
///
/// # Safety
///
/// `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_open_model(
    model: TactusModel,
    out: *mut *mut TactusDevice,
) -> TactusStatus {
    entry(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        match Device::open_model(model_from_ffi(model)) {
            Ok(inner) => {
                unsafe { *out = Box::into_raw(Box::new(TactusDevice { inner })) };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Close a device and release its handle. Passing null is safe.
///
/// # Safety
///
/// `device` must come from one of the `tactus_device_open_*` functions and
/// must not have been freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_free(device: *mut TactusDevice) {
    if device.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(device) });
}

// ---------------------------------------------------------------------------
// Device identity and state
// ---------------------------------------------------------------------------

/// The model of an open device.
///
/// # Safety
///
/// `device` must be live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_model(
    device: *const TactusDevice,
    out: *mut TactusModel,
) -> TactusStatus {
    entry(|| {
        if device.is_null() || out.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        unsafe { *out = model_to_ffi(device.inner.model()) };
        TactusStatus::Ok
    })
}

/// Whether this handle was opened exclusively, taking the device away from
/// other processes.
///
/// Should always report `false`. macOS HID opens are exclusive by default —
/// `hidapi`'s `hid_init()` calls `hid_darwin_set_open_exclusive(1)` — which
/// would silently break a running vendor session, so every context this SDK
/// creates turns that off. This function exists so a host can *check* that on
/// real hardware instead of taking a comment's word for it.
///
/// # Safety
///
/// `device` must be live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_is_open_exclusive(
    device: *const TactusDevice,
    out: *mut bool,
) -> TactusStatus {
    entry(|| {
        if device.is_null() || out.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        match device.inner.is_open_exclusive() {
            Ok(v) => {
                unsafe { *out = v };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Read the serial number from feature report `0xD9`.
///
/// Written NUL-terminated into `out`; `out_len`, when non-null, receives the
/// length excluding the terminator. Pass a null `out` with a non-null
/// `out_len` to ask for the required length first.
///
/// This is **not** the USB descriptor serial in [`TactusDeviceInfo`]; the two
/// are different device-reported fields.
///
/// The returned text is **data**: display it, compare it, log it. It is not a
/// path or a command, and nothing in this SDK treats it as one.
///
/// # Safety
///
/// `device` must be live. `out` must be writable for `out_cap` bytes or null;
/// `out_len` must be writable or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_serial(
    device: *const TactusDevice,
    out: *mut core::ffi::c_char,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    entry(|| {
        if device.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        let serial = match device.inner.serial() {
            Ok(s) => s,
            Err(e) => return fail(&e),
        };
        unsafe { write_c_string(&serial, out, out_cap, out_len) }
    })
}

/// Copy `text` into a caller buffer as a NUL-terminated C string.
///
/// # Safety
///
/// `out` must be writable for `out_cap` bytes or null; `out_len` writable or
/// null.
unsafe fn write_c_string(
    text: &str,
    out: *mut core::ffi::c_char,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if !out_len.is_null() {
        unsafe { *out_len = len };
    }
    if out.is_null() {
        return if out_len.is_null() {
            TactusStatus::NullArgument
        } else {
            TactusStatus::Ok
        };
    }
    if out_cap < len + 1 {
        return fail_with(
            TactusStatus::BufferTooSmall,
            "buffer too small for the string; call again with the reported length plus one",
        );
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
}

// ---------------------------------------------------------------------------
// Feature reports
// ---------------------------------------------------------------------------

/// Read a feature report.
///
/// Only the three report ids this firmware declares — `0xD0`, `0xD8`, `0xD9` —
/// are accepted; anything else is [`TactusStatus::Refused`] without reaching
/// the device. That includes `0xF8`, which the vendor's own published
/// report-map table claims and the live descriptor does not declare at all.
///
/// `out` must hold at least `TACTUS_FEATURE_BUFFER_LEN` bytes. On success
/// `*out_len` receives the byte count, and `out[0]` is the report id the
/// device echoed back.
///
/// # Safety
///
/// `device` must be live, `out` writable for `out_cap` bytes, `out_len`
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_read_feature(
    device: *const TactusDevice,
    report_id: u8,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> TactusStatus {
    entry(|| {
        if device.is_null() || out.is_null() || out_len.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        let buf = unsafe { slice::from_raw_parts_mut(out, out_cap) };
        match device.inner.read_feature(report_id, buf) {
            Ok(n) => {
                unsafe { *out_len = n };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Always fails with [`TactusStatus::Refused`]. Writing feature reports is not
/// supported.
///
/// This function exists so the refusal is discoverable at the ABI rather than
/// looking like an oversight. `0xD8` and `0xD9` are read-only. `0xD0` is
/// settable and reads back as all zeros on a device with the vendor's software
/// running, so not one of its 32 bytes has a known meaning — writing a guess
/// to a configuration register on somebody's instrument is not a thing this
/// SDK will do.
///
/// # Safety
///
/// Arguments are not dereferenced; the call always fails.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_write_feature(
    _device: *mut TactusDevice,
    report_id: u8,
    _data: *const u8,
    _data_len: usize,
) -> TactusStatus {
    entry(|| {
        set_last_error(format!(
            "writing feature report 0x{report_id:02X} is not supported: \
             0xD8 and 0xD9 are read-only and 0xD0 has no verified field semantics"
        ));
        TactusStatus::Refused
    })
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Wait up to `timeout_ms` for an input report.
///
/// On success `*got` is `true` and `*out` holds the decoded report; `*got` is
/// `false` when the timeout expired with nothing to read. **A quiet keyboard
/// is not an error** — an idle instrument is the normal case, and reporting it
/// as a failure would make every host's main loop log noise.
///
/// A report this SDK does not model yet arrives with
/// `kind == TactusInputKind_Unmodelled` and a meaningful `report_id`, rather
/// than being dropped or guessed at.
///
/// # Safety
///
/// `device` must be live; `out` and `got` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_poll_input(
    device: *const TactusDevice,
    timeout_ms: u32,
    out: *mut TactusInputReport,
    got: *mut bool,
) -> TactusStatus {
    entry(|| {
        if device.is_null() || out.is_null() || got.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        match device
            .inner
            .poll_input(Duration::from_millis(u64::from(timeout_ms)))
        {
            Ok(Some(report)) => {
                let id = report_id_of(&report);
                unsafe {
                    *out = TactusInputReport::from_decoded(id, report);
                    *got = true;
                }
                TactusStatus::Ok
            }
            Ok(None) => {
                unsafe { *got = false };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// The report id a decoded report came from.
fn report_id_of(report: &tactus_hid::InputReport) -> u8 {
    use tactus_hid::{InputReport, report_id};
    match report {
        InputReport::Controls(_) => report_id::IN_CONTROLS,
        InputReport::Analog(_) => report_id::IN_ANALOG,
        InputReport::Wheels(_) => report_id::IN_WHEELS,
        InputReport::Unmodelled { id, .. } => *id,
    }
}

// ---------------------------------------------------------------------------
// Light guide
// ---------------------------------------------------------------------------

/// Set the per-key light guide.
///
/// `len` must equal the device's key count — 49, 61 or 88 — so a buffer sized
/// for the wrong model is refused rather than silently truncated. Each value
/// is `0..=TACTUS_LIGHT_GUIDE_MAX` (127); an out-of-range value is refused
/// rather than masked, because masking turns a caller's bug into a wrong
/// colour on the hardware and hides it.
///
/// # Safety
///
/// `device` must be live and `values` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_set_light_guide(
    device: *const TactusDevice,
    values: *const u8,
    len: usize,
) -> TactusStatus {
    entry(|| {
        if device.is_null() || values.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        let values = unsafe { slice::from_raw_parts(values, len) };
        match device.inner.set_light_guide(values) {
            Ok(()) => TactusStatus::Ok,
            Err(e) => fail(&e),
        }
    })
}

/// Turn every light-guide LED off.
///
/// # Safety
///
/// `device` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_device_clear_light_guide(
    device: *const TactusDevice,
) -> TactusStatus {
    entry(|| {
        if device.is_null() {
            return TactusStatus::NullArgument;
        }
        let device = unsafe { &*device };
        match device.inner.clear_light_guide() {
            Ok(()) => TactusStatus::Ok,
            Err(e) => fail(&e),
        }
    })
}

// ---------------------------------------------------------------------------
// MIDI
// ---------------------------------------------------------------------------

/// Which of the keyboard's three MIDI ports to use.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TactusPortRole {
    /// Keys, aftertouch, pedals, wheels — ordinary performance data.
    Main = 0,
    /// Host integration: the SysEx command set and channel-16 control changes.
    Daw = 1,
    /// External MIDI routed through the instrument.
    Ext = 2,
}

fn role_from_ffi(role: TactusPortRole) -> PortRole {
    match role {
        TactusPortRole::Main => PortRole::Main,
        TactusPortRole::Daw => PortRole::Daw,
        TactusPortRole::Ext => PortRole::Ext,
    }
}

/// Read an optional C string argument as a filter.
///
/// # Safety
///
/// `filter` must be NUL-terminated or null.
unsafe fn filter_str(filter: *const core::ffi::c_char) -> Result<String, TactusStatus> {
    if filter.is_null() {
        return Ok(String::new());
    }
    match unsafe { CStr::from_ptr(filter) }.to_str() {
        Ok(s) => Ok(s.to_owned()),
        Err(_) => Err(fail_with(
            TactusStatus::InvalidArgument,
            "device filter is not valid UTF-8",
        )),
    }
}

/// An open MIDI output port.
///
/// Opaque. Released — and the port closed — by [`tactus_midi_out_free`].
pub struct TactusMidiOut {
    inner: MidiOut,
}

/// Open a MIDI output port by role.
///
/// `device_filter` narrows the match by device name — pass `"S88"` to pick one
/// model, or null for no filter. Two matching keyboards yield
/// [`TactusStatus::AmbiguousDevice`] rather than a guess.
///
/// # Safety
///
/// `device_filter` must be NUL-terminated or null; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_out_open(
    role: TactusPortRole,
    device_filter: *const core::ffi::c_char,
    out: *mut *mut TactusMidiOut,
) -> TactusStatus {
    entry(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        let filter = match unsafe { filter_str(device_filter) } {
            Ok(f) => f,
            Err(s) => return s,
        };
        match MidiOut::open(role_from_ffi(role), &filter) {
            Ok(inner) => {
                unsafe { *out = Box::into_raw(Box::new(TactusMidiOut { inner })) };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Send raw MIDI bytes.
///
/// Nothing here validates the message — a caller who has already encoded one
/// with [`crate::tactus_daw_encode`] should not have it re-checked. Use
/// [`tactus_midi_out_send_command`] to have the framing and data-byte
/// validation done for you.
///
/// # Safety
///
/// `out` must be live and `bytes` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_out_send(
    out: *mut TactusMidiOut,
    bytes: *const u8,
    len: usize,
) -> TactusStatus {
    entry(|| {
        if out.is_null() || bytes.is_null() {
            return TactusStatus::NullArgument;
        }
        let port = unsafe { &mut *out };
        let bytes = unsafe { slice::from_raw_parts(bytes, len) };
        match port.inner.send_raw(bytes) {
            Ok(()) => TactusStatus::Ok,
            Err(e) => fail(&e),
        }
    })
}

/// Encode and send a DAW-port SysEx command.
///
/// `command` is one of the ids in [`crate::tactus_daw_encode`]'s table.
/// Payload bytes must all be `<= 0x7F`; a byte with its status bit set is
/// refused rather than sent, because it would be read as a new status byte and
/// corrupt the rest of the stream.
///
/// # Safety
///
/// `out` must be live; `payload` readable for `payload_len` bytes, or null
/// when `payload_len` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_out_send_command(
    out: *mut TactusMidiOut,
    command: u8,
    value: u8,
    track: u8,
    payload: *const u8,
    payload_len: usize,
) -> TactusStatus {
    entry(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        if payload.is_null() && payload_len != 0 {
            return TactusStatus::NullArgument;
        }
        let Some(command) = Command::from_id(command) else {
            return fail_with(
                TactusStatus::InvalidArgument,
                "unrecognised DAW command id; see the command table in tactus.h",
            );
        };
        let port = unsafe { &mut *out };
        let payload = if payload_len == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(payload, payload_len) }
        };
        match port.inner.send_command(command, value, track, payload) {
            Ok(()) => TactusStatus::Ok,
            Err(e) => fail(&e),
        }
    })
}

/// Encode and send a DAW-port SysEx command carrying text.
///
/// `text` must be NUL-terminated and 7-bit ASCII. Non-ASCII is refused with
/// [`TactusStatus::NonAsciiText`] rather than mangled: the device's text
/// payloads are one byte per character with no length prefix, so anything
/// above `U+007F` cannot be represented without corrupting the frame.
/// Transliterate before calling.
///
/// # Safety
///
/// `out` must be live and `text` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_out_send_text(
    out: *mut TactusMidiOut,
    command: u8,
    value: u8,
    track: u8,
    text: *const core::ffi::c_char,
) -> TactusStatus {
    entry(|| {
        if out.is_null() || text.is_null() {
            return TactusStatus::NullArgument;
        }
        let Some(command) = Command::from_id(command) else {
            return fail_with(
                TactusStatus::InvalidArgument,
                "unrecognised DAW command id; see the command table in tactus.h",
            );
        };
        let Ok(text) = unsafe { CStr::from_ptr(text) }.to_str() else {
            return fail_with(
                TactusStatus::NonAsciiText,
                "text is not valid UTF-8, so it cannot be ASCII either",
            );
        };
        let port = unsafe { &mut *out };
        match port.inner.send_text(command, value, track, text) {
            Ok(()) => TactusStatus::Ok,
            Err(e) => fail(&e),
        }
    })
}

/// Close a MIDI output port. Passing null is safe.
///
/// # Safety
///
/// `out` must come from [`tactus_midi_out_open`] and must not have been freed
/// already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_out_free(out: *mut TactusMidiOut) {
    if out.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(out) });
}

/// An open MIDI input port.
///
/// Opaque. Incoming messages queue as they arrive; drain them with
/// [`tactus_midi_in_recv`]. Released by [`tactus_midi_in_free`].
pub struct TactusMidiIn {
    inner: MidiIn,
}

/// Open a MIDI input port by role.
///
/// # Safety
///
/// `device_filter` must be NUL-terminated or null; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_in_open(
    role: TactusPortRole,
    device_filter: *const core::ffi::c_char,
    out: *mut *mut TactusMidiIn,
) -> TactusStatus {
    entry(|| {
        if out.is_null() {
            return TactusStatus::NullArgument;
        }
        let filter = match unsafe { filter_str(device_filter) } {
            Ok(f) => f,
            Err(s) => return s,
        };
        match MidiIn::open(role_from_ffi(role), &filter) {
            Ok(inner) => {
                unsafe { *out = Box::into_raw(Box::new(TactusMidiIn { inner })) };
                TactusStatus::Ok
            }
            Err(e) => fail(&e),
        }
    })
}

/// Take the next queued MIDI message, waiting up to `timeout_ms`.
///
/// `*got` is `false` when the timeout expired with nothing queued — a silent
/// port is normal, not an error. On success `*out_len` holds the message
/// length.
///
/// A message longer than `out_cap` yields [`TactusStatus::BufferTooSmall`]
/// with `*out_len` set to the length needed. **The message is consumed** in
/// that case: it has already left the queue, so size the buffer for the
/// longest SysEx you expect rather than relying on a retry.
///
/// # Safety
///
/// `in_port` must be live; `out` writable for `out_cap` bytes; `out_len` and
/// `got` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_in_recv(
    in_port: *const TactusMidiIn,
    timeout_ms: u32,
    out: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
    got: *mut bool,
) -> TactusStatus {
    entry(|| {
        if in_port.is_null() || out.is_null() || out_len.is_null() || got.is_null() {
            return TactusStatus::NullArgument;
        }
        let port = unsafe { &*in_port };
        let Some(message) = port
            .inner
            .recv_timeout(Duration::from_millis(u64::from(timeout_ms)))
        else {
            unsafe { *got = false };
            return TactusStatus::Ok;
        };
        unsafe { *out_len = message.len() };
        if out_cap < message.len() {
            unsafe { *got = false };
            return fail_with(
                TactusStatus::BufferTooSmall,
                "buffer too small for the received message, which has been discarded",
            );
        }
        let dst = unsafe { slice::from_raw_parts_mut(out, message.len()) };
        dst.copy_from_slice(&message);
        unsafe { *got = true };
        TactusStatus::Ok
    })
}

/// Close a MIDI input port. Passing null is safe.
///
/// # Safety
///
/// `in_port` must come from [`tactus_midi_in_open`] and must not have been
/// freed already.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tactus_midi_in_free(in_port: *mut TactusMidiIn) {
    if in_port.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(in_port) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_handles_are_refused_not_dereferenced() {
        // Every one of these would be a segfault if the null check were
        // missing, which is why they are asserted rather than assumed.
        let mut model = TactusModel::S49;
        let mut flag = false;
        let mut len = 0usize;
        let mut report = TactusInputReport::zeroed();
        unsafe {
            assert_eq!(
                tactus_device_list_new(core::ptr::null_mut()),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_device_list_get(core::ptr::null(), 0, core::ptr::null_mut()),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_device_model(core::ptr::null(), &mut model),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_device_is_open_exclusive(core::ptr::null(), &mut flag),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_device_serial(core::ptr::null(), core::ptr::null_mut(), 0, &mut len),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_device_poll_input(core::ptr::null(), 0, &mut report, &mut flag),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_device_clear_light_guide(core::ptr::null()),
                TactusStatus::NullArgument
            );
            assert_eq!(
                tactus_midi_out_send(core::ptr::null_mut(), core::ptr::null(), 0),
                TactusStatus::NullArgument
            );
        }
    }

    #[test]
    fn freeing_null_handles_is_a_no_op() {
        // So a caller's cleanup path needs no null checks of its own.
        unsafe {
            tactus_device_list_free(core::ptr::null_mut());
            tactus_device_free(core::ptr::null_mut());
            tactus_midi_out_free(core::ptr::null_mut());
            tactus_midi_in_free(core::ptr::null_mut());
        }
    }

    #[test]
    fn counting_a_null_list_is_zero_not_a_crash() {
        assert_eq!(unsafe { tactus_device_list_count(core::ptr::null()) }, 0);
    }

    #[test]
    fn writing_a_feature_report_always_refuses_and_says_why() {
        let status = unsafe {
            tactus_device_write_feature(core::ptr::null_mut(), 0xD0, core::ptr::null(), 0)
        };
        assert_eq!(status, TactusStatus::Refused);

        let mut len = 0usize;
        let sized = unsafe { crate::tactus_last_error_message(core::ptr::null_mut(), 0, &mut len) };
        assert_eq!(sized, TactusStatus::Ok);
        assert!(len > 0, "a refusal must explain itself");

        let mut buf = vec![0i8; len + 1];
        let got = unsafe {
            crate::tactus_last_error_message(buf.as_mut_ptr(), buf.len(), core::ptr::null_mut())
        };
        assert_eq!(got, TactusStatus::Ok);
        let text = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
        assert!(text.contains("0xD0"), "names the report id: {text}");
        assert!(
            text.contains("read-only") || text.contains("semantics"),
            "gives the reason: {text}"
        );
    }

    #[test]
    fn a_too_small_error_buffer_reports_the_length_it_needs() {
        set_last_error("a message of some length");
        let mut len = 0usize;
        let mut tiny = [0i8; 2];
        let status =
            unsafe { crate::tactus_last_error_message(tiny.as_mut_ptr(), tiny.len(), &mut len) };
        assert_eq!(status, TactusStatus::BufferTooSmall);
        assert_eq!(len, "a message of some length".len());
    }

    #[test]
    fn the_error_message_is_cleared_on_entry_so_it_is_never_stale() {
        set_last_error("stale message from an earlier call");
        // A null-argument rejection records nothing of its own.
        let mut model = TactusModel::S49;
        let status = unsafe { tactus_device_model(core::ptr::null(), &mut model) };
        assert_eq!(status, TactusStatus::NullArgument);

        let mut len = 1usize;
        let sized = unsafe { crate::tactus_last_error_message(core::ptr::null_mut(), 0, &mut len) };
        assert_eq!(sized, TactusStatus::Ok);
        assert_eq!(len, 0, "the stale message must not survive the next call");
    }

    #[test]
    fn an_embedded_nul_does_not_discard_the_diagnostic() {
        // Device-supplied strings reach these messages. A NUL in one must not
        // cost us the whole diagnostic.
        set_last_error("port \0 name");
        let mut len = 0usize;
        unsafe { crate::tactus_last_error_message(core::ptr::null_mut(), 0, &mut len) };
        assert_eq!(len, "port ? name".len());
    }

    #[test]
    fn models_round_trip_through_the_abi() {
        for model in [Model::S49, Model::S61, Model::S88] {
            assert_eq!(model_from_ffi(model_to_ffi(model)), model);
        }
    }

    #[test]
    fn roles_map_to_every_port_the_device_has() {
        let mapped: Vec<PortRole> = [
            TactusPortRole::Main,
            TactusPortRole::Daw,
            TactusPortRole::Ext,
        ]
        .into_iter()
        .map(role_from_ffi)
        .collect();
        assert_eq!(mapped, PortRole::ALL.to_vec());
    }

    #[test]
    fn a_refusal_and_a_transport_failure_are_distinct_codes() {
        // A caller must be able to tell "never retry this" from "this might
        // work next time"; collapsing them into one code would make a policy
        // refusal look like a flaky cable.
        let refused = status_of(&IoError::FeatureWriteRefused { id: 0xD0 });
        let transport = status_of(&IoError::ShortWrite {
            id: 0x82,
            sent: 1,
            expected: 89,
        });
        assert_eq!(refused, TactusStatus::Refused);
        assert_eq!(transport, TactusStatus::IoFailure);
        assert_ne!(refused, transport);
    }

    #[test]
    fn an_out_of_range_light_guide_value_keeps_its_own_code() {
        // Not folded into InvalidArgument: the caller's fix is to clamp a
        // value, which is different from passing the wrong buffer.
        let status = status_of(&IoError::Protocol(HidError::LightGuideOutOfRange {
            index: 3,
            value: 200,
        }));
        assert_eq!(status, TactusStatus::OutOfRange);
    }

    #[test]
    fn every_status_code_has_a_name() {
        for code in [-1i32, -2, -3, -4, -5, -6, -7, -8, -9, -10, -11, -12, -100] {
            let ptr = crate::tactus_status_name(code);
            let name = unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned();
            assert_ne!(name, "unknown", "status {code} has no name");
        }
    }
}
