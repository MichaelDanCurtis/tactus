//! HID input decoding and LED output encoding for the Kontrol S-series MK3.
//!
//! # Where these layouts come from
//!
//! Every field layout in this crate was parsed from the **device's own HID
//! report descriptor**, read live off an S88 MK3 (1001 bytes, 476 HID items).
//! That makes the *structure* hardware-authoritative: field order, bit widths,
//! counts and logical ranges are what the hardware declares about itself.
//!
//! Three independent cross-checks confirm the parse:
//!
//! | Parsed | Observed on device |
//! |---|---|
//! | report `0xAA` = 62 byte payload + 1 id | `MaxInputReportSize` = 63 |
//! | report `0xA1` = 203 byte payload + 1 id | `MaxOutputReportSize` = 204 |
//! | report `0x82` = 88 bytes | 88 keys on an S88 |
//!
//! # What is structure and what is semantics
//!
//! The descriptor declares **how many** controls of **what width** each report
//! carries. It does **not** say which physical control each index is — the
//! vendor uses one opaque vendor-defined usage page (`0xFF01`) with generic
//! usage ids, so there is no "this is the Play button" in the data.
//!
//! This crate therefore exposes fields **by index, with their declared range**,
//! and does not invent names. Semantic mapping (index → "Play") requires
//! pressing each control and observing which bit changes; that belongs in a
//! device profile populated from observation, not in guesses baked into a
//! decoder. Naming a field wrongly here would be worse than leaving it
//! unnamed, because callers would build on it.
//!
//! Do not confuse this with the vendor's own report-map table, which was found
//! to **disagree with real hardware in two places** — it claims a feature
//! report `0xF8` this firmware does not implement, and omits input report
//! `0x40` which it does. Always trust the live descriptor.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use thiserror::Error;

/// USB vendor id for Native Instruments.
pub const VENDOR_ID: u16 = 0x17CC;

/// A known Kontrol S-series MK3 model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    /// 49-key.
    S49,
    /// 61-key.
    S61,
    /// 88-key.
    S88,
}

impl Model {
    /// Identify a model from its USB product id.
    pub const fn from_product_id(pid: u16) -> Option<Self> {
        match pid {
            0x2100 => Some(Self::S49),
            0x2110 => Some(Self::S61),
            0x2120 => Some(Self::S88),
            _ => None,
        }
    }

    /// USB product id.
    pub const fn product_id(self) -> u16 {
        match self {
            Self::S49 => 0x2100,
            Self::S61 => 0x2110,
            Self::S88 => 0x2120,
        }
    }

    /// Number of keys on the keybed.
    ///
    /// Also the exact length of the light-guide report payload, which is how
    /// this was confirmed: the vendor's own report map sizes report `0x82` as
    /// `0x31`/`0x3D`/`0x58` for the three models.
    pub const fn keys(self) -> usize {
        match self {
            Self::S49 => 49,
            Self::S61 => 61,
            Self::S88 => 88,
        }
    }
}

/// HID report ids declared by the device.
///
/// Verified against the live descriptor. Note the vendor's own table is wrong
/// about this set — see the crate docs.
pub mod report_id {
    /// Input: 40 single-bit controls + 2 four-bit controls.
    pub const IN_CONTROLS: u8 = 0x01;
    /// Input: touch bits, 8 continuous controls, 4 wide controls, more bits.
    pub const IN_ANALOG: u8 = 0x02;
    /// Input: bits, 3 full-range controls, 2 eleven-bit controls, a byte.
    pub const IN_WHEELS: u8 = 0x03;
    /// Input: 13 bytes, purpose unconfirmed. Absent from the vendor's table.
    pub const IN_UNKNOWN_40: u8 = 0x40;
    /// Input: 31 sixteen-bit values. The largest input report.
    pub const IN_WIDE: u8 = 0xAA;

    /// Output: 42 LED brightness values.
    pub const OUT_LEDS_A: u8 = 0x80;
    /// Output: 30 LED brightness values.
    pub const OUT_LEDS_B: u8 = 0x81;
    /// Output: per-key light guide, one byte per key.
    pub const OUT_LIGHT_GUIDE: u8 = 0x82;
    /// Output: single brightness value.
    pub const OUT_BRIGHTNESS: u8 = 0xF3;

    /// Feature: settable configuration. Reads as all zeros when unwritten.
    pub const FEAT_CONFIG: u8 = 0xD0;
    /// Feature: read-only device identity and version.
    pub const FEAT_IDENTITY: u8 = 0xD8;
    /// Feature: read-only ASCII serial number.
    pub const FEAT_SERIAL: u8 = 0xD9;
}

/// Feature report ids this firmware actually implements.
///
/// Three, not four: the vendor's shipped report-map table also claims `0xF8`,
/// which this firmware does not have. A platform layer should reject anything
/// outside this set before it reaches the device.
pub const DECLARED_FEATURE_REPORTS: [u8; 3] = [
    report_id::FEAT_CONFIG,
    report_id::FEAT_IDENTITY,
    report_id::FEAT_SERIAL,
];

/// Payload bytes in each feature report this firmware declares.
///
/// From the live report descriptor: `0xD0`, `0xD8` and `0xD9` are each 32
/// bytes. Cross-checked against the device's own `MaxFeatureReportSize` of 33
/// (32 payload + 1 report id) as reported by IOKit.
pub const FEATURE_PAYLOAD_LEN: usize = 32;

/// Buffer length for a feature-report exchange: report id plus payload.
pub const FEATURE_BUFFER_LEN: usize = FEATURE_PAYLOAD_LEN + 1;

/// Largest input report, including its report id.
///
/// Report `0xAA` is 62 payload bytes + 1 id; the device's own
/// `MaxInputReportSize` is 63, which is how the descriptor parse was validated.
pub const MAX_INPUT_REPORT_LEN: usize = 63;

/// Errors from decoding or encoding HID reports.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HidError {
    /// A report payload was not the declared length.
    #[error("report 0x{id:02X} expects {expected} payload bytes, got {got}")]
    WrongLength {
        /// Report id.
        id: u8,
        /// Payload bytes the descriptor declares.
        expected: usize,
        /// Payload bytes supplied.
        got: usize,
    },
    /// The leading byte did not match the expected report id.
    #[error("expected report id 0x{expected:02X}, got 0x{got:02X}")]
    WrongReportId {
        /// Report id expected.
        expected: u8,
        /// Report id found.
        got: u8,
    },
    /// A light-guide buffer did not match the model's key count.
    #[error("{model:?} has {keys} keys, got {got} values")]
    KeyCountMismatch {
        /// The model.
        model: Model,
        /// Keys the model has.
        keys: usize,
        /// Values supplied.
        got: usize,
    },
    /// A light-guide value exceeded the device's declared maximum.
    #[error("light guide values are 0..=127, got {value} at key {index}")]
    LightGuideOutOfRange {
        /// Offending value.
        value: u8,
        /// Key index.
        index: usize,
    },
}

/// Read `count` big-endian-indexed bits starting at `bit_offset`.
///
/// HID packs bit fields least-significant-bit first within each byte, which is
/// the opposite of how a casual reader expects, and is a classic source of
/// off-by-one-bit decoding bugs.
fn read_bits(payload: &[u8], bit_offset: usize, count: usize, out: &mut [bool]) {
    for (i, slot) in out.iter_mut().take(count).enumerate() {
        let bit = bit_offset + i;
        let byte = bit / 8;
        let shift = bit % 8;
        *slot = payload.get(byte).is_some_and(|b| (b >> shift) & 1 == 1);
    }
}

/// Read a little-endian u16 at a byte offset.
fn read_u16(payload: &[u8], at: usize) -> u16 {
    let lo = payload.get(at).copied().unwrap_or(0) as u16;
    let hi = payload.get(at + 1).copied().unwrap_or(0) as u16;
    lo | (hi << 8)
}

/// Input report `0x01`: 6 payload bytes.
///
/// Declared as 40 one-bit controls followed by 2 four-bit controls
/// (range 0..=15). The four-bit fields are plausibly relative encoders, but
/// that is **inferred from their width and range, not confirmed**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    /// 40 single-bit controls. Index-to-control mapping unconfirmed.
    pub bits: [bool; 40],
    /// 2 four-bit controls, each 0..=15.
    pub nibbles: [u8; 2],
}

impl Controls {
    /// Payload bytes, excluding the report id.
    pub const PAYLOAD_LEN: usize = 6;

    /// Decode a payload (without the leading report id).
    pub fn decode(payload: &[u8]) -> Result<Self, HidError> {
        if payload.len() != Self::PAYLOAD_LEN {
            return Err(HidError::WrongLength {
                id: report_id::IN_CONTROLS,
                expected: Self::PAYLOAD_LEN,
                got: payload.len(),
            });
        }
        let mut bits = [false; 40];
        read_bits(payload, 0, 40, &mut bits);
        let packed = payload[5];
        Ok(Self {
            bits,
            nibbles: [packed & 0x0F, packed >> 4],
        })
    }
}

/// Input report `0x02`: 27 payload bytes.
///
/// Declared as 8 one-bit controls, 8 sixteen-bit controls (0..=999),
/// 4 sixteen-bit controls (0..=4095), then 16 one-bit controls.
///
/// The eight 0..=999 fields are almost certainly the eight continuous
/// encoders — the count matches the hardware and 1000 steps is a plausible
/// resolution — but the identification is **inferred**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Analog {
    /// 8 leading single-bit controls, plausibly touch detection.
    pub leading_bits: [bool; 8],
    /// 8 continuous controls, each 0..=999.
    pub continuous: [u16; 8],
    /// 4 wider controls, each 0..=4095 (12-bit).
    pub wide: [u16; 4],
    /// 16 trailing single-bit controls.
    pub trailing_bits: [bool; 16],
}

impl Analog {
    /// Payload bytes, excluding the report id.
    pub const PAYLOAD_LEN: usize = 27;
    /// Declared maximum of the [`Self::continuous`] fields.
    pub const CONTINUOUS_MAX: u16 = 999;
    /// Declared maximum of the [`Self::wide`] fields.
    pub const WIDE_MAX: u16 = 4095;

    /// Decode a payload (without the leading report id).
    pub fn decode(payload: &[u8]) -> Result<Self, HidError> {
        if payload.len() != Self::PAYLOAD_LEN {
            return Err(HidError::WrongLength {
                id: report_id::IN_ANALOG,
                expected: Self::PAYLOAD_LEN,
                got: payload.len(),
            });
        }
        let mut leading_bits = [false; 8];
        read_bits(payload, 0, 8, &mut leading_bits);

        let mut continuous = [0u16; 8];
        for (i, slot) in continuous.iter_mut().enumerate() {
            *slot = read_u16(payload, 1 + i * 2);
        }
        let mut wide = [0u16; 4];
        for (i, slot) in wide.iter_mut().enumerate() {
            *slot = read_u16(payload, 17 + i * 2);
        }
        let mut trailing_bits = [false; 16];
        read_bits(payload, 25 * 8, 16, &mut trailing_bits);

        Ok(Self {
            leading_bits,
            continuous,
            wide,
            trailing_bits,
        })
    }
}

/// Input report `0x03`: 12 payload bytes.
///
/// Declared as 8 one-bit controls, 3 full-range sixteen-bit controls,
/// 2 eleven-bit controls (0..=2047), and one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wheels {
    /// 8 single-bit controls.
    pub bits: [bool; 8],
    /// 3 full-range controls, each 0..=65535.
    pub full_range: [u16; 3],
    /// 2 controls declared 0..=2047.
    pub eleven_bit: [u16; 2],
    /// Trailing byte, 0..=255.
    pub tail: u8,
}

impl Wheels {
    /// Payload bytes, excluding the report id.
    pub const PAYLOAD_LEN: usize = 12;

    /// Decode a payload (without the leading report id).
    pub fn decode(payload: &[u8]) -> Result<Self, HidError> {
        if payload.len() != Self::PAYLOAD_LEN {
            return Err(HidError::WrongLength {
                id: report_id::IN_WHEELS,
                expected: Self::PAYLOAD_LEN,
                got: payload.len(),
            });
        }
        let mut bits = [false; 8];
        read_bits(payload, 0, 8, &mut bits);
        let mut full_range = [0u16; 3];
        for (i, slot) in full_range.iter_mut().enumerate() {
            *slot = read_u16(payload, 1 + i * 2);
        }
        let eleven_bit = [read_u16(payload, 7), read_u16(payload, 9)];
        Ok(Self {
            bits,
            full_range,
            eleven_bit,
            tail: payload[11],
        })
    }
}

/// Any decoded input report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputReport {
    /// Report `0x01`.
    Controls(Controls),
    /// Report `0x02`.
    Analog(Analog),
    /// Report `0x03`.
    Wheels(Wheels),
    /// A report whose semantics are not yet modelled. Carries its id and
    /// declared payload length so callers can log or forward it.
    Unmodelled {
        /// Report id.
        id: u8,
        /// Payload length received.
        len: usize,
    },
}

/// Decode a complete HID input report, **including** its leading report id.
pub fn decode_input(report: &[u8]) -> Result<InputReport, HidError> {
    let (id, payload) = report.split_first().ok_or(HidError::WrongLength {
        id: 0,
        expected: 1,
        got: 0,
    })?;
    match *id {
        report_id::IN_CONTROLS => Ok(InputReport::Controls(Controls::decode(payload)?)),
        report_id::IN_ANALOG => Ok(InputReport::Analog(Analog::decode(payload)?)),
        report_id::IN_WHEELS => Ok(InputReport::Wheels(Wheels::decode(payload)?)),
        other => Ok(InputReport::Unmodelled {
            id: other,
            len: payload.len(),
        }),
    }
}

/// Maximum light-guide value the device declares.
///
/// The descriptor gives report `0x82` a logical range of **0..=127**, not
/// 0..=255. One byte per key, but only the low seven bits are valid — which is
/// consistent with a palette index rather than an intensity.
pub const LIGHT_GUIDE_MAX: u8 = 127;

/// Encode a light-guide output report for `model`.
///
/// `values` must hold exactly one entry per key, each within
/// `0..=`[`LIGHT_GUIDE_MAX`]. Writes the report id followed by the payload and
/// returns the total byte count.
///
/// Values are **validated rather than clamped**: a caller passing 200 has a
/// bug, and masking it would hide the bug while sending a value the device
/// never declared it accepts.
pub fn encode_light_guide(out: &mut [u8], model: Model, values: &[u8]) -> Result<usize, HidError> {
    let keys = model.keys();
    if values.len() != keys {
        return Err(HidError::KeyCountMismatch {
            model,
            keys,
            got: values.len(),
        });
    }
    if let Some((index, value)) = values
        .iter()
        .copied()
        .enumerate()
        .find(|&(_, v)| v > LIGHT_GUIDE_MAX)
    {
        return Err(HidError::LightGuideOutOfRange { value, index });
    }
    let total = keys + 1;
    if out.len() < total {
        return Err(HidError::WrongLength {
            id: report_id::OUT_LIGHT_GUIDE,
            expected: total,
            got: out.len(),
        });
    }
    out[0] = report_id::OUT_LIGHT_GUIDE;
    out[1..total].copy_from_slice(values);
    Ok(total)
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_buffer_sizes_match_the_devices_reported_maxima() {
        // IOKit reports MaxFeatureReportSize 33 and MaxInputReportSize 63 for
        // this firmware. Those two numbers are how the descriptor parse was
        // validated, so a change here means either the parse or the firmware
        // moved -- both worth failing a build over.
        assert_eq!(super::FEATURE_BUFFER_LEN, 33);
        assert_eq!(super::FEATURE_PAYLOAD_LEN, 32);
        assert_eq!(super::MAX_INPUT_REPORT_LEN, 63);
    }

    #[test]
    fn exactly_three_feature_reports_are_declared() {
        // The vendor's shipped table claims a fourth, 0xF8, which the live
        // descriptor does not declare at all.
        assert_eq!(super::DECLARED_FEATURE_REPORTS.len(), 3);
        assert!(!super::DECLARED_FEATURE_REPORTS.contains(&0xF8));
    }

    use super::*;

    #[test]
    fn models_round_trip_through_their_product_ids() {
        for m in [Model::S49, Model::S61, Model::S88] {
            assert_eq!(Model::from_product_id(m.product_id()), Some(m));
        }
        assert_eq!(Model::from_product_id(0x0000), None);
        // A Traktor S4 MK3 shares the vendor but is a different device.
        assert_eq!(Model::from_product_id(0x1720), None);
    }

    #[test]
    fn key_counts_match_the_light_guide_report_sizes() {
        // The vendor's report map sizes report 0x82 as 0x31/0x3D/0x58, which is
        // how the key counts were confirmed.
        assert_eq!(Model::S49.keys(), 0x31);
        assert_eq!(Model::S61.keys(), 0x3D);
        assert_eq!(Model::S88.keys(), 0x58);
    }

    #[test]
    fn hid_bit_fields_are_least_significant_bit_first() {
        // The ordering most likely to be got wrong. Bit 0 of byte 0 is the
        // first declared control, not the high bit.
        let payload = [0b0000_0001, 0, 0, 0, 0, 0];
        let c = Controls::decode(&payload).unwrap();
        assert!(c.bits[0], "first control is bit 0 of byte 0");
        assert!(!c.bits[1]);

        let payload = [0b1000_0000, 0, 0, 0, 0, 0];
        let c = Controls::decode(&payload).unwrap();
        assert!(!c.bits[0]);
        assert!(c.bits[7], "eighth control is bit 7 of byte 0");

        // Control 8 is bit 0 of byte 1.
        let payload = [0, 0b0000_0001, 0, 0, 0, 0];
        let c = Controls::decode(&payload).unwrap();
        assert!(c.bits[8]);
    }

    #[test]
    fn controls_decodes_all_forty_bits_and_both_nibbles() {
        let payload = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x5A];
        let c = Controls::decode(&payload).unwrap();
        assert!(c.bits.iter().all(|&b| b), "all 40 controls set");
        // Low nibble first, then high.
        assert_eq!(c.nibbles, [0x0A, 0x05]);
    }

    #[test]
    fn controls_rejects_a_wrong_length_payload() {
        assert!(matches!(
            Controls::decode(&[0; 5]),
            Err(HidError::WrongLength {
                expected: 6,
                got: 5,
                ..
            })
        ));
        assert!(matches!(
            Controls::decode(&[0; 7]),
            Err(HidError::WrongLength {
                expected: 6,
                got: 7,
                ..
            })
        ));
    }

    #[test]
    fn analog_reads_little_endian_u16_fields() {
        let mut payload = [0u8; Analog::PAYLOAD_LEN];
        payload[0] = 0b0000_0011; // two leading bits
        // First continuous field = 999 = 0x03E7, little-endian.
        payload[1] = 0xE7;
        payload[2] = 0x03;
        // First wide field = 4095 = 0x0FFF.
        payload[17] = 0xFF;
        payload[18] = 0x0F;

        let a = Analog::decode(&payload).unwrap();
        assert!(a.leading_bits[0] && a.leading_bits[1] && !a.leading_bits[2]);
        assert_eq!(a.continuous[0], Analog::CONTINUOUS_MAX);
        assert_eq!(a.wide[0], Analog::WIDE_MAX);
    }

    #[test]
    fn analog_field_offsets_do_not_overlap() {
        // Distinct marker in each field slot must land in exactly one field.
        let mut payload = [0u8; Analog::PAYLOAD_LEN];
        for i in 0..8 {
            payload[1 + i * 2] = (i + 1) as u8;
        }
        for i in 0..4 {
            payload[17 + i * 2] = (i + 100) as u8;
        }
        let a = Analog::decode(&payload).unwrap();
        assert_eq!(a.continuous, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(a.wide, [100, 101, 102, 103]);
    }

    #[test]
    fn wheels_decodes_its_declared_layout() {
        let mut payload = [0u8; Wheels::PAYLOAD_LEN];
        payload[1] = 0xFF;
        payload[2] = 0xFF; // full_range[0] = 65535
        payload[7] = 0xFF;
        payload[8] = 0x07; // eleven_bit[0] = 2047
        payload[11] = 0x42;
        let w = Wheels::decode(&payload).unwrap();
        assert_eq!(w.full_range[0], u16::MAX);
        assert_eq!(w.eleven_bit[0], 2047);
        assert_eq!(w.tail, 0x42);
    }

    #[test]
    fn decode_input_dispatches_on_the_report_id() {
        let mut r = vec![report_id::IN_CONTROLS];
        r.extend_from_slice(&[0u8; 6]);
        assert!(matches!(decode_input(&r), Ok(InputReport::Controls(_))));

        let mut r = vec![report_id::IN_ANALOG];
        r.extend_from_slice(&[0u8; 27]);
        assert!(matches!(decode_input(&r), Ok(InputReport::Analog(_))));

        let mut r = vec![report_id::IN_WHEELS];
        r.extend_from_slice(&[0u8; 12]);
        assert!(matches!(decode_input(&r), Ok(InputReport::Wheels(_))));
    }

    #[test]
    fn an_unmodelled_report_is_surfaced_not_dropped() {
        // 0x40 and 0xAA are real reports we have not characterised. Silently
        // discarding them would hide device behaviour from callers.
        let mut r = vec![report_id::IN_WIDE];
        r.extend_from_slice(&[0u8; 62]);
        assert_eq!(
            decode_input(&r),
            Ok(InputReport::Unmodelled { id: 0xAA, len: 62 })
        );
    }

    #[test]
    fn decode_input_rejects_an_empty_report() {
        assert!(matches!(
            decode_input(&[]),
            Err(HidError::WrongLength { .. })
        ));
    }

    #[test]
    fn light_guide_encodes_id_then_one_byte_per_key() {
        let values = vec![7u8; 88];
        let mut out = [0u8; 89];
        let n = encode_light_guide(&mut out, Model::S88, &values).unwrap();
        assert_eq!(n, 89, "88 keys + 1 report id");
        assert_eq!(out[0], 0x82);
        assert!(out[1..].iter().all(|&v| v == 7));
    }

    #[test]
    fn light_guide_enforces_the_models_key_count() {
        let mut out = [0u8; 128];
        // An S88 buffer sent to an S49 is a bug, not something to truncate.
        let values = vec![0u8; 88];
        assert!(matches!(
            encode_light_guide(&mut out, Model::S49, &values),
            Err(HidError::KeyCountMismatch {
                keys: 49,
                got: 88,
                ..
            })
        ));
    }

    #[test]
    fn light_guide_rejects_out_of_range_values_rather_than_masking() {
        // The descriptor declares 0..=127. Masking 200 to 72 would send a
        // wrong colour while hiding the caller's bug.
        let mut values = vec![0u8; 88];
        values[40] = 200;
        let mut out = [0u8; 89];
        assert!(matches!(
            encode_light_guide(&mut out, Model::S88, &values),
            Err(HidError::LightGuideOutOfRange {
                value: 200,
                index: 40
            })
        ));
        // The declared maximum itself is accepted.
        values[40] = LIGHT_GUIDE_MAX;
        assert!(encode_light_guide(&mut out, Model::S88, &values).is_ok());
    }

    #[test]
    fn light_guide_rejects_a_short_output_buffer() {
        let values = vec![0u8; 88];
        let mut out = [0u8; 10];
        assert!(matches!(
            encode_light_guide(&mut out, Model::S88, &values),
            Err(HidError::WrongLength {
                expected: 89,
                got: 10,
                ..
            })
        ));
    }

    #[test]
    fn payload_lengths_match_the_parsed_descriptor() {
        // Guards against someone editing a layout without re-reading the
        // descriptor. These are the byte counts the device declares.
        assert_eq!(Controls::PAYLOAD_LEN, 6);
        assert_eq!(Analog::PAYLOAD_LEN, 27);
        assert_eq!(Wheels::PAYLOAD_LEN, 12);
    }
}
