//! Learn the shape of client_parameter_page_set_parameter_value from the
//! agent's own warnings. Publishes a real page first so the parameter exists.

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
    let m =
        |pairs: Vec<(&str, Value)>| Value::Map(pairs.into_iter().map(|(k, v)| (s(k), v)).collect());
    let pv = || {
        m(vec![
            ("value", Value::F64(0.9)),
            ("display_value", s("90%")),
        ])
    };

    // Four arguments: (instance, serial, parameter_id, value).
    let variants: Vec<(&str, Value)> = vec![
        ("value = {value, display_value}", pv()),
        (
            "value = {continuous_parameter: {value, display_value}}",
            m(vec![("continuous_parameter", pv())]),
        ),
        (
            "value = {continuous_parameter_value: {...}}",
            m(vec![("continuous_parameter_value", pv())]),
        ),
        ("value = 0.9", Value::F64(0.9)),
    ];
    for (label, payload) in variants {
        let mut c =
            Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
        c.instance_hello("tactus-value", "0.1.0")?;
        c.connect_device(serial)?;
        c.request_focus(serial)?;
        let knobs = [Knob {
            id: 0,
            name: "Cutoff",
            value: 0.1,
            display: "10%",
        }];
        c.set_parameter_page(serial, "TACTUS", "Synth", &knobs)?;
        std::thread::sleep(Duration::from_millis(400));

        let mark = log_len();
        let args = vec![s(c.instance_id()), s(serial), Value::from(0u32), payload];
        c.notify_symbol("client_parameter_page_set_parameter_value", args)?;
        std::thread::sleep(Duration::from_millis(2200));
        let log = log_since(mark);
        if log.contains("cannot be parsed") {
            let why: Vec<&str> = log
                .lines()
                .filter(|l| l.contains("size mismatch"))
                .collect();
            println!(
                "  {label:<52} FATAL {}",
                why.first()
                    .map(|l| l.split("] ").last().unwrap_or(""))
                    .unwrap_or("")
                    .chars()
                    .take(70)
                    .collect::<String>()
            );
            continue;
        }
        let missing: Vec<&str> = log
            .lines()
            .filter_map(|l| l.split("expected key missing: ").nth(1))
            .map(str::trim)
            .collect();
        let unexpected: Vec<&str> = log
            .lines()
            .filter_map(|l| l.split("unexpected key found: ").nth(1))
            .map(str::trim)
            .collect();
        let other: Vec<&str> = log
            .lines()
            .filter(|l| l.contains("[warning]") || l.contains("[error]"))
            .filter(|l| !l.contains("key missing") && !l.contains("unexpected key"))
            .map(|l| l.splitn(4, "] ").last().unwrap_or(l))
            .collect();
        println!("  {label:<52} ok  missing={missing:?} unexpected={unexpected:?} other={other:?}");
    }
    Ok(())
}
