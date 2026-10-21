//! Live-hardware checks. **Requires a Kontrol MK3 keyboard attached.**
//!
//! Ignored by default so CI never needs hardware. Run deliberately:
//!
//! ```text
//! cargo test -p tactus-io --test live_s88 -- --ignored --nocapture
//! ```
//!
//! What this is allowed to do to the instrument, and nothing more:
//!
//! - read the three declared feature reports (`0xD0`, `0xD8`, `0xD9`);
//! - listen for input reports;
//! - write the light guide **all zeros**, i.e. all keys off — in range by
//!   definition and visually neutral.
//!
//! It never writes a feature report, never sends display or page data, and
//! never sends MIDI. Strings it reads off the device are printed as data.

use std::time::{Duration, Instant};

use tactus_io::{
    Device, IoError, Model, PortRole, list_devices, list_input_ports, list_output_ports, report_id,
};
use tactus_midi::port::find_port;

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore = "requires an attached Kontrol MK3 keyboard"]
fn live_hid_round_trip() {
    let devices = list_devices().expect("HID enumeration");
    println!("\n== enumeration ==");
    println!("{} Kontrol MK3 keyboard(s) attached", devices.len());
    for info in &devices {
        println!(
            "  {:?} pid 0x{:04X} usb_serial {:?} product {:?} iface {} path {}",
            info.model,
            info.product_id,
            info.usb_serial,
            info.product_string,
            info.interface_number,
            info.path_lossy()
        );
    }
    assert!(!devices.is_empty(), "no MK3 keyboard attached");

    let info = devices
        .iter()
        .find(|d| d.model == Model::S88)
        .expect("an S88 (product id 0x2120) to be attached");
    assert_eq!(info.product_id, 0x2120, "S88 product id");

    // Opens shared, not exclusive: a running Komplete Kontrol keeps the device.
    let device = match Device::open(info) {
        Ok(device) => device,
        Err(err) => panic!("{err}"),
    };
    println!("\n== open ==");
    println!("opened {:?} at {}", device.model(), info.path_lossy());
    assert_eq!(device.model(), Model::S88);

    // The "never seize the device" policy, checked rather than asserted: macOS
    // hidapi opens exclusively by default, which would take the keyboard away
    // from a running Komplete Kontrol or NI agent.
    #[cfg(target_os = "macos")]
    {
        let exclusive = device
            .is_open_exclusive()
            .expect("query the open mode of the handle");
        println!("opened exclusively (seizing the device)? {exclusive}");
        assert!(!exclusive, "tactus must open shared, never seize");
    }

    println!("\n== feature reports ==");

    // 0xD9 — ASCII serial. Device-supplied data; printed, never interpreted.
    let serial = device.serial().expect("serial from feature report 0xD9");
    let mut d9 = [0u8; tactus_io::FEATURE_BUFFER_LEN];
    let d9_len = device
        .read_feature(report_id::FEAT_SERIAL, &mut d9)
        .expect("read 0xD9");
    println!("D9 ({d9_len} bytes): {}", hex(&d9[..d9_len]));
    println!("D9 serial: {serial:?}");
    assert!(!serial.is_empty(), "serial is non-empty");
    assert!(serial.is_ascii(), "serial is ASCII");

    // 0xD8 — read-only identity/version. The documented prefix for this
    // firmware is D8 05 01 00 13 (docs/protocol/mk3-hid.md); asserted only for
    // the model it was observed on.
    let mut d8 = [0u8; tactus_io::FEATURE_BUFFER_LEN];
    let d8_len = device
        .read_feature(report_id::FEAT_IDENTITY, &mut d8)
        .expect("read 0xD8");
    println!("D8 ({d8_len} bytes): {}", hex(&d8[..d8_len]));
    assert_eq!(
        &d8[..5],
        &[0xD8, 0x05, 0x01, 0x00, 0x13],
        "0xD8 identity prefix for this firmware"
    );

    // 0xD0 — settable, semantics unknown. Reading is fine; writing is refused.
    let mut d0 = [0u8; tactus_io::FEATURE_BUFFER_LEN];
    let d0_len = device
        .read_feature(report_id::FEAT_CONFIG, &mut d0)
        .expect("read 0xD0");
    println!("D0 ({d0_len} bytes): {}", hex(&d0[..d0_len]));

    // Guards, against the live device rather than a mock.
    let refused = device
        .write_feature(report_id::FEAT_CONFIG, &[0u8; 32])
        .expect_err("writing 0xD0 must be refused");
    assert!(matches!(refused, IoError::FeatureWriteRefused { id } if id == 0xD0));
    println!("write 0xD0 refused: {refused}");

    // 0xF8 is the report the vendor's own table claims and this firmware lacks.
    let undeclared = device
        .read_feature(0xF8, &mut d0)
        .expect_err("0xF8 must be refused before it reaches the device");
    assert!(matches!(
        undeclared,
        IoError::UndeclaredFeatureReport { .. }
    ));
    println!("read 0xF8 refused: {undeclared}");

    // Input reports. Nobody is touching the keyboard, so zero is the expected
    // result and emphatically not a failure.
    println!("\n== input polling (5s, nobody touching the keyboard) ==");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut count = 0usize;
    while Instant::now() < deadline {
        match device.poll_input(Duration::from_millis(250)) {
            Ok(Some(report)) => {
                count += 1;
                if count <= 8 {
                    println!("  {report:?}");
                }
            }
            Ok(None) => {}
            Err(err) => panic!("poll_input failed: {err}"),
        }
    }
    println!("{count} input report(s) in 5s (zero is expected when idle)");

    // Light guide: all zeros, all keys off.
    println!("\n== light guide ==");
    device
        .clear_light_guide()
        .expect("write light guide all zeros");
    println!("wrote report 0x82 with {} zero values", Model::S88.keys());

    // Validation still comes from tactus-hid, on the real path.
    let wrong_len = device
        .set_light_guide(&[0u8; 61])
        .expect_err("61 values on an 88-key device must be refused");
    println!("wrong length refused: {wrong_len}");
    let out_of_range = device
        .set_light_guide(&{
            let mut v = [0u8; 88];
            v[0] = 128;
            v
        })
        .expect_err("128 is outside the declared 0..=127 range");
    println!("out-of-range refused: {out_of_range}");
}

#[test]
#[ignore = "requires an attached Kontrol MK3 keyboard"]
fn live_midi_port_resolution() {
    // Enumerate and resolve only. Nothing is connected and nothing is sent:
    // this proves the identity mapping against real platform names.
    for (direction, ports) in [
        ("input", list_input_ports().expect("MIDI input enumeration")),
        (
            "output",
            list_output_ports().expect("MIDI output enumeration"),
        ),
    ] {
        println!("\n== {direction} ports ({}) ==", ports.len());
        for port in &ports {
            println!(
                "  {:?} device {:?} entity {:?} ni {}",
                port.role(),
                port.device_name,
                port.entity_name,
                port.is_native_instruments()
            );
        }
        for role in PortRole::ALL {
            match find_port(&ports, role, tactus_io::KONTROL_FILTER) {
                Ok(index) => println!(
                    "  {role:?} -> [{index}] {}",
                    ports
                        .get(index)
                        .map(|p| p.display_name())
                        .unwrap_or_default()
                ),
                Err(err) => println!("  {role:?} -> {err}"),
            }
        }
    }
}
