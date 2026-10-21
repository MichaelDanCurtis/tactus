//! MIDI protocol support for the Kontrol S-series MK3.
//!
//! **Transport-agnostic by design.** This crate encodes and decodes bytes and
//! resolves port identities; it opens no ports and links no platform MIDI
//! framework. That keeps the protocol testable without hardware and keeps the
//! SDK portable — a Linux or Windows host uses the same logic with a different
//! I/O layer.
//!
//! Two independent surfaces live here:
//!
//! - [`daw`] — the DAW-port protocol: SysEx commands, handshake, control
//!   changes. Recovered from NI's own MIDI Remote script. **Vendor source, not
//!   hardware-verified.**
//! - [`port`] — port discovery and matching. **Hardware-confirmed**, and it
//!   encodes two traps that bite naive implementations.
//!
//! Note the evidence asymmetry against the HID layer: there, the device
//! declares structure but no semantics, so controls are exposed by index. Here,
//! NI's source supplies semantics — real names like "play" and "metronome" —
//! but nothing has confirmed them against hardware. Different weaknesses,
//! documented rather than averaged away.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod daw;
pub mod port;

pub use daw::Command;
pub use port::{PortIdentity, PortRole};

use thiserror::Error;

/// Errors from MIDI encoding or port resolution.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MidiError {
    /// A byte intended as MIDI data had its status bit set.
    ///
    /// Any value `>= 0x80` inside a message would be read as a new status
    /// byte, corrupting the remainder of the stream.
    #[error("byte 0x{byte:02X} at index {index} is not a valid MIDI data byte (must be <= 0x7F)")]
    NotADataByte {
        /// The offending byte.
        byte: u8,
        /// Its position in the payload.
        index: usize,
    },

    /// Text contained a character outside 7-bit ASCII.
    ///
    /// The device's text payloads are one byte per character with no length
    /// prefix, so anything above `U+007F` cannot be represented without
    /// corrupting the SysEx frame.
    #[error("text contains non-ASCII character {ch:?} at byte {index}; transliterate first")]
    NonAsciiText {
        /// The offending character.
        ch: char,
        /// Byte offset within the string.
        index: usize,
    },

    /// The output buffer was too small for the encoded message.
    #[error("output buffer holds {got} bytes, need {need}")]
    OutputTooSmall {
        /// Bytes available.
        got: usize,
        /// Bytes required.
        need: usize,
    },

    /// No port matched the requested role.
    #[error("no {role:?} port found among {candidates} candidates")]
    PortNotFound {
        /// The role requested.
        role: PortRole,
        /// How many candidates were considered.
        candidates: usize,
    },

    /// More than one online port matched, so the choice is ambiguous.
    ///
    /// Reported rather than resolved arbitrarily: with two identical keyboards
    /// attached, picking one silently would send a user's output to the wrong
    /// instrument.
    #[error("{count} online ports matched role {role:?}; disambiguate by serial")]
    AmbiguousPort {
        /// The role requested.
        role: PortRole,
        /// How many matched.
        count: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_the_offending_value() {
        // Diagnostics a user can act on, not just a failure flag.
        let e = MidiError::NotADataByte {
            byte: 0x80,
            index: 3,
        };
        let s = e.to_string();
        assert!(s.contains("0x80"), "message names the byte: {s}");
        assert!(s.contains('3'), "message names the index: {s}");

        let e = MidiError::NonAsciiText { ch: 'é', index: 3 };
        assert!(e.to_string().contains("transliterate"), "says what to do");
    }
}
