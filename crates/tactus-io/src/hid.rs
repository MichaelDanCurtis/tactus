//! Real HID I/O against an attached MK3 keyboard.
//!
//! This module moves bytes. Every byte's *meaning* comes from `tactus-hid`:
//! input reports are handed to [`tactus_hid::decode_input`] and light-guide
//! values to [`tactus_hid::encode_light_guide`]. Nothing here parses or builds
//! a report field itself, so there is exactly one place where a layout can be
//! wrong.
//!
//! # Sharing the device with the vendor's software
//!
//! On macOS `hidapi` opens HID devices with `kIOHIDOptionsTypeSeizeDevice` by
//! default — it **takes the device away** from whoever else has it, which would
//! silently break a running Komplete Kontrol session. This module therefore
//! turns that off for every context it creates (see [`open_api`]), so a
//! `tactus` process coexists with the vendor's software instead of displacing
//! it. If the platform still refuses the open, that is reported as
//! [`IoError::Open`] with the likely cause named, and the other process is
//! left alone.

use std::ffi::{CStr, CString};
use std::time::Duration;

use hidapi::HidApi;
use tactus_hid::{InputReport, Model, VENDOR_ID, decode_input, encode_light_guide, report_id};

use crate::IoError;

// Buffer sizes and the declared feature-report set are facts the *device*
// declares in its report descriptor, not properties of this I/O layer, so they
// live in `tactus-hid` alongside the rest of the protocol. They are re-exported
// here because every caller of this module needs them to size a buffer, and a
// Linux or Windows I/O layer must use the same numbers rather than its own copy.
pub use tactus_hid::{
    DECLARED_FEATURE_REPORTS, FEATURE_BUFFER_LEN, FEATURE_PAYLOAD_LEN, MAX_INPUT_REPORT_LEN,
};

/// Light-guide output buffer: one byte per key on the largest model, plus the
/// report id.
const LIGHT_GUIDE_BUFFER_LEN: usize = Model::S88.keys() + 1;

/// Create a HID context.
///
/// `hidapi` 2.x permits several contexts per process and never deinitialises
/// the C library, so this is cheap to call per operation and a [`Device`] does
/// not have to keep one alive.
fn open_api() -> Result<HidApi, IoError> {
    let api = HidApi::new().map_err(|source| IoError::HidInit { source })?;
    // Do not seize the device from Komplete Kontrol or the NI agents. See the
    // module docs: this is the default on macOS and it is the wrong default
    // for a tool that is meant to observe hardware somebody else is using.
    #[cfg(target_os = "macos")]
    api.set_open_exclusive(false);
    Ok(api)
}

/// An attached MK3 keyboard, before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Model, from the USB product id.
    pub model: Model,
    /// USB product id.
    pub product_id: u16,
    /// Serial from the **USB string descriptor**, where the platform reports
    /// one.
    ///
    /// This is not the same string as [`Device::serial`]: the USB descriptor on
    /// an S88 MK3 carries a short id (observed: `99FE7169`) while feature
    /// report `0xD9` carries a longer one. Both are device-reported; they are
    /// different fields and must not be conflated.
    pub usb_serial: Option<String>,
    /// USB product string, e.g. `KONTROL S88 MK3`.
    pub product_string: Option<String>,
    /// Platform HID path. Opaque, and only valid until the device is replugged.
    pub path: CString,
    /// USB interface number, or `-1` where the platform does not report one.
    pub interface_number: i32,
}

impl DeviceInfo {
    /// The HID path as a lossy [`String`], for messages and logs.
    pub fn path_lossy(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// A one-line description for disambiguation messages.
    pub fn describe(&self) -> String {
        match &self.usb_serial {
            Some(s) => format!("{:?} (usb serial {s}) at {}", self.model, self.path_lossy()),
            None => format!("{:?} at {}", self.model, self.path_lossy()),
        }
    }
}

/// Enumerate attached Kontrol S-series MK3 keyboards.
///
/// Only Native Instruments devices with a product id `tactus-hid` recognises
/// are returned; unknown NI products are skipped rather than guessed at.
pub fn list_devices() -> Result<Vec<DeviceInfo>, IoError> {
    let api = open_api()?;
    Ok(collect(&api))
}

fn collect(api: &HidApi) -> Vec<DeviceInfo> {
    let mut found: Vec<DeviceInfo> = Vec::new();
    for raw in api.device_list() {
        if raw.vendor_id() != VENDOR_ID {
            continue;
        }
        let Some(model) = Model::from_product_id(raw.product_id()) else {
            continue;
        };
        let info = DeviceInfo {
            model,
            product_id: raw.product_id(),
            usb_serial: raw.serial_number().map(str::to_owned),
            product_string: raw.product_string().map(str::to_owned),
            path: raw.path().to_owned(),
            interface_number: raw.interface_number(),
        };
        // macOS can list one entry per top-level HID collection; the path is
        // what identifies a thing we can open, so collapse duplicates.
        if !found.iter().any(|existing| existing.path == info.path) {
            found.push(info);
        }
    }
    found
}

/// How many HID devices the platform listed, for diagnostics only.
fn examined(api: &HidApi) -> usize {
    api.device_list().count()
}

fn describe_all(devices: &[DeviceInfo]) -> String {
    devices
        .iter()
        .map(DeviceInfo::describe)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Pick exactly one device, or say why that was not possible.
fn sole(
    devices: Vec<DeviceInfo>,
    model: Option<Model>,
    api: &HidApi,
) -> Result<DeviceInfo, IoError> {
    let count = devices.len();
    if count > 1 {
        return Err(IoError::AmbiguousDevice {
            model,
            count,
            candidates: describe_all(&devices),
        });
    }
    match devices.into_iter().next() {
        Some(info) => Ok(info),
        None => Err(match model {
            Some(model) => IoError::ModelNotFound {
                model,
                found: collect(api).len(),
            },
            None => IoError::NoDevice {
                vendor: VENDOR_ID,
                examined: examined(api),
            },
        }),
    }
}

/// An open MK3 keyboard.
///
/// Dropping it closes the HID handle.
pub struct Device {
    inner: hidapi::HidDevice,
    info: DeviceInfo,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").field("info", &self.info).finish()
    }
}

impl Device {
    /// Open a specific enumerated keyboard.
    pub fn open(info: &DeviceInfo) -> Result<Self, IoError> {
        let api = open_api()?;
        Self::open_with(&api, info)
    }

    fn open_with(api: &HidApi, info: &DeviceInfo) -> Result<Self, IoError> {
        let inner = api.open_path(&info.path).map_err(|source| IoError::Open {
            model: info.model,
            path: info.path_lossy(),
            source,
        })?;
        Ok(Self {
            inner,
            info: info.clone(),
        })
    }

    /// Open the only attached MK3 keyboard, whichever model it is.
    ///
    /// Errors with [`IoError::AmbiguousDevice`] when more than one is attached,
    /// rather than choosing for the caller.
    pub fn open_only() -> Result<Self, IoError> {
        let api = open_api()?;
        let info = sole(collect(&api), None, &api)?;
        Self::open_with(&api, &info)
    }

    /// Open the only attached keyboard of `model`.
    pub fn open_model(model: Model) -> Result<Self, IoError> {
        let api = open_api()?;
        let matching: Vec<DeviceInfo> = collect(&api)
            .into_iter()
            .filter(|d| d.model == model)
            .collect();
        let info = sole(matching, Some(model), &api)?;
        Self::open_with(&api, &info)
    }

    /// The model, from the USB product id.
    pub fn model(&self) -> Model {
        self.info.model
    }

    /// Everything enumeration knew about this device.
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// The HID path this device was opened from.
    pub fn path(&self) -> &CStr {
        &self.info.path
    }

    /// Whether this handle seized the device from other processes (macOS only).
    ///
    /// Always `false` for a handle this crate opened — the policy is to share,
    /// never to seize. Exposed so that policy is *checkable on hardware*
    /// rather than merely asserted in a comment: see the live-hardware test.
    #[cfg(target_os = "macos")]
    pub fn is_open_exclusive(&self) -> Result<bool, IoError> {
        self.inner
            .is_open_exclusive()
            .map_err(|source| IoError::Hid {
                op: "is_open_exclusive",
                source,
            })
    }

    /// Read the device's serial number from feature report `0xD9`.
    ///
    /// Read live on each call rather than cached at open: a cache can only be
    /// stale, and reading at open would make opening fail for a device that is
    /// otherwise perfectly usable.
    ///
    /// The returned string is **device-supplied data**. Treat it as data —
    /// display it, compare it, store it — never as a command or a path.
    pub fn serial(&self) -> Result<String, IoError> {
        let mut buf = [0u8; FEATURE_BUFFER_LEN];
        let read = self.read_feature(report_id::FEAT_SERIAL, &mut buf)?;
        let payload = buf.get(1..read).ok_or(IoError::FeatureReportTooShort {
            id: report_id::FEAT_SERIAL,
            got: read,
        })?;
        decode_ascii(payload, report_id::FEAT_SERIAL)
    }

    /// Read one of the three feature reports this firmware declares.
    ///
    /// `out` must hold at least [`FEATURE_BUFFER_LEN`] bytes. On return it
    /// holds the report id followed by the payload, and the count includes that
    /// leading id — which is how `0xD8` reads back as `D8 05 01 00 13 ...`.
    ///
    /// Undeclared ids are refused here rather than passed to the platform, so a
    /// caller who trusted the vendor's table gets a useful message instead of a
    /// backend error.
    pub fn read_feature(&self, id: u8, out: &mut [u8]) -> Result<usize, IoError> {
        check_feature_request(id, out.len())?;
        match out.first_mut() {
            Some(first) => *first = id,
            None => return Err(IoError::feature_buffer_too_small(id, 0)),
        }
        let read = self
            .inner
            .get_feature_report(out)
            .map_err(|source| IoError::Hid {
                op: "get_feature_report",
                source,
            })?;
        if read == 0 {
            return Err(IoError::FeatureReportTooShort { id, got: 0 });
        }
        match out.first() {
            Some(&got) if got == id => Ok(read),
            Some(&got) => Err(IoError::FeatureReportIdMismatch { id, got }),
            None => Err(IoError::FeatureReportTooShort { id, got: read }),
        }
    }

    /// Writing a feature report is **not supported**, by policy.
    ///
    /// This method exists so the refusal is discoverable where somebody would
    /// look for the capability, and always returns
    /// [`IoError::FeatureWriteRefused`].
    ///
    /// Of the three declared feature reports, `0xD8` and `0xD9` are `[Const]`
    /// — read-only. `0xD0` is `[Data]`, i.e. settable, and reads all zeros even
    /// with the vendor's software running, so **nothing is known about what
    /// writing it does**. Its first fields have the shape of a size pair and a
    /// percentage, which is exactly the kind of resemblance that invites a
    /// guess; guessing here writes unverified configuration into somebody's
    /// instrument. There is no safe default, so there is no write.
    pub fn write_feature(&self, id: u8, _data: &[u8]) -> Result<(), IoError> {
        Err(IoError::FeatureWriteRefused { id })
    }

    /// Read and decode one HID input report, waiting up to `timeout`.
    ///
    /// `Ok(None)` means the timeout expired with nothing to read, which is the
    /// normal state of an untouched keyboard — it is **not** an error and not
    /// evidence that the read path is broken.
    ///
    /// Reports the firmware declares but this SDK does not model yet come back
    /// as [`InputReport::Unmodelled`] rather than being dropped.
    pub fn poll_input(&self, timeout: Duration) -> Result<Option<InputReport>, IoError> {
        let mut buf = [0u8; MAX_INPUT_REPORT_LEN];
        // hidapi takes milliseconds as i32 and treats negatives as "block
        // forever"; saturate rather than wrap into an accidental block.
        let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let read = self
            .inner
            .read_timeout(&mut buf, millis)
            .map_err(|source| IoError::Hid {
                op: "read_timeout",
                source,
            })?;
        if read == 0 {
            return Ok(None);
        }
        let report = buf.get(..read).ok_or(IoError::Internal {
            what: "input report longer than MAX_INPUT_REPORT_LEN",
        })?;
        Ok(Some(decode_input(report)?))
    }

    /// Set the light guide: one value per key, each `0..=127`.
    ///
    /// Validation and framing are [`tactus_hid::encode_light_guide`]'s job;
    /// this only writes the bytes it returns. Out-of-range values are rejected,
    /// not clamped, so a caller bug surfaces instead of lighting a wrong
    /// colour.
    pub fn set_light_guide(&self, values: &[u8]) -> Result<(), IoError> {
        let mut buf = [0u8; LIGHT_GUIDE_BUFFER_LEN];
        let total = encode_light_guide(&mut buf, self.info.model, values)?;
        let report = buf.get(..total).ok_or(IoError::Internal {
            what: "encoded light guide exceeds LIGHT_GUIDE_BUFFER_LEN",
        })?;
        let sent = self.inner.write(report).map_err(|source| IoError::Hid {
            op: "write light guide",
            source,
        })?;
        if sent == total {
            Ok(())
        } else {
            Err(IoError::ShortWrite {
                id: report_id::OUT_LIGHT_GUIDE,
                sent,
                expected: total,
            })
        }
    }

    /// Turn every light-guide LED off.
    ///
    /// All zeros: in range by definition, and visually neutral — the one
    /// light-guide write that cannot leave the instrument in a surprising
    /// state.
    pub fn clear_light_guide(&self) -> Result<(), IoError> {
        let zeros = [0u8; LIGHT_GUIDE_BUFFER_LEN];
        let keys = zeros
            .get(..self.info.model.keys())
            .ok_or(IoError::Internal {
                what: "key count exceeds LIGHT_GUIDE_BUFFER_LEN",
            })?;
        self.set_light_guide(keys)
    }
}

/// Validate a feature-report request before it reaches the platform.
///
/// Split out from [`Device::read_feature`] so the rules are testable without
/// hardware, which is most of what can go wrong here.
pub(crate) fn check_feature_request(id: u8, out_len: usize) -> Result<(), IoError> {
    if !DECLARED_FEATURE_REPORTS.contains(&id) {
        return Err(IoError::undeclared_feature(id));
    }
    if out_len < FEATURE_BUFFER_LEN {
        return Err(IoError::feature_buffer_too_small(id, out_len));
    }
    Ok(())
}

/// Decode a NUL/`0xFF`-padded printable-ASCII feature payload.
///
/// Padding is where the string stops. Any other byte outside printable ASCII is
/// an error: a serial that has been quietly mangled still looks like a serial,
/// and would be compared against, logged and filed as if it were real.
pub(crate) fn decode_ascii(payload: &[u8], id: u8) -> Result<String, IoError> {
    let mut text = String::with_capacity(payload.len());
    for (index, &byte) in payload.iter().enumerate() {
        match byte {
            0x00 | 0xFF => break,
            0x20..=0x7E => text.push(char::from(byte)),
            other => {
                return Err(IoError::SerialNotAscii {
                    id,
                    byte: other,
                    index,
                });
            }
        }
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(IoError::SerialEmpty { id });
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_light_guide_buffer_fits_the_widest_keybed() {
        // The protocol constants are asserted against the device's reported
        // maxima in `tactus-hid`; this one is sized by this module.
        assert_eq!(LIGHT_GUIDE_BUFFER_LEN, 89);
        assert_eq!(LIGHT_GUIDE_BUFFER_LEN, Model::S88.keys() + 1);
    }

    #[test]
    fn only_the_three_declared_feature_reports_are_accepted() {
        for id in DECLARED_FEATURE_REPORTS {
            assert!(
                check_feature_request(id, FEATURE_BUFFER_LEN).is_ok(),
                "0x{id:02X} is declared"
            );
        }
        // 0xF8: claimed by the vendor's table, absent from this firmware.
        // 0x82: a real report id, but an output report, not a feature report.
        for id in [0xF8u8, 0x82, 0x00, 0xD1, 0xFF] {
            let err = check_feature_request(id, FEATURE_BUFFER_LEN).unwrap_err();
            assert!(
                matches!(err, IoError::UndeclaredFeatureReport { .. }),
                "0x{id:02X} must be refused, got {err:?}"
            );
        }
    }

    #[test]
    fn feature_buffer_must_hold_id_plus_payload() {
        // One byte short is the interesting case: 32 bytes is the payload
        // length, and a caller who sizes to the payload loses the last byte.
        let err = check_feature_request(report_id::FEAT_IDENTITY, FEATURE_PAYLOAD_LEN).unwrap_err();
        match err {
            IoError::FeatureBufferTooSmall { need, got, .. } => {
                assert_eq!(need, 33);
                assert_eq!(got, 32);
            }
            other => panic!("expected FeatureBufferTooSmall, got {other:?}"),
        }
        assert!(check_feature_request(report_id::FEAT_IDENTITY, 0).is_err());
        assert!(check_feature_request(report_id::FEAT_IDENTITY, 64).is_ok());
    }

    #[test]
    fn id_validation_precedes_buffer_validation() {
        // An undeclared id with a tiny buffer should complain about the id:
        // fixing the buffer would not help.
        let err = check_feature_request(0xF8, 1).unwrap_err();
        assert!(
            matches!(err, IoError::UndeclaredFeatureReport { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn serial_decodes_to_the_padding() {
        let mut payload = [0u8; FEATURE_PAYLOAD_LEN];
        payload[..4].copy_from_slice(b"KS8F");
        assert_eq!(decode_ascii(&payload, 0xD9).unwrap(), "KS8F");

        // 0xFF padding, as seen in report 0xD8's tail.
        let mut ff = [0xFFu8; FEATURE_PAYLOAD_LEN];
        ff[..3].copy_from_slice(b"ABC");
        assert_eq!(decode_ascii(&ff, 0xD9).unwrap(), "ABC");

        // Surrounding whitespace is trimmed, interior content is not touched.
        assert_eq!(decode_ascii(b"  A B  \0", 0xD9).unwrap(), "A B");
    }

    #[test]
    fn serial_rejects_rather_than_mangles() {
        // A high byte is not padding and not ASCII: report it.
        let err = decode_ascii(b"KS8\x81F", 0xD9).unwrap_err();
        match err {
            IoError::SerialNotAscii { byte, index, .. } => {
                assert_eq!(byte, 0x81);
                assert_eq!(index, 3);
            }
            other => panic!("expected SerialNotAscii, got {other:?}"),
        }
        // Control characters are not silently dropped either.
        assert!(decode_ascii(b"AB\x07C", 0xD9).is_err());
    }

    #[test]
    fn all_padding_is_not_a_serial() {
        assert!(matches!(
            decode_ascii(&[0u8; 32], 0xD9),
            Err(IoError::SerialEmpty { .. })
        ));
        assert!(matches!(
            decode_ascii(b"   \0", 0xD9),
            Err(IoError::SerialEmpty { .. })
        ));
        assert!(matches!(
            decode_ascii(&[], 0xD9),
            Err(IoError::SerialEmpty { .. })
        ));
    }

    #[test]
    fn light_guide_validation_is_delegated_not_duplicated() {
        // Proof that this crate's buffer is big enough for the real encoder and
        // that the protocol crate's rules are the ones in force.
        let mut buf = [0u8; LIGHT_GUIDE_BUFFER_LEN];
        let total = encode_light_guide(&mut buf, Model::S88, &[0u8; 88]).unwrap();
        assert_eq!(total, LIGHT_GUIDE_BUFFER_LEN);
        assert_eq!(buf.first(), Some(&report_id::OUT_LIGHT_GUIDE));

        // Wrong key count and out-of-range values both come back as protocol
        // errors through IoError's From impl.
        let short: IoError = encode_light_guide(&mut buf, Model::S88, &[0u8; 61])
            .unwrap_err()
            .into();
        assert!(short.to_string().contains("88 keys"), "{short}");

        let mut hot = [0u8; 88];
        hot[12] = 128;
        let loud: IoError = encode_light_guide(&mut buf, Model::S88, &hot)
            .unwrap_err()
            .into();
        assert!(loud.to_string().contains("0..=127"), "{loud}");
    }

    #[test]
    fn describe_names_the_model_and_path() {
        let info = DeviceInfo {
            model: Model::S88,
            product_id: 0x2120,
            usb_serial: Some("99FE7169".to_owned()),
            product_string: Some("KONTROL S88 MK3".to_owned()),
            path: CString::new("DevSrvsID:4294970000").unwrap(),
            interface_number: -1,
        };
        let described = info.describe();
        assert!(described.contains("S88"), "{described}");
        assert!(described.contains("99FE7169"), "{described}");
        assert!(described.contains("DevSrvsID"), "{described}");

        let anonymous = DeviceInfo {
            usb_serial: None,
            ..info
        };
        assert!(!anonymous.describe().contains("usb serial"));
    }

    #[test]
    fn the_refusal_variant_names_the_report() {
        // Device::write_feature needs hardware to call, but the error it always
        // returns must name the report a caller tried to write.
        let err = IoError::FeatureWriteRefused {
            id: report_id::FEAT_CONFIG,
        };
        assert!(matches!(err, IoError::FeatureWriteRefused { id } if id == 0xD0));
        assert!(err.to_string().contains("0xD0"), "{err}");
    }
}
