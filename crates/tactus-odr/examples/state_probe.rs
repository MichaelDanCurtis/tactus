//! Learn the host-owned instance state schema from the agent's warnings.
//!
//! Brings up the knob page first so the device is in host mode, then sends
//! client_set_host_owned_instance_client_state variants and reports what the
//! agent says about each.

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use rmpv::Value;
use tactus_odr::{Client, Knob, OdrError, Registry};

fn log_path() -> String {
    format!(
        "{}/Library/Logs/Native Instruments/NIHardwareConnectionService.log",
        std::env::var("HOME").unwrap_or_default()
    )
}
fn log_len() -> u64 {
    std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0)
}
fn log_since(offset: u64) -> String {
    let mut out = String::new();
    if let Ok(mut f) = std::fs::File::open(log_path())
        && f.seek(SeekFrom::Start(offset)).is_ok()
    {
        let _ = f.read_to_string(&mut out);
    }
    out
}

/// What the agent said about one send.
fn report(label: &str, log: &str) {
    let pick = |needle: &str| -> Vec<String> {
        log.lines()
            .filter_map(|l| l.split(needle).nth(1))
            .map(|t| t.trim().chars().take(90).collect())
            .collect()
    };
    let missing = pick("expected key missing: ");
    let unexpected = pick("unexpected key found: ");
    let arity = pick("size mismatch: ");
    let fatal = log.contains("cannot be parsed");
    let other: Vec<String> = log
        .lines()
        .filter(|l| l.contains("[warning]") || l.contains("[error]"))
        .filter(|l| {
            !l.contains("key missing")
                && !l.contains("unexpected key")
                && !l.contains("size mismatch")
                && !l.contains("cannot be parsed")
        })
        .map(|l| {
            l.splitn(4, "] ")
                .last()
                .unwrap_or(l)
                .chars()
                .take(110)
                .collect()
        })
        .collect();
    println!("--- {label}");
    if !arity.is_empty() {
        println!("    arity: {arity:?}");
    }
    println!(
        "    {}missing={missing:?}",
        if fatal { "FATAL  " } else { "" }
    );
    if !unexpected.is_empty() {
        println!("    unexpected={unexpected:?}");
    }
    if !other.is_empty() {
        println!("    other={other:?}");
    }
}

fn main() -> Result<(), OdrError> {
    let Ok(path) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;
    let serial = "99FE7169";
    let s = |t: &str| Value::String(t.into());

    let m = |pairs: Vec<(&str, Value)>| {
        Value::Map(
            pairs
                .into_iter()
                .map(|(k, v)| (Value::String(k.into()), v))
                .collect(),
        )
    };
    let full = |flag: Value, flag2: Value| {
        vec![m(vec![
            (
                "transport_section_state",
                m(vec![
                    ("transport", s("playing")),
                    ("recording", s("off")),
                    ("metro", s("on")),
                    ("loop", s("off")),
                    ("tap_tempo", s("off")),
                    ("count_in", s("off")),
                ]),
            ),
            (
                "undo_state",
                m(vec![
                    ("can_undo", flag.clone()),
                    ("can_redo", flag2.clone()),
                ]),
            ),
            (
                "quantize_state",
                m(vec![("can_quantize", flag), ("can_quantize_half", flag2)]),
            ),
            ("smartplay_enabled", Value::Boolean(false)),
            ("browser_enabled", Value::Boolean(true)),
            ("tempo", Value::F64(120.0)),
            ("current_track_name", s("TACTUS")),
            ("current_preset_name", s("Init")),
        ])]
    };
    let variants: Vec<(&str, Vec<Value>)> = vec![
        (
            "full state, undo/quantize as control_state",
            full(s("on"), s("off")),
        ),
        (
            "full state, undo/quantize as bool",
            full(Value::Boolean(true), Value::Boolean(false)),
        ),
    ];
    for (label, extra) in variants {
        let mut c =
            Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
        c.instance_hello("tactus-state", "0.1.0")?;
        c.connect_device(serial)?;
        c.request_focus(serial)?;
        let knobs = [Knob {
            id: 0,
            name: "Cutoff",
            value: 0.5,
            display: "50%",
        }];
        c.set_parameter_page(serial, "TACTUS", "Synth", &knobs)?;
        std::thread::sleep(Duration::from_millis(400));

        let mark = log_len();
        let args = if extra.is_empty() {
            vec![s(c.instance_id()), Value::Map(vec![])]
        } else {
            let mut a = vec![s(c.instance_id()), s(serial)];
            a.extend(extra);
            a
        };
        c.notify_symbol("client_set_host_owned_instance_client_state", args)?;
        std::thread::sleep(Duration::from_millis(2200));
        report(label, &log_since(mark));
    }
    Ok(())
}
