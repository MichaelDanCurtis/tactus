//! Watch all three MIDI ports at once and dump what arrives.
//!
//! Exists to answer a question the HID layer raised: the descriptor declares
//! input reports for the control surface, but a keyboard driven by the vendor's
//! software delivers nothing on them. The DAW-port protocol recovered from the
//! vendor's MIDI Remote script contains a full channel-16 control-change map,
//! which would only be there if buttons and knobs report over MIDI. This tool
//! checks that against the hardware.
//!
//! ```text
//! cargo run -p tactus-io --example midiwatch
//! ```
//!
//! Listening is read-only: it opens inputs and transmits nothing.

use std::time::{Duration, Instant};

use tactus_io::{MidiIn, PortRole};

/// A one-line decode of the message types we expect from a control surface.
///
/// Deliberately shallow — it names the MIDI message, not the control. Which
/// physical control a CC number belongs to is the thing being discovered, so
/// claiming it here would defeat the purpose.
fn describe(bytes: &[u8]) -> String {
    let Some(&status) = bytes.first() else {
        return "empty".to_owned();
    };
    let channel = (status & 0x0F) + 1;
    match status & 0xF0 {
        0x80 => match (bytes.get(1), bytes.get(2)) {
            (Some(n), Some(v)) => format!("note off  ch{channel:<2} note {n:<3} vel {v}"),
            _ => "note off (truncated)".to_owned(),
        },
        0x90 => match (bytes.get(1), bytes.get(2)) {
            (Some(n), Some(v)) => format!("note on   ch{channel:<2} note {n:<3} vel {v}"),
            _ => "note on (truncated)".to_owned(),
        },
        0xA0 => match (bytes.get(1), bytes.get(2)) {
            (Some(n), Some(v)) => format!("poly at   ch{channel:<2} note {n:<3} val {v}"),
            _ => "poly aftertouch (truncated)".to_owned(),
        },
        0xB0 => match (bytes.get(1), bytes.get(2)) {
            (Some(c), Some(v)) => format!("CC        ch{channel:<2} cc   {c:<3} val {v}"),
            _ => "CC (truncated)".to_owned(),
        },
        0xC0 => format!("program   ch{channel:<2}"),
        0xD0 => match bytes.get(1) {
            Some(v) => format!("chan at   ch{channel:<2} val  {v}"),
            None => "channel aftertouch (truncated)".to_owned(),
        },
        0xE0 => match (bytes.get(1), bytes.get(2)) {
            (Some(l), Some(m)) => {
                let value = (u16::from(*m) << 7) | u16::from(*l);
                format!("pitchbend ch{channel:<2} value {value}")
            }
            _ => "pitch bend (truncated)".to_owned(),
        },
        0xF0 if status == 0xF0 => format!("sysex     {} bytes", bytes.len()),
        _ => format!("status 0x{status:02X}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let roles = [
        (PortRole::Main, "Main"),
        (PortRole::Daw, "DAW "),
        (PortRole::Ext, "Ext "),
    ];

    let mut ports = Vec::new();
    for (role, label) in roles {
        match MidiIn::open(role, "KONTROL") {
            Ok(port) => {
                println!("listening on {label} ({})", port.identity().display_name());
                ports.push((label, port));
            }
            Err(e) => println!("{label}: not opened — {e}"),
        }
    }

    if ports.is_empty() {
        eprintln!("no ports opened; nothing to watch");
        std::process::exit(1);
    }

    println!("\nWatching. Press or turn anything. Ctrl-C to stop.\n");
    let started = Instant::now();

    loop {
        let mut idle = true;
        for (label, port) in &ports {
            while let Some(message) = port.recv_timeout(Duration::from_millis(5)) {
                idle = false;
                println!(
                    "[{:7.3}s] {label} | {:<34} | {}",
                    started.elapsed().as_secs_f32(),
                    describe(&message),
                    hex(&message)
                );
            }
        }
        if idle {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
