//! Port discovery and matching.
//!
//! **Evidence class: hardware observation.** Verified against a connected
//! S88 MK3 through CoreMIDI.
//!
//! # Two traps this module exists to avoid
//!
//! **1. The port name you want is a concatenation.** NI's own script matches
//! the literal string `KONTROL S88 MK3 DAW`. That string exists only as
//! *device name* + *entity name*. Querying an endpoint's own name property
//! returns the bare word `DAW` — which is ambiguous, and will happily match an
//! unrelated interface that also calls a port "DAW".
//!
//! Observed on hardware:
//!
//! ```text
//! device   "KONTROL S88 MK3"   manufacturer "Native Instruments"
//!   entity "Main"   -> "KONTROL S88 MK3 Main"
//!   entity "DAW"    -> "KONTROL S88 MK3 DAW"
//!   entity "Ext"    -> "KONTROL S88 MK3 Ext"
//! ```
//!
//! **2. Stale devices linger.** CoreMIDI retains offline devices. With one
//! keyboard physically attached, enumeration showed **two** devices named
//! `KONTROL S88 MK3` among fifteen, while only one had live endpoints. Matching
//! on name alone can therefore select a device that is not there.
//!
//! The conclusion both traps point to: match on **manufacturer plus device name
//! plus role, over online endpoints only**, and treat multiple matches as an
//! error rather than guessing.

use crate::MidiError;

/// Manufacturer string reported by the device.
pub const MANUFACTURER: &str = "Native Instruments";

/// The role a port plays, identified by its entity name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortRole {
    /// Keys, aftertouch, pedals, wheels — ordinary musical performance data.
    Main,
    /// Host integration: the SysEx command set and channel-16 control changes.
    Daw,
    /// External MIDI routed through the instrument.
    Ext,
}

impl PortRole {
    /// Every role.
    pub const ALL: [PortRole; 3] = [PortRole::Main, PortRole::Daw, PortRole::Ext];

    /// The entity name the device uses for this role.
    pub const fn entity_name(self) -> &'static str {
        match self {
            Self::Main => "Main",
            Self::Daw => "DAW",
            Self::Ext => "Ext",
        }
    }

    /// Identify a role from an entity name, case-insensitively.
    pub fn from_entity_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|r| r.entity_name().eq_ignore_ascii_case(name.trim()))
    }
}

/// What a platform MIDI layer must report about a candidate port.
///
/// Deliberately a plain data structure: the SDK does not care how these were
/// obtained, which keeps the matching logic portable and unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortIdentity {
    /// Device-level name, e.g. `KONTROL S88 MK3`.
    pub device_name: String,
    /// Entity/endpoint name, e.g. `DAW`.
    pub entity_name: String,
    /// Manufacturer, where the platform reports one.
    pub manufacturer: Option<String>,
    /// Whether the endpoint is currently live.
    ///
    /// Critical: CoreMIDI lists offline devices, so this must be populated
    /// honestly or trap 2 returns.
    pub online: bool,
}

impl PortIdentity {
    /// The combined name a user sees, and the string NI's own script matches.
    pub fn display_name(&self) -> String {
        format!("{} {}", self.device_name.trim(), self.entity_name.trim())
    }

    /// The role of this port, if it is one we recognise.
    pub fn role(&self) -> Option<PortRole> {
        PortRole::from_entity_name(&self.entity_name)
    }

    /// Whether this looks like a Native Instruments device.
    ///
    /// Accepts a missing manufacturer, since not every platform reports one;
    /// falls back to the device name in that case.
    pub fn is_native_instruments(&self) -> bool {
        match &self.manufacturer {
            Some(m) => m.eq_ignore_ascii_case(MANUFACTURER),
            None => self.device_name.to_ascii_uppercase().contains("KONTROL"),
        }
    }
}

/// Select the single online port matching `role` on a Native Instruments
/// device whose name contains `device_filter`.
///
/// Returns the index into `candidates`.
///
/// Ambiguity is an **error**, not a coin flip: with two identical keyboards
/// attached, silently choosing one would send a user's output to the wrong
/// instrument. Callers with multiple devices should filter by serial first.
pub fn find_port(
    candidates: &[PortIdentity],
    role: PortRole,
    device_filter: &str,
) -> Result<usize, MidiError> {
    let filter = device_filter.trim().to_ascii_uppercase();
    let matches: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.online
                && c.is_native_instruments()
                && c.role() == Some(role)
                && (filter.is_empty() || c.device_name.to_ascii_uppercase().contains(&filter))
        })
        .map(|(i, _)| i)
        .collect();

    match matches.len() {
        0 => Err(MidiError::PortNotFound {
            role,
            candidates: candidates.len(),
        }),
        1 => Ok(matches[0]),
        n => Err(MidiError::AmbiguousPort { role, count: n }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(dev: &str, ent: &str, online: bool) -> PortIdentity {
        PortIdentity {
            device_name: dev.into(),
            entity_name: ent.into(),
            manufacturer: Some(MANUFACTURER.into()),
            online,
        }
    }

    /// The hardware-observed topology: one device, three entities.
    fn observed() -> Vec<PortIdentity> {
        vec![
            id("KONTROL S88 MK3", "Main", true),
            id("KONTROL S88 MK3", "DAW", true),
            id("KONTROL S88 MK3", "Ext", true),
        ]
    }

    #[test]
    fn display_name_reproduces_the_string_ni_matches() {
        // This exact string is what NI's own script expects. It exists only as
        // device + entity, never as an endpoint's own name.
        assert_eq!(
            id("KONTROL S88 MK3", "DAW", true).display_name(),
            "KONTROL S88 MK3 DAW"
        );
    }

    #[test]
    fn roles_round_trip_and_tolerate_case() {
        for r in PortRole::ALL {
            assert_eq!(PortRole::from_entity_name(r.entity_name()), Some(r));
        }
        assert_eq!(PortRole::from_entity_name("daw"), Some(PortRole::Daw));
        assert_eq!(PortRole::from_entity_name(" DAW "), Some(PortRole::Daw));
        assert_eq!(PortRole::from_entity_name("Mixer"), None);
    }

    #[test]
    fn finds_the_daw_port_in_the_observed_topology() {
        let c = observed();
        let i = find_port(&c, PortRole::Daw, "KONTROL S88 MK3").unwrap();
        assert_eq!(c[i].display_name(), "KONTROL S88 MK3 DAW");
    }

    #[test]
    fn each_role_resolves_distinctly() {
        let c = observed();
        let mut picked = Vec::new();
        for r in PortRole::ALL {
            picked.push(find_port(&c, r, "").unwrap());
        }
        picked.sort_unstable();
        picked.dedup();
        assert_eq!(picked.len(), 3, "three roles, three distinct ports");
    }

    #[test]
    fn offline_duplicates_are_ignored() {
        // Trap 2, reproduced: CoreMIDI showed two devices of this name with
        // only one live. Matching on name alone would pick the stale one.
        let mut c = vec![id("KONTROL S88 MK3", "DAW", false)];
        c.extend(observed());
        let i = find_port(&c, PortRole::Daw, "KONTROL S88 MK3").unwrap();
        assert!(c[i].online, "must select the live endpoint");
        assert_ne!(i, 0);
    }

    #[test]
    fn an_all_offline_device_is_not_found() {
        let c = vec![
            id("KONTROL S88 MK3", "Main", false),
            id("KONTROL S88 MK3", "DAW", false),
        ];
        assert!(matches!(
            find_port(&c, PortRole::Daw, ""),
            Err(MidiError::PortNotFound { .. })
        ));
    }

    #[test]
    fn a_foreign_port_named_daw_is_not_matched() {
        // Trap 1's consequence: bare "DAW" is ambiguous across vendors.
        let c = vec![PortIdentity {
            device_name: "SomeOtherInterface".into(),
            entity_name: "DAW".into(),
            manufacturer: Some("Another Company".into()),
            online: true,
        }];
        assert!(matches!(
            find_port(&c, PortRole::Daw, ""),
            Err(MidiError::PortNotFound { .. })
        ));
    }

    #[test]
    fn two_identical_keyboards_are_reported_as_ambiguous() {
        // Guessing here would send output to the wrong instrument.
        let mut c = observed();
        c.extend(observed());
        assert!(matches!(
            find_port(&c, PortRole::Daw, "KONTROL S88 MK3"),
            Err(MidiError::AmbiguousPort {
                role: PortRole::Daw,
                count: 2
            })
        ));
    }

    #[test]
    fn the_device_filter_separates_different_models() {
        let mut c = observed();
        c.push(id("KONTROL S61 MK3", "DAW", true));
        let i = find_port(&c, PortRole::Daw, "S61").unwrap();
        assert_eq!(c[i].device_name, "KONTROL S61 MK3");
        // Without a filter the two models are ambiguous, correctly.
        assert!(matches!(
            find_port(&c, PortRole::Daw, ""),
            Err(MidiError::AmbiguousPort { .. })
        ));
    }

    #[test]
    fn a_missing_manufacturer_falls_back_to_the_device_name() {
        // Not every platform reports a manufacturer string.
        let c = vec![PortIdentity {
            device_name: "KONTROL S88 MK3".into(),
            entity_name: "DAW".into(),
            manufacturer: None,
            online: true,
        }];
        assert!(find_port(&c, PortRole::Daw, "").is_ok());
    }
}
