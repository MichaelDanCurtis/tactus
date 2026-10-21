//! The single error type every I/O operation returns.
//!
//! Every variant names the thing that went wrong **and** the value that caused
//! it, because an I/O layer that reports only "it failed" forces the caller to
//! guess between a missing device, a busy device and a programming error —
//! three problems with three different fixes.

use thiserror::Error;

use tactus_core::CoreError;
use tactus_hid::{HidError as HidProtocolError, Model};
use tactus_midi::MidiError;

use crate::hid::{DECLARED_FEATURE_REPORTS, FEATURE_BUFFER_LEN};

/// Anything that can go wrong talking to real hardware.
///
/// `#[non_exhaustive]` because new platforms will need new variants and adding
/// one should not be a breaking change for callers that match on this.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IoError {
    /// The HID backend could not be initialised at all.
    #[error("could not initialise the HID backend: {source}")]
    HidInit {
        /// Underlying backend error.
        #[source]
        source: hidapi::HidError,
    },

    /// No Kontrol S-series MK3 keyboard is attached.
    #[error(
        "no Kontrol S-series MK3 keyboard found: nothing with USB vendor 0x{vendor:04X} \
         and a known MK3 product id among {examined} HID devices"
    )]
    NoDevice {
        /// Vendor id searched for.
        vendor: u16,
        /// HID devices examined.
        examined: usize,
    },

    /// Keyboards are attached, but none of the requested model.
    #[error("no {model:?} attached; found {found} other Kontrol MK3 keyboard(s)")]
    ModelNotFound {
        /// Model requested.
        model: Model,
        /// How many other MK3 keyboards were present.
        found: usize,
    },

    /// Several keyboards matched, so the choice is the caller's to make.
    ///
    /// Reported rather than resolved: picking one of two attached keyboards
    /// silently would light the wrong instrument's keys, and the caller has
    /// information (which one the user selected) that this layer does not.
    #[error(
        "{count} attached keyboards match{}; open one explicitly with Device::open(&info) \
         — candidates: {candidates}",
        match model { Some(m) => format!(" model {m:?}"), None => String::new() }
    )]
    AmbiguousDevice {
        /// The model filter in force, if any.
        model: Option<Model>,
        /// How many matched.
        count: usize,
        /// Human-readable list of the candidates, for the caller to choose from.
        candidates: String,
    },

    /// The device is present but could not be opened.
    ///
    /// On macOS the overwhelmingly common cause is another process already
    /// holding it. `tactus` opens non-exclusively and will not seize a device
    /// from another process, so the remedy is to quit the other one.
    #[error(
        "could not open {model:?} at {path}: {source}. Another process is probably holding \
         the device — Komplete Kontrol, the NI Hardware/Host Integration Agent, or another \
         tactus process. Quit it and retry; tactus will not seize the device from another \
         process. (Feature reports often still read while another process holds it, so \
         read_feature may work where open fails.)"
    )]
    Open {
        /// Model that failed to open.
        model: Model,
        /// HID path, as the platform reports it.
        path: String,
        /// Underlying backend error.
        #[source]
        source: hidapi::HidError,
    },

    /// A HID transfer failed.
    #[error("HID {op} failed: {source}")]
    Hid {
        /// Which operation.
        op: &'static str,
        /// Underlying backend error.
        #[source]
        source: hidapi::HidError,
    },

    /// A feature report id this firmware does not implement was requested.
    ///
    /// Notably `0xF8`, which the vendor's own report-map table claims and this
    /// firmware does not have — see `docs/protocol/mk3-hid.md`.
    #[error(
        "feature report 0x{id:02X} is not declared by this firmware; it declares only \
         {declared}"
    )]
    UndeclaredFeatureReport {
        /// Report id requested.
        id: u8,
        /// The ids that do exist.
        declared: String,
    },

    /// A feature-report write was refused by this crate.
    #[error(
        "refusing to write feature report 0x{id:02X}: it is settable but its semantics are \
         unknown, and speculative configuration must not reach the hardware"
    )]
    FeatureWriteRefused {
        /// Report id the caller tried to write.
        id: u8,
    },

    /// The caller's buffer cannot hold a feature report.
    #[error(
        "buffer for feature report 0x{id:02X} holds {got} bytes; need {need} \
         (1 report id + {payload} payload bytes)"
    )]
    FeatureBufferTooSmall {
        /// Report id requested.
        id: u8,
        /// Bytes required.
        need: usize,
        /// Bytes supplied.
        got: usize,
        /// Payload bytes the firmware declares.
        payload: usize,
    },

    /// A feature report came back shorter than its own report id.
    #[error("feature report 0x{id:02X} returned only {got} bytes")]
    FeatureReportTooShort {
        /// Report id requested.
        id: u8,
        /// Bytes returned.
        got: usize,
    },

    /// A feature report answered with a different report id than requested.
    #[error("requested feature report 0x{id:02X} but the device answered with 0x{got:02X}")]
    FeatureReportIdMismatch {
        /// Report id requested.
        id: u8,
        /// Report id received.
        got: u8,
    },

    /// The serial feature report held a byte that is not printable ASCII.
    ///
    /// `0x00` and `0xFF` are accepted as padding; anything else is reported
    /// rather than silently dropped, because a mangled serial that looks
    /// plausible is worse than no serial.
    #[error(
        "serial from feature report 0x{id:02X} holds byte 0x{byte:02X} at index {index}, \
         which is not printable ASCII"
    )]
    SerialNotAscii {
        /// Report id read.
        id: u8,
        /// The offending byte.
        byte: u8,
        /// Its index within the payload.
        index: usize,
    },

    /// The serial feature report was empty or all padding.
    #[error("feature report 0x{id:02X} carried no serial characters")]
    SerialEmpty {
        /// Report id read.
        id: u8,
    },

    /// A write reached the device only partially.
    #[error("wrote {sent} of {expected} bytes of report 0x{id:02X}")]
    ShortWrite {
        /// Report id written.
        id: u8,
        /// Bytes the platform accepted.
        sent: usize,
        /// Bytes offered.
        expected: usize,
    },

    /// The MIDI backend could not be initialised.
    #[error("could not initialise the MIDI backend: {source}")]
    MidiInit {
        /// Underlying backend error.
        #[source]
        source: midir::InitError,
    },

    /// A MIDI port's name could not be read, so it cannot be identified.
    #[error("could not read the name of {direction} MIDI port {index}: {source}")]
    MidiPortInfo {
        /// `"input"` or `"output"`.
        direction: &'static str,
        /// Index within the platform's port list.
        index: usize,
        /// Underlying backend error.
        #[source]
        source: midir::PortInfoError,
    },

    /// A MIDI port was resolved but could not be connected.
    #[error("could not connect to MIDI port {name:?}: {kind:?}")]
    MidiConnect {
        /// Display name of the port.
        name: String,
        /// What the backend reported.
        ///
        /// `midir`'s `ConnectError<T>` carries the unopened port back to the
        /// caller, which would make this error type generic; only the kind is
        /// kept.
        kind: midir::ConnectErrorKind,
    },

    /// Sending MIDI bytes failed.
    #[error("could not send {len} bytes of MIDI: {source}")]
    MidiSend {
        /// Bytes offered.
        len: usize,
        /// Underlying backend error.
        #[source]
        source: midir::SendError,
    },

    /// A protocol crate rejected the data — a caller bug, surfaced verbatim.
    #[error("HID protocol: {0}")]
    Protocol(#[from] HidProtocolError),

    /// MIDI protocol encoding or port resolution failed.
    #[error("MIDI protocol: {0}")]
    Midi(#[from] MidiError),

    /// A device-neutral core operation failed.
    ///
    /// Nothing in this crate produces this today — there is no geometry on the
    /// HID or MIDI path. It is here so that a caller whose own function returns
    /// [`IoError`] can `?` a `tactus-core` operation (building a [`Surface`],
    /// clipping a [`Rect`]) alongside its I/O, instead of needing a second
    /// error type for the layer above.
    ///
    /// [`Surface`]: tactus_core::Surface
    /// [`Rect`]: tactus_core::Rect
    #[error("core: {0}")]
    Core(#[from] CoreError),

    /// A buffer this crate sized itself turned out not to fit.
    ///
    /// Unreachable by construction; it exists so that the impossible case is
    /// an error rather than a panic.
    #[error("internal invariant violated: {what}")]
    Internal {
        /// What went wrong.
        what: &'static str,
    },
}

impl IoError {
    /// Build an [`IoError::UndeclaredFeatureReport`] with the declared set
    /// filled in, so the message tells the caller what *is* available.
    pub(crate) fn undeclared_feature(id: u8) -> Self {
        let declared = DECLARED_FEATURE_REPORTS
            .iter()
            .map(|d| format!("0x{d:02X}"))
            .collect::<Vec<_>>()
            .join(", ");
        Self::UndeclaredFeatureReport { id, declared }
    }

    /// Build an [`IoError::FeatureBufferTooSmall`] for `id`.
    pub(crate) fn feature_buffer_too_small(id: u8, got: usize) -> Self {
        Self::FeatureBufferTooSmall {
            id,
            need: FEATURE_BUFFER_LEN,
            got,
            payload: FEATURE_BUFFER_LEN.saturating_sub(1),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used)]

    use super::*;
    use tactus_hid::report_id;

    #[test]
    fn open_error_names_the_likely_culprit() {
        // The whole point of this variant: a user whose Komplete Kontrol is
        // running should be told that, not handed "operation failed".
        let e = IoError::Open {
            model: Model::S88,
            path: "DevSrvsID:4294970000".to_owned(),
            source: hidapi::HidError::HidApiError {
                message: "hid_open_path failed".to_owned(),
            },
        };
        let s = e.to_string();
        assert!(s.contains("Komplete Kontrol"), "names the usual cause: {s}");
        assert!(s.contains("will not seize"), "states the policy: {s}");
        assert!(s.contains("DevSrvsID:4294970000"), "names the device: {s}");
    }

    #[test]
    fn undeclared_feature_error_lists_the_real_ids() {
        // 0xF8 is the vendor table's phantom report; the message must steer a
        // caller who trusted that table towards the three that exist.
        let s = IoError::undeclared_feature(0xF8).to_string();
        assert!(s.contains("0xF8"), "names the request: {s}");
        assert!(s.contains("0xD0"), "lists 0xD0: {s}");
        assert!(s.contains("0xD8"), "lists 0xD8: {s}");
        assert!(s.contains("0xD9"), "lists 0xD9: {s}");
    }

    #[test]
    fn feature_write_refusal_explains_itself() {
        let s = IoError::FeatureWriteRefused {
            id: report_id::FEAT_CONFIG,
        }
        .to_string();
        assert!(s.contains("0xD0"), "names the report: {s}");
        assert!(s.contains("unknown"), "says why: {s}");
    }

    #[test]
    fn ambiguity_error_lists_candidates_and_the_way_out() {
        let e = IoError::AmbiguousDevice {
            model: Some(Model::S88),
            count: 2,
            candidates: "S88 (serial A), S88 (serial B)".to_owned(),
        };
        let s = e.to_string();
        assert!(s.contains("serial A"), "lists candidates: {s}");
        assert!(s.contains("Device::open"), "says how to disambiguate: {s}");

        let anonymous = IoError::AmbiguousDevice {
            model: None,
            count: 3,
            candidates: "x, y, z".to_owned(),
        };
        assert!(
            !anonymous.to_string().contains("model"),
            "no model filter, no model noise: {anonymous}"
        );
    }

    #[test]
    fn protocol_errors_pass_through_verbatim() {
        // A caller bug in light-guide values must stay legible after crossing
        // the I/O boundary, not become "write failed".
        let inner = HidProtocolError::LightGuideOutOfRange {
            value: 200,
            index: 7,
        };
        let wrapped: IoError = inner.into();
        let s = wrapped.to_string();
        assert!(s.contains("200"), "keeps the value: {s}");
        assert!(s.contains("0..=127"), "keeps the declared range: {s}");
    }

    #[test]
    fn buffer_error_states_the_required_size() {
        let s = IoError::feature_buffer_too_small(report_id::FEAT_SERIAL, 32).to_string();
        assert!(s.contains("33"), "names the needed size: {s}");
        assert!(s.contains("32"), "names the supplied size: {s}");
    }
}
