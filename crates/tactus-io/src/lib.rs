//! Platform HID and MIDI I/O for Kontrol S-series MK3 hardware.
//!
//! This is the layer that touches real devices. The protocol crates
//! (`tactus-hid`, `tactus-midi`) decide what bytes *mean*; this one decides how
//! they get there and what to do when the platform says no. The split is load
//! bearing: everything testable without hardware stays testable without
//! hardware, and there is exactly one place where a report layout or a port
//! matching rule can be wrong.
//!
//! ```no_run
//! use std::time::Duration;
//! use tactus_io::{Device, IoError, Model};
//!
//! # fn main() -> Result<(), IoError> {
//! let device = Device::open_model(Model::S88)?;
//! println!("serial {}", device.serial()?);
//! if let Some(report) = device.poll_input(Duration::from_millis(100))? {
//!     println!("{report:?}");
//! }
//! device.clear_light_guide()?;
//! # Ok(())
//! # }
//! ```
//!
//! # What this crate will not do
//!
//! Three refusals, each because the alternative is to send bytes to somebody's
//! instrument on a guess:
//!
//! - **No feature-report writes.** `0xD8` and `0xD9` are read-only; `0xD0` is
//!   settable with entirely unknown semantics. [`Device::write_feature`] exists
//!   only to say so. See `docs/protocol/mk3-hid.md`.
//! - **No display or page data.** The MK3 renders its own UI, nothing about a
//!   host-side framebuffer is verified, and unverified bytes must not reach the
//!   hardware. There is no API here that can send any.
//! - **No seizing the device.** macOS HID opens are exclusive-by-default and
//!   would take the keyboard away from a running Komplete Kontrol; this crate
//!   opens shared and reports a clear error instead of displacing another
//!   process. See [`Device::open`].
//!
//! Writing the light guide (output report `0x82`) *is* supported: it is
//! device-declared, its range is device-declared, and the values are validated
//! upstream by [`tactus_hid::encode_light_guide`].
//!
//! # Panics
//!
//! None by construction. The crate denies `unwrap`, `expect`, `panic!` and
//! indexing outside its tests, and every failure is an [`IoError`] variant that
//! names the value that caused it.
//!
//! # Trust
//!
//! Serials, product strings and port names come from the device. They are
//! **data**: display them, compare them, log them. Nothing here interprets them
//! as a path, a command, or an instruction.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod error;
mod hid;
mod midi;

pub use error::IoError;
pub use hid::{
    DECLARED_FEATURE_REPORTS, Device, DeviceInfo, FEATURE_BUFFER_LEN, FEATURE_PAYLOAD_LEN,
    MAX_INPUT_REPORT_LEN, list_devices,
};
pub use midi::{
    CLIENT_NAME, KONTROL_FILTER, MidiIn, MidiOut, list_input_ports, list_output_ports,
    parse_port_name,
};

// Re-exported so a caller does not need every protocol crate in scope just to
// name the types this crate hands back.
pub use tactus_hid::{InputReport, Model, VENDOR_ID, report_id};
pub use tactus_midi::port::{PortIdentity, PortRole};
