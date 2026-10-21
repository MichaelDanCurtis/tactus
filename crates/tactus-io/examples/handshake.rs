//! Send the DAW-port handshake and observe whether the control surface wakes.
//!
//! ```text
//! cargo run -p tactus-io --example handshake -- [watch_seconds]
//! ```
//!
//! # What this answers
//!
//! An S88 MK3 emits no HID input reports and no MIDI control change while its
//! buttons and knobs are operated — established with the vendor's background
//! agents running and with all of them stopped. See
//! `docs/protocol/mk3-usb-interfaces.md`.
//!
//! The vendor's own Cubase MIDI Remote script opens the DAW port and sends a
//! three-byte control change before expecting anything back. The channel-16 CC
//! map in `docs/protocol/mk3-daw-port.md` also contains *host → device*
//! capability enables, which only make sense once a conversation has started.
//! So the hypothesis under test is: **the surface reports over the DAW port,
//! but only to a host that has claimed it.**
//!
//! # Why transmitting is justified here, having refused elsewhere
//!
//! This SDK does not send unverified bytes to hardware, and that rule stands
//! for feature report `0xD0`, for display data, and for the undocumented
//! output reports. This message is different on every axis that made those
//! dangerous:
//!
//! - It is a **MIDI control change**, the most ordinary message the transport
//!   carries. It writes no configuration register and no firmware.
//! - Its bytes come from the **vendor's own shipped script**, not from a guess.
//! - Nothing else holds the instrument, so no third party's session can be
//!   disrupted by whatever it does.
//! - It is reversible: [`handshake::GOODBYE`] relinquishes the surface, this
//!   program always sends it, and a power cycle resets anything regardless.
//!
//! The experiment is also *bounded*: one message, one variable.
//! `ENABLE_14BIT_PARAMS` is deliberately **not** sent, so that if the surface
//! does wake, we know which message woke it.

use std::io::Write;
use std::time::{Duration, Instant};

use tactus_midi::daw::{cc, handshake};
use tactus_io::{MidiIn, MidiOut, PortRole};

/// The device → host meaning of a channel-16 CC, from the vendor's script.
///
/// Three of these numbers are **overloaded by direction**: `0x20`–`0x22` are a
/// button press coming from the device and a capability enable going to it.
/// Since this tool only ever reports what arrives, the inbound meaning is the
/// right one — but the collision is named so nobody reads this output as
/// evidence about the outbound direction.
fn cc_name(number: u8) -> &'static str {
    match number {
        // Not in the `cc` module, but it is the controller `handshake::HELLO`
        // uses, and the device answers on it with its protocol version.
        0x01 => "PROTOCOL_VERSION (handshake)",
        0x02 => "RELINQUISH (handshake)",
        0x06 => "ENABLE_14BIT_PARAMS",
        cc::SHIFT => "SHIFT",
        cc::BANK_MAPPING => "BANK_MAPPING",
        cc::PLAY => "PLAY",
        cc::RECORD => "RECORD",
        cc::COUNT_IN => "COUNT_IN",
        cc::STOP => "STOP",
        cc::LOOP => "LOOP",
        cc::METRONOME => "METRONOME",
        cc::TAP_TEMPO => "TAP_TEMPO",
        cc::UNDO => "UNDO (outbound: CAN_UNDO)",
        cc::REDO => "REDO (outbound: CAN_REDO)",
        cc::QUANTIZE => "QUANTIZE (outbound: CAN_QUANTIZE)",
        cc::AUTO => "AUTO",
        cc::NAV_TRACK => "NAV_TRACK",
        cc::NAV_BANK => "NAV_BANK",
        cc::NAV_CLIP => "NAV_CLIP",
        cc::MOVE_TRANSPORT => "MOVE_TRANSPORT (relative)",
        cc::MOVE_LOOP => "MOVE_LOOP (relative)",
        cc::SELECT_RAW => "SELECT_RAW",
        cc::MUTE_RAW => "MUTE_RAW",
        cc::SOLO_RAW => "SOLO_RAW",
        cc::SELECTED_VOLUME => "SELECTED_VOLUME",
        cc::SELECTED_PAN => "SELECTED_PAN",
        0x50..=0x57 => "VOLUME_DISPLAY_BASE + n",
        0x58..=0x5F => "PAN_DISPLAY_BASE + n",
        0x70..=0x77 => "PARAM_VALUE_BASE + n",
        _ => "not in the vendor's map",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render one message, naming a channel-16 CC where the vendor's map has one.
fn render(bytes: &[u8]) -> String {
    let status = bytes.first().copied().unwrap_or(0);
    // 0xBF is a control change on channel 16 — the DAW-port control channel.
    if status == 0xBF {
        if let (Some(&number), Some(&value)) = (bytes.get(1), bytes.get(2)) {
            return format!(
                "CC ch16 {number:#04X} = {value:<3}  {:<28} | {}",
                cc_name(number),
                hex(bytes)
            );
        }
    }
    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => {
            format!("{:<44} | {}", "channel message", hex(bytes))
        }
        0xF0 if status == 0xF0 => format!("{:<44} | {}", "SYSEX", hex(bytes)),
        _ => format!("{:<44} | {}", "other", hex(bytes)),
    }
}

struct Listener {
    label: &'static str,
    port: MidiIn,
}

/// Drain every port, printing what arrived. Returns how many messages came in.
fn drain(listeners: &[Listener], started: &Instant) -> usize {
    let mut count = 0;
    for l in listeners {
        while let Some(message) = l.port.recv_timeout(Duration::from_millis(5)) {
            count += 1;
            println!(
                "[{:7.3}s] {} | {}",
                started.elapsed().as_secs_f32(),
                l.label,
                render(&message)
            );
            let _ = std::io::stdout().flush();
        }
    }
    count
}

/// Watch for a fixed window, printing anything that arrives.
fn watch_for(listeners: &[Listener], started: &Instant, window: Duration) -> usize {
    let until = Instant::now() + window;
    let mut total = 0;
    while Instant::now() < until {
        let n = drain(listeners, started);
        total += n;
        if n == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    total
}

fn main() {
    let watch_seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    // Inputs first, so nothing the device says in reply can be missed.
    let mut listeners = Vec::new();
    for (role, label) in [
        (PortRole::Main, "Main"),
        (PortRole::Daw, "DAW "),
        (PortRole::Ext, "Ext "),
    ] {
        match MidiIn::open(role, "KONTROL") {
            Ok(port) => {
                println!("listening on {label}");
                listeners.push(Listener { label, port });
            }
            Err(e) => println!("{label}: not opened — {e}"),
        }
    }
    if listeners.is_empty() {
        eprintln!("no input ports; nothing to observe");
        std::process::exit(1);
    }

    let mut out = match MidiOut::open(PortRole::Daw, "KONTROL") {
        Ok(o) => o,
        Err(e) => {
            eprintln!("could not open the DAW output port: {e}");
            std::process::exit(1);
        }
    };
    println!("DAW output open\n");

    let started = Instant::now();

    // Phase 1 — baseline. If anything arrives here, the device was already
    // talking and the handshake cannot be credited for what follows.
    println!("== phase 1: baseline, 1.5s, transmitting nothing ==");
    let baseline = watch_for(&listeners, &started, Duration::from_millis(1500));
    println!("baseline: {baseline} message(s) (expected 0)\n");

    // Phase 2 — the one message under test.
    println!(
        "== phase 2: sending HELLO {} ==",
        hex(&handshake::HELLO)
    );
    if let Err(e) = out.send_raw(&handshake::HELLO) {
        eprintln!("send failed: {e}");
        std::process::exit(1);
    }
    println!("sent; watching 3s for a reply");
    let replied = watch_for(&listeners, &started, Duration::from_secs(3));
    println!(
        "reply: {replied} message(s){}\n",
        if replied == 0 {
            " — silence is a result, not a failure"
        } else {
            ""
        }
    );

    // Phase 3 — the actual question, in stages.
    //
    // The vendor's map contains host → device "capability enables", which only
    // make sense once a conversation has started. If one of them is what makes
    // the surface report, sending them all at once would not say which. So each
    // stage adds exactly one thing and gets its own watch window, and the
    // per-stage counts below are what attributes the effect.
    let stage_window = Duration::from_secs(watch_seconds.max(3) / 3);
    let mut stages: Vec<(&str, usize)> = Vec::new();

    println!("== phase 3 ==");
    println!("Operate controls CONTINUOUSLY for the whole of this phase.");
    println!("Each stage below lasts {}s.\n", stage_window.as_secs());

    println!("-- stage A: handshake only (nothing further sent) --");
    let a = watch_for(&listeners, &started, stage_window);
    println!("stage A: {a} message(s)\n");
    stages.push(("handshake only", a));

    println!(
        "-- stage B: + ENABLE_14BIT_PARAMS {} --",
        hex(&handshake::ENABLE_14BIT_PARAMS)
    );
    if let Err(e) = out.send_raw(&handshake::ENABLE_14BIT_PARAMS) {
        eprintln!("send failed: {e}");
    }
    let b = watch_for(&listeners, &started, stage_window);
    println!("stage B: {b} message(s)\n");
    stages.push(("+ 14-bit params", b));

    // The navigation enables, verbatim from the vendor's table. The 0x03 value
    // is documented there as an inferred two-bit mask, so it is a weaker claim
    // than the handshake -- but it is still the vendor's value, not ours.
    println!("-- stage C: + navigation enables (BF 30/31/32 03) --");
    for number in [cc::NAV_TRACK, cc::NAV_BANK, cc::NAV_CLIP] {
        let message = [0xBF, number, 0x03];
        println!("   sending {}", hex(&message));
        if let Err(e) = out.send_raw(&message) {
            eprintln!("send failed: {e}");
        }
    }
    let c = watch_for(&listeners, &started, stage_window);
    println!("stage C: {c} message(s)\n");
    stages.push(("+ nav enables", c));

    let observed = a + b + c;

    // Phase 4 — always release the surface, whatever happened.
    println!("\n== phase 4: releasing with GOODBYE {} ==", hex(&handshake::GOODBYE));
    match out.send_raw(&handshake::GOODBYE) {
        Ok(()) => println!("released"),
        Err(e) => eprintln!("could not send GOODBYE: {e} — power cycle to be sure"),
    }

    println!("\n== summary ==");
    println!("baseline (before handshake): {baseline}");
    println!("reply to HELLO:              {replied}");
    println!("during control operation:    {observed}");
    for (label, n) in &stages {
        println!("    {label:<18} {n}");
    }
    if observed > 0 {
        println!("\nThe surface reports over the DAW port once claimed.");
    } else {
        println!("\nNo control traffic. The handshake alone does not wake the surface.");
    }
}
