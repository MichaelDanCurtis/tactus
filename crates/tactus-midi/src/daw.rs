//! The DAW-port protocol: SysEx commands, handshake, and control changes.
//!
//! # Provenance
//!
//! **Evidence class: vendor source.** Recovered from Native Instruments' own
//! MIDI Remote script, shipped in plaintext as
//! `NativeInstruments_Kontrol_S_Mk3.js` (1066 lines, unobfuscated). That makes
//! the command ids and control assignments *authoritative as NI's intent* —
//! considerably stronger than inference from captures.
//!
//! It is **not hardware-verified**. No session has been observed on the wire,
//! and NI's own tables have already been found to disagree with real MK3
//! hardware elsewhere (see the HID report map). Treat these constants as
//! expected values to confirm, not as proven.
//!
//! The script declares `protocolVersion = 0x04` and targets a specific host
//! version, so **firmware is a load-bearing variable**: the version byte in
//! the handshake is the first thing to check against a real device. Note this
//! is a different layer from the `protocol version 2.4.0` reported by NI's
//! hardware service — do not conflate them.
//!
//! # Framing
//!
//! ```text
//! F0 00 21 09 00 00 44 43 01 00 <command> <value> <track> [payload...] F7
//! ```
//!
//! `00 21 09` is Native Instruments' MIDI manufacturer id. The following six
//! bytes are a fixed sub-header; its internal field split is **unknown**, so it
//! is treated as an opaque constant.
//!
//! All channel messages use **MIDI channel 16** (status `0xBF`).

use crate::MidiError;

/// The complete SysEx prefix, up to but excluding the command byte.
pub const SYSEX_HEADER: [u8; 10] = [0xF0, 0x00, 0x21, 0x09, 0x00, 0x00, 0x44, 0x43, 0x01, 0x00];

/// SysEx terminator.
pub const SYSEX_END: u8 = 0xF7;

/// Control-change status byte for the DAW port: channel 16.
pub const STATUS_CC: u8 = 0xBF;

/// The protocol version this command set was recovered from.
pub const PROTOCOL_VERSION: u8 = 0x04;

/// Highest legal value for any MIDI data byte.
///
/// A payload byte of `0x80` or above would be read as a new status byte and
/// corrupt the rest of the stream. This is the invariant every encoder here
/// enforces.
pub const MAX_DATA_BYTE: u8 = 0x7F;

/// A DAW-port SysEx command.
///
/// Discriminants are NI's own `sysexIds` values. `0x4A` is absent from their
/// table; its absence is not evidence that the device ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Command {
    /// Surface configuration, e.g. navigation orientation.
    SurfaceConfiguration = 0x03,
    /// Host identity announcement.
    Identity = 0x07,
    /// Set tempo.
    SetTempo = 0x19,
    /// Whether a track slot is populated.
    TrackEnabled = 0x40,
    /// Focus-follow state.
    FocusFollow = 0x41,
    /// Track selection state.
    TrackSelected = 0x42,
    /// Mute button state.
    MuteButton = 0x43,
    /// Solo button state.
    SoloButton = 0x44,
    /// Record-arm state.
    TrackArmed = 0x45,
    /// Track volume.
    Volume = 0x46,
    /// Track pan.
    Pan = 0x47,
    /// Track name (text payload).
    TrackName = 0x48,
    /// VU meter level.
    VuMeter = 0x49,
    /// Track colour.
    TrackColor = 0x4B,
    /// Plugin selection on the selected track.
    SelectedTrackSelectPlugin = 0x70,
    /// Plugin chain information.
    SelectedTrackPluginChainInfo = 0x71,
    /// Parameter name (text payload).
    SelectedTrackParamName = 0x72,
    /// Parameter display value (text payload).
    SelectedTrackParamDisplayValue = 0x73,
    /// Parameter page count/index.
    SelectedTrackParamPageNum = 0x74,
    /// Parameter value adjustment. Observed device to host.
    AdjustParameterValue = 0x7F,
}

impl Command {
    /// The command byte.
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Recognise a command byte, or `None` if it is not in NI's table.
    ///
    /// Total by design: an unrecognised command must not panic a host, because
    /// a future firmware may add one.
    pub const fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0x03 => Self::SurfaceConfiguration,
            0x07 => Self::Identity,
            0x19 => Self::SetTempo,
            0x40 => Self::TrackEnabled,
            0x41 => Self::FocusFollow,
            0x42 => Self::TrackSelected,
            0x43 => Self::MuteButton,
            0x44 => Self::SoloButton,
            0x45 => Self::TrackArmed,
            0x46 => Self::Volume,
            0x47 => Self::Pan,
            0x48 => Self::TrackName,
            0x49 => Self::VuMeter,
            0x4B => Self::TrackColor,
            0x70 => Self::SelectedTrackSelectPlugin,
            0x71 => Self::SelectedTrackPluginChainInfo,
            0x72 => Self::SelectedTrackParamName,
            0x73 => Self::SelectedTrackParamDisplayValue,
            0x74 => Self::SelectedTrackParamPageNum,
            0x7F => Self::AdjustParameterValue,
            _ => return None,
        })
    }

    /// Whether this command's payload is text.
    pub const fn takes_text(self) -> bool {
        matches!(
            self,
            Self::Identity
                | Self::SurfaceConfiguration
                | Self::TrackName
                | Self::SelectedTrackParamName
                | Self::SelectedTrackParamDisplayValue
        )
    }
}

/// Encode a SysEx command with a raw payload.
///
/// Every payload byte must be `<= `[`MAX_DATA_BYTE`]. Returns bytes written.
pub fn encode(
    out: &mut [u8],
    command: Command,
    value: u8,
    track: u8,
    payload: &[u8],
) -> Result<usize, MidiError> {
    for (index, &b) in payload.iter().enumerate() {
        if b > MAX_DATA_BYTE {
            return Err(MidiError::NotADataByte { byte: b, index });
        }
    }
    if value > MAX_DATA_BYTE {
        return Err(MidiError::NotADataByte {
            byte: value,
            index: 0,
        });
    }
    if track > MAX_DATA_BYTE {
        return Err(MidiError::NotADataByte {
            byte: track,
            index: 1,
        });
    }

    let total = SYSEX_HEADER.len() + 3 + payload.len() + 1;
    if out.len() < total {
        return Err(MidiError::OutputTooSmall {
            got: out.len(),
            need: total,
        });
    }
    let mut n = 0;
    out[n..n + SYSEX_HEADER.len()].copy_from_slice(&SYSEX_HEADER);
    n += SYSEX_HEADER.len();
    out[n] = command.id();
    out[n + 1] = value;
    out[n + 2] = track;
    n += 3;
    out[n..n + payload.len()].copy_from_slice(payload);
    n += payload.len();
    out[n] = SYSEX_END;
    Ok(n + 1)
}

/// Encode a SysEx command whose payload is text.
///
/// # Encoding, and its limits
///
/// NI's script builds text payloads with JavaScript `charCodeAt`, one byte per
/// UTF-16 code unit, with **no length prefix** — the payload runs to the
/// terminating `F7`.
///
/// That is only safe for **7-bit ASCII**. A character above `U+007F` yields a
/// code unit `>= 0x80`, which as a SysEx data byte would be read as a status
/// byte and corrupt the stream. NI's script does not guard against this; a
/// track named "Café" would break it.
///
/// This function therefore **rejects** non-ASCII rather than silently
/// truncating or mangling it. Callers wanting best-effort display should
/// transliterate before calling — a deliberate choice, because a host that
/// mangles names quietly is worse than one that tells you it cannot send them.
pub fn encode_text(
    out: &mut [u8],
    command: Command,
    value: u8,
    track: u8,
    text: &str,
) -> Result<usize, MidiError> {
    if let Some((index, ch)) = text.char_indices().find(|(_, c)| !c.is_ascii()) {
        return Err(MidiError::NonAsciiText { ch, index });
    }
    encode(out, command, value, track, text.as_bytes())
}

/// The handshake, in the order NI's script sends it.
pub mod handshake {
    use super::{PROTOCOL_VERSION, STATUS_CC};

    /// Announce the host and request the protocol version.
    pub const HELLO: [u8; 3] = [STATUS_CC, 0x01, PROTOCOL_VERSION];
    /// Opt in to 14-bit SysEx parameter updates.
    pub const ENABLE_14BIT_PARAMS: [u8; 3] = [STATUS_CC, 0x06, 0x01];
    /// Relinquish the surface.
    pub const GOODBYE: [u8; 3] = [STATUS_CC, 0x02, 0x00];
}

/// Control-change assignments on the DAW port, all channel 16.
///
/// Names are NI's own, from their script's variable names. Device-to-host
/// unless noted.
pub mod cc {
    /// Shift modifier.
    pub const SHIFT: u8 = 0x04;
    /// Bank mapping select. Range 0..1: 0 = mixer tracks, 1 = plugin parameters.
    pub const BANK_MAPPING: u8 = 0x05;

    /// Transport: play. Bidirectional — also carries LED feedback.
    pub const PLAY: u8 = 0x10;
    /// Transport: record. Bidirectional.
    pub const RECORD: u8 = 0x12;
    /// Transport: count-in. Toggle.
    pub const COUNT_IN: u8 = 0x13;
    /// Transport: stop. Bidirectional.
    pub const STOP: u8 = 0x14;
    /// Transport: loop. Bidirectional.
    pub const LOOP: u8 = 0x16;
    /// Transport: metronome. Bidirectional.
    pub const METRONOME: u8 = 0x17;
    /// Transport: tap tempo.
    pub const TAP_TEMPO: u8 = 0x18;

    /// Undo.
    pub const UNDO: u8 = 0x20;
    /// Redo.
    pub const REDO: u8 = 0x21;
    /// Quantize.
    pub const QUANTIZE: u8 = 0x22;
    /// Automation.
    pub const AUTO: u8 = 0x23;

    /// Track navigation enable, host to device. Two-bit mask: prev | next.
    pub const NAV_TRACK: u8 = 0x30;
    /// Bank navigation enable, host to device.
    pub const NAV_BANK: u8 = 0x31;
    /// Clip navigation enable, host to device.
    pub const NAV_CLIP: u8 = 0x32;

    /// Transport position encoder. Relative.
    pub const MOVE_TRANSPORT: u8 = 0x34;
    /// Loop position encoder. Relative.
    pub const MOVE_LOOP: u8 = 0x35;

    /// Per-channel select, raw bitfield across channels.
    pub const SELECT_RAW: u8 = 0x42;
    /// Per-channel mute, raw bitfield.
    pub const MUTE_RAW: u8 = 0x43;
    /// Per-channel solo, raw bitfield.
    pub const SOLO_RAW: u8 = 0x44;

    /// Selected-track volume encoder. Relative.
    pub const SELECTED_VOLUME: u8 = 0x64;
    /// Selected-track pan encoder. Relative.
    pub const SELECTED_PAN: u8 = 0x65;

    /// Base of the eight per-channel volume displays, host to device.
    pub const VOLUME_DISPLAY_BASE: u8 = 0x50;
    /// Base of the eight per-channel pan displays, host to device.
    pub const PAN_DISPLAY_BASE: u8 = 0x58;
    /// Base of the eight per-channel parameter values, host to device.
    pub const PARAM_VALUE_BASE: u8 = 0x70;

    /// Number of channels in a bank.
    pub const CHANNELS: u8 = 8;

    /// Capability flag: undo available, host to device.
    pub const CAN_UNDO: u8 = 0x20;
    /// Capability flag: redo available.
    pub const CAN_REDO: u8 = 0x21;
    /// Capability flag: quantize available.
    pub const CAN_QUANTIZE: u8 = 0x22;

    /// A per-channel CC, given a base and a channel index.
    ///
    /// Returns `None` for an index outside the bank, rather than silently
    /// addressing the wrong control.
    pub const fn per_channel(base: u8, index: u8) -> Option<u8> {
        if index >= CHANNELS {
            return None;
        }
        Some(base + index)
    }
}

/// Decode a relative ("two's complement") encoder value into a signed delta.
///
/// NI's script marks the navigation and selected-parameter encoders
/// `setTypeRelativeTwosComplement`. Values `0x01..=0x3F` are positive deltas;
/// `0x41..=0x7F` are negative, read as a 7-bit two's complement.
///
/// Getting this wrong makes an encoder appear to jump by 127 when turned down
/// by one, which is why it has its own function and its own tests.
pub const fn decode_relative(value: u8) -> i8 {
    let v = value & MAX_DATA_BYTE;
    if v < 0x40 {
        v as i8
    } else {
        (v as i16 - 128) as i8
    }
}

/// Encode a signed delta as a relative encoder value.
///
/// Deltas outside `-64..=63` cannot be represented in seven bits and are
/// rejected rather than wrapped.
pub const fn encode_relative(delta: i8) -> Option<u8> {
    if delta >= 0 {
        if delta > 0x3F {
            None
        } else {
            Some(delta as u8)
        }
    } else if delta < -64 {
        None
    } else {
        Some(((delta as i16) + 128) as u8 & MAX_DATA_BYTE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sysex_header_is_nis_manufacturer_id_plus_a_fixed_subheader() {
        // 00 21 09 is Native Instruments. The rest is opaque but constant.
        assert_eq!(&SYSEX_HEADER[..4], &[0xF0, 0x00, 0x21, 0x09]);
        assert_eq!(SYSEX_HEADER.len(), 10);
    }

    #[test]
    fn commands_round_trip_through_their_ids() {
        let all = [
            Command::SurfaceConfiguration,
            Command::Identity,
            Command::SetTempo,
            Command::TrackEnabled,
            Command::FocusFollow,
            Command::TrackSelected,
            Command::MuteButton,
            Command::SoloButton,
            Command::TrackArmed,
            Command::Volume,
            Command::Pan,
            Command::TrackName,
            Command::VuMeter,
            Command::TrackColor,
            Command::SelectedTrackSelectPlugin,
            Command::SelectedTrackPluginChainInfo,
            Command::SelectedTrackParamName,
            Command::SelectedTrackParamDisplayValue,
            Command::SelectedTrackParamPageNum,
            Command::AdjustParameterValue,
        ];
        assert_eq!(all.len(), 20, "NI's table has twenty entries");
        for c in all {
            assert_eq!(Command::from_id(c.id()), Some(c));
        }
    }

    #[test]
    fn unknown_command_ids_are_rejected_not_guessed() {
        // 0x4A is conspicuously absent from NI's table.
        assert_eq!(Command::from_id(0x4A), None);
        assert_eq!(Command::from_id(0x00), None);
        assert_eq!(Command::from_id(0xFF), None);
    }

    #[test]
    fn encode_produces_the_documented_frame() {
        let mut out = [0u8; 32];
        let n = encode(&mut out, Command::TrackEnabled, 1, 3, &[]).unwrap();
        assert_eq!(
            &out[..n],
            &[
                0xF0, 0x00, 0x21, 0x09, 0x00, 0x00, 0x44, 0x43, 0x01, 0x00, // header
                0x40, // track_enabled
                0x01, // value
                0x03, // track
                0xF7,
            ]
        );
    }

    #[test]
    fn text_payloads_have_no_length_prefix_and_end_at_f7() {
        let mut out = [0u8; 64];
        let n = encode_text(&mut out, Command::TrackName, 0, 2, "Bass").unwrap();
        assert_eq!(&out[10..13], &[0x48, 0x00, 0x02]);
        assert_eq!(&out[13..17], b"Bass", "raw bytes, no length byte");
        assert_eq!(out[n - 1], SYSEX_END);
        assert_eq!(n, 10 + 3 + 4 + 1);
    }

    #[test]
    fn non_ascii_text_is_rejected_rather_than_mangled() {
        // A code unit >= 0x80 inside SysEx would be read as a status byte and
        // corrupt the stream. NI's own script does not guard this.
        let mut out = [0u8; 64];
        let err = encode_text(&mut out, Command::TrackName, 0, 0, "Café").unwrap_err();
        assert!(matches!(err, MidiError::NonAsciiText { ch: 'é', .. }));
        // Plain ASCII is fine.
        assert!(encode_text(&mut out, Command::TrackName, 0, 0, "Cafe").is_ok());
    }

    #[test]
    fn a_payload_byte_with_the_status_bit_set_is_rejected() {
        let mut out = [0u8; 64];
        assert!(matches!(
            encode(&mut out, Command::TrackName, 0, 0, &[0x41, 0x80]),
            Err(MidiError::NotADataByte {
                byte: 0x80,
                index: 1
            })
        ));
    }

    #[test]
    fn out_of_range_value_or_track_is_rejected() {
        let mut out = [0u8; 64];
        assert!(encode(&mut out, Command::Volume, 0x80, 0, &[]).is_err());
        assert!(encode(&mut out, Command::Volume, 0, 0x80, &[]).is_err());
    }

    #[test]
    fn every_encoded_byte_respects_midi_framing() {
        // Exactly one leading F0, exactly one trailing F7, and no data byte in
        // between with the status bit set.
        let mut out = [0u8; 64];
        let n = encode_text(&mut out, Command::SelectedTrackParamName, 2, 5, "Cutoff").unwrap();
        let frame = &out[..n];
        assert_eq!(frame[0], 0xF0);
        assert_eq!(frame[n - 1], 0xF7);
        assert!(
            frame[1..n - 1].iter().all(|&b| b <= MAX_DATA_BYTE),
            "no interior byte may set the status bit"
        );
    }

    #[test]
    fn a_short_output_buffer_is_rejected() {
        let mut out = [0u8; 8];
        assert!(matches!(
            encode(&mut out, Command::Identity, 0, 0, &[]),
            Err(MidiError::OutputTooSmall { .. })
        ));
    }

    #[test]
    fn handshake_matches_the_recovered_sequence() {
        assert_eq!(handshake::HELLO, [0xBF, 0x01, 0x04]);
        assert_eq!(handshake::ENABLE_14BIT_PARAMS, [0xBF, 0x06, 0x01]);
        assert_eq!(handshake::GOODBYE, [0xBF, 0x02, 0x00]);
        // All three are channel-16 control changes.
        for m in [
            handshake::HELLO,
            handshake::ENABLE_14BIT_PARAMS,
            handshake::GOODBYE,
        ] {
            assert_eq!(m[0], STATUS_CC);
            assert!(m[1] <= MAX_DATA_BYTE && m[2] <= MAX_DATA_BYTE);
        }
    }

    #[test]
    fn relative_encoders_decode_both_directions() {
        // The failure mode this guards: reading a decrement as +127.
        assert_eq!(decode_relative(0x01), 1);
        assert_eq!(decode_relative(0x3F), 63);
        assert_eq!(decode_relative(0x7F), -1);
        assert_eq!(decode_relative(0x41), -63);
        assert_eq!(decode_relative(0x40), -64);
        assert_eq!(decode_relative(0x00), 0);
    }

    #[test]
    fn relative_encoding_round_trips_across_the_representable_range() {
        for delta in -64i8..=63 {
            let encoded = encode_relative(delta).expect("representable");
            assert!(encoded <= MAX_DATA_BYTE, "must be a data byte");
            assert_eq!(decode_relative(encoded), delta, "round trip for {delta}");
        }
    }

    #[test]
    fn unrepresentable_deltas_are_rejected_not_wrapped() {
        assert_eq!(encode_relative(64), None);
        assert_eq!(encode_relative(127), None);
        assert_eq!(encode_relative(-65), None);
        assert_eq!(encode_relative(i8::MIN), None);
    }

    #[test]
    fn per_channel_cc_refuses_an_out_of_bank_index() {
        assert_eq!(cc::per_channel(cc::VOLUME_DISPLAY_BASE, 0), Some(0x50));
        assert_eq!(cc::per_channel(cc::VOLUME_DISPLAY_BASE, 7), Some(0x57));
        assert_eq!(cc::per_channel(cc::VOLUME_DISPLAY_BASE, 8), None);
        assert_eq!(cc::per_channel(cc::PAN_DISPLAY_BASE, 7), Some(0x5F));
        assert_eq!(cc::per_channel(cc::PARAM_VALUE_BASE, 7), Some(0x77));
    }

    #[test]
    fn per_channel_ranges_do_not_collide() {
        // Volume 0x50..0x57, pan 0x58..0x5F, params 0x70..0x77.
        let mut seen = std::collections::HashSet::new();
        for base in [
            cc::VOLUME_DISPLAY_BASE,
            cc::PAN_DISPLAY_BASE,
            cc::PARAM_VALUE_BASE,
        ] {
            for i in 0..cc::CHANNELS {
                let n = cc::per_channel(base, i).unwrap();
                assert!(seen.insert(n), "CC 0x{n:02X} assigned twice");
                assert!(n <= MAX_DATA_BYTE);
            }
        }
    }

    #[test]
    fn cc_and_sysex_numbers_live_in_separate_namespaces() {
        // 0x42/0x43/0x44 are both CC numbers and SysEx command ids. They are
        // unrelated, and conflating them would be an easy and nasty bug.
        assert_eq!(cc::SELECT_RAW, 0x42);
        assert_eq!(Command::TrackSelected.id(), 0x42);
        assert_eq!(cc::MUTE_RAW, 0x43);
        assert_eq!(Command::MuteButton.id(), 0x43);
    }
}
