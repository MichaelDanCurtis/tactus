//! Real MIDI I/O: resolving, opening, sending and receiving ports.
//!
//! All the *judgement* about which port is the right one lives in
//! [`tactus_midi::port`] — this module's job is to turn what `midir` reports
//! into [`PortIdentity`] values and hand them to
//! [`tactus_midi::port::find_port`]. The matching rules encode two
//! hardware-confirmed traps and must not be reimplemented here:
//!
//! 1. the name worth matching is *device name + entity name*
//!    (`KONTROL S88 MK3 DAW`), never the bare entity name (`DAW`), which would
//!    match an unrelated interface;
//! 2. two matches are an error, not a coin flip.
//!
//! # What `midir` does and does not tell us
//!
//! On macOS `midir` reports one string per endpoint: CoreMIDI's *display name*,
//! which is already the `device entity` concatenation trap 1 is about. It
//! reports **no manufacturer** and **no offline flag**, so:
//!
//! - [`PortIdentity::manufacturer`] is `None`, and `is_native_instruments()`
//!   falls back to the device name — which is why that fallback exists;
//! - [`PortIdentity::online`] is `true` for every enumerated port, because the
//!   platform's endpoint list is all we can see. The consequence is benign in
//!   the direction that matters: a lingering stale device makes resolution fail
//!   with [`tactus_midi::MidiError::AmbiguousPort`] rather than silently
//!   selecting a port that is not there.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use midir::{Ignore, MidiIO, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use tactus_midi::daw::{self, Command};
use tactus_midi::port::{PortIdentity, PortRole, find_port};

use crate::IoError;

/// Client name this crate registers with the platform MIDI server.
pub const CLIENT_NAME: &str = "tactus";

/// Device-name filter that matches any Kontrol MK3, including NI's simulator.
///
/// Passed to [`find_port`] as the `device_filter`. `"KONTROL"` rather than a
/// full model name because the device string differs per model, and the
/// simulator calls itself `Kontrol MK3 Simulator`.
pub const KONTROL_FILTER: &str = "KONTROL";

/// Parse a platform port name into a [`PortIdentity`].
///
/// CoreMIDI display names are `"<device name> <entity name>"`, so the entity is
/// the trailing word — but **only** when that word is a role this SDK knows.
/// Anything else yields an identity with an empty entity name, which
/// deliberately matches no role: a port literally called `DAW` with no device
/// prefix is the exact ambiguity trap 1 warns about, and silently accepting it
/// would let an unrelated interface answer for the keyboard.
pub fn parse_port_name(display_name: &str) -> PortIdentity {
    let name = display_name.trim();
    if let Some((device, entity)) = name.rsplit_once(' ') {
        let device = device.trim();
        if !device.is_empty() && PortRole::from_entity_name(entity).is_some() {
            return PortIdentity {
                device_name: device.to_owned(),
                entity_name: entity.to_owned(),
                manufacturer: None,
                online: true,
            };
        }
    }
    PortIdentity {
        device_name: name.to_owned(),
        entity_name: String::new(),
        manufacturer: None,
        online: true,
    }
}

fn identities<T: MidiIO>(io: &T, direction: &'static str) -> Result<Vec<PortIdentity>, IoError> {
    io.ports()
        .iter()
        .enumerate()
        .map(|(index, port)| {
            io.port_name(port)
                .map(|name| parse_port_name(&name))
                .map_err(|source| IoError::MidiPortInfo {
                    direction,
                    index,
                    source,
                })
        })
        .collect()
}

/// Enumerate MIDI **input** ports as [`PortIdentity`] values.
pub fn list_input_ports() -> Result<Vec<PortIdentity>, IoError> {
    let input = MidiInput::new(CLIENT_NAME).map_err(|source| IoError::MidiInit { source })?;
    identities(&input, "input")
}

/// Enumerate MIDI **output** ports as [`PortIdentity`] values.
pub fn list_output_ports() -> Result<Vec<PortIdentity>, IoError> {
    let output = MidiOutput::new(CLIENT_NAME).map_err(|source| IoError::MidiInit { source })?;
    identities(&output, "output")
}

/// An open MIDI output port on an MK3 keyboard.
pub struct MidiOut {
    connection: MidiOutputConnection,
    identity: PortIdentity,
}

impl std::fmt::Debug for MidiOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiOut")
            .field("port", &self.identity.display_name())
            .finish()
    }
}

impl MidiOut {
    /// Resolve and open the output port playing `role` on a device whose name
    /// contains `device_filter` (see [`KONTROL_FILTER`]).
    pub fn open(role: PortRole, device_filter: &str) -> Result<Self, IoError> {
        let output = MidiOutput::new(CLIENT_NAME).map_err(|source| IoError::MidiInit { source })?;
        let candidates = identities(&output, "output")?;
        let index = find_port(&candidates, role, device_filter)?;
        let ports = output.ports();
        let port = ports.get(index).ok_or(IoError::Internal {
            what: "find_port returned an index outside the port list",
        })?;
        let identity = candidates.get(index).cloned().ok_or(IoError::Internal {
            what: "find_port returned an index outside the identity list",
        })?;
        let name = identity.display_name();
        let connection = output
            .connect(port, &name)
            .map_err(|e| IoError::MidiConnect {
                name,
                kind: e.kind(),
            })?;
        Ok(Self {
            connection,
            identity,
        })
    }

    /// The port this is connected to.
    pub fn identity(&self) -> &PortIdentity {
        &self.identity
    }

    /// Send raw MIDI bytes.
    ///
    /// No framing is added and nothing is validated here: callers that want a
    /// DAW-port command should use [`MidiOut::send_command`] or
    /// [`MidiOut::send_text`], which encode through `tactus-midi`.
    pub fn send_raw(&mut self, bytes: &[u8]) -> Result<(), IoError> {
        self.connection
            .send(bytes)
            .map_err(|source| IoError::MidiSend {
                len: bytes.len(),
                source,
            })
    }

    /// Encode and send a DAW-port SysEx command.
    ///
    /// Framing and data-byte validation are
    /// [`tactus_midi::daw::encode`]'s; this only moves the result.
    pub fn send_command(
        &mut self,
        command: Command,
        value: u8,
        track: u8,
        payload: &[u8],
    ) -> Result<(), IoError> {
        let mut buf = vec![0u8; daw::SYSEX_HEADER.len() + 3 + payload.len() + 1];
        let written = daw::encode(&mut buf, command, value, track, payload)?;
        let message = buf.get(..written).ok_or(IoError::Internal {
            what: "encoded SysEx exceeds the buffer sized for it",
        })?;
        self.connection
            .send(message)
            .map_err(|source| IoError::MidiSend {
                len: written,
                source,
            })
    }

    /// Encode and send a DAW-port SysEx command with a text payload.
    ///
    /// Non-ASCII text is rejected by [`tactus_midi::daw::encode_text`] rather
    /// than mangled.
    pub fn send_text(
        &mut self,
        command: Command,
        value: u8,
        track: u8,
        text: &str,
    ) -> Result<(), IoError> {
        let mut buf = vec![0u8; daw::SYSEX_HEADER.len() + 3 + text.len() + 1];
        let written = daw::encode_text(&mut buf, command, value, track, text)?;
        let message = buf.get(..written).ok_or(IoError::Internal {
            what: "encoded SysEx text exceeds the buffer sized for it",
        })?;
        self.connection
            .send(message)
            .map_err(|source| IoError::MidiSend {
                len: written,
                source,
            })
    }
}

/// An open MIDI input port on an MK3 keyboard.
///
/// Incoming messages are queued as they arrive; read them with
/// [`MidiIn::try_recv`] or [`MidiIn::recv_timeout`]. Dropping this closes the
/// port.
pub struct MidiIn {
    // Held to keep the callback alive; the queue is the public surface.
    _connection: MidiInputConnection<()>,
    messages: Receiver<Vec<u8>>,
    identity: PortIdentity,
}

impl std::fmt::Debug for MidiIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiIn")
            .field("port", &self.identity.display_name())
            .finish()
    }
}

impl MidiIn {
    /// Resolve and open the input port playing `role` on a device whose name
    /// contains `device_filter`.
    pub fn open(role: PortRole, device_filter: &str) -> Result<Self, IoError> {
        let mut input =
            MidiInput::new(CLIENT_NAME).map_err(|source| IoError::MidiInit { source })?;
        // SysEx *is* the DAW protocol, and midir ignores it by default. Keeping
        // that default would make the DAW port look silent.
        input.ignore(Ignore::None);

        let candidates = identities(&input, "input")?;
        let index = find_port(&candidates, role, device_filter)?;
        let ports = input.ports();
        let port = ports.get(index).ok_or(IoError::Internal {
            what: "find_port returned an index outside the port list",
        })?;
        let identity = candidates.get(index).cloned().ok_or(IoError::Internal {
            what: "find_port returned an index outside the identity list",
        })?;
        let name = identity.display_name();

        let (sender, messages) = channel();
        let connection = input
            .connect(
                port,
                &name,
                move |_timestamp, bytes, _| {
                    // The receiver lives in the struct that owns this
                    // connection, so a send only fails during teardown.
                    let _ = sender.send(bytes.to_vec());
                },
                (),
            )
            .map_err(|e| IoError::MidiConnect {
                name,
                kind: e.kind(),
            })?;

        Ok(Self {
            _connection: connection,
            messages,
            identity,
        })
    }

    /// The port this is connected to.
    pub fn identity(&self) -> &PortIdentity {
        &self.identity
    }

    /// Take the next queued message, if one is waiting.
    pub fn try_recv(&self) -> Option<Vec<u8>> {
        self.messages.try_recv().ok()
    }

    /// Wait up to `timeout` for the next message.
    ///
    /// `None` means nothing arrived in time. It cannot mean "port closed": the
    /// connection is owned by this value, so the queue outlives every sender
    /// only when `self` is being dropped.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        self.messages.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use tactus_midi::MidiError;

    #[test]
    fn coremidi_display_names_split_into_device_and_entity() {
        // The exact strings observed on hardware through CoreMIDI.
        for (name, role) in [
            ("KONTROL S88 MK3 Main", PortRole::Main),
            ("KONTROL S88 MK3 DAW", PortRole::Daw),
            ("KONTROL S88 MK3 Ext", PortRole::Ext),
        ] {
            let id = parse_port_name(name);
            assert_eq!(id.device_name, "KONTROL S88 MK3", "{name}");
            assert_eq!(id.role(), Some(role), "{name}");
            assert!(id.is_native_instruments(), "{name}");
            // Round-trips to the string NI's own script matches.
            assert_eq!(id.display_name(), name, "{name}");
        }
    }

    #[test]
    fn a_bare_entity_name_matches_nothing() {
        // Trap 1: querying an endpoint's own name property yields "DAW", which
        // would happily match an unrelated interface. It must not resolve.
        for name in ["DAW", "Main", "Ext", " DAW "] {
            let id = parse_port_name(name);
            assert_eq!(id.role(), None, "{name:?} must not claim a role");
        }
        let err = find_port(&[parse_port_name("DAW")], PortRole::Daw, KONTROL_FILTER).unwrap_err();
        assert!(matches!(err, MidiError::PortNotFound { .. }), "{err:?}");
    }

    #[test]
    fn unrelated_ports_are_not_mistaken_for_the_keyboard() {
        for name in [
            "IAC Driver Bus 1",
            "Komplete Audio 6 MK2",
            "Launchpad Pro Standalone Port",
            "",
        ] {
            let id = parse_port_name(name);
            assert_eq!(id.role(), None, "{name:?}");
        }
    }

    #[test]
    fn the_simulator_resolves_like_the_hardware() {
        // NI's script accepts "Kontrol MK3 Simulator DAW"; with no
        // manufacturer from midir, the KONTROL fallback has to carry it.
        let id = parse_port_name("Kontrol MK3 Simulator DAW");
        assert_eq!(id.device_name, "Kontrol MK3 Simulator");
        assert_eq!(id.role(), Some(PortRole::Daw));
        assert!(id.is_native_instruments());
        assert_eq!(
            find_port(&[id], PortRole::Daw, KONTROL_FILTER),
            Ok(0),
            "matches the KONTROL filter case-insensitively"
        );
    }

    #[test]
    fn resolution_picks_the_daw_port_out_of_a_realistic_port_list() {
        // Shape of a real macOS enumeration: fifteen ports, three of them the
        // keyboard's.
        let names = [
            "IAC Driver Bus 1",
            "Komplete Audio 6 MK2",
            "KONTROL S88 MK3 Main",
            "KONTROL S88 MK3 DAW",
            "KONTROL S88 MK3 Ext",
            "Stream Deck Plus",
        ];
        let ports: Vec<PortIdentity> = names.iter().map(|n| parse_port_name(n)).collect();
        assert_eq!(find_port(&ports, PortRole::Daw, KONTROL_FILTER), Ok(3));
        assert_eq!(find_port(&ports, PortRole::Main, KONTROL_FILTER), Ok(2));
        assert_eq!(find_port(&ports, PortRole::Ext, KONTROL_FILTER), Ok(4));
    }

    #[test]
    fn two_identical_keyboards_are_an_error_not_a_guess() {
        // Trap 2, as it reaches us through midir: we cannot see an offline
        // flag, so a duplicate must fail loudly rather than pick one.
        let ports: Vec<PortIdentity> = ["KONTROL S88 MK3 DAW", "KONTROL S88 MK3 DAW"]
            .iter()
            .map(|n| parse_port_name(n))
            .collect();
        let err = find_port(&ports, PortRole::Daw, KONTROL_FILTER).unwrap_err();
        match err {
            MidiError::AmbiguousPort { count, .. } => assert_eq!(count, 2),
            other => panic!("expected AmbiguousPort, got {other:?}"),
        }
        // And it surfaces through IoError with the advice intact.
        let wrapped: IoError = err.into();
        assert!(wrapped.to_string().contains("disambiguate"), "{wrapped}");
    }

    #[test]
    fn midir_cannot_report_manufacturer_or_offline_state() {
        // Pinning the honest-but-limited mapping: if a future midir exposes
        // these, this test is where the change should be noticed.
        let id = parse_port_name("KONTROL S88 MK3 DAW");
        assert_eq!(id.manufacturer, None);
        assert!(id.online);
    }

    #[test]
    fn entity_matching_is_case_insensitive_but_device_prefixed() {
        let id = parse_port_name("KONTROL S61 MK3 daw");
        assert_eq!(id.role(), Some(PortRole::Daw));
        assert_eq!(id.device_name, "KONTROL S61 MK3");
    }
}
