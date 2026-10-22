//! Learn the parameter-page schema from the agent itself.
//!
//! Sends a deliberately minimal `client_parameter_page_set_data` and prints
//! what the agent logs about it. The agent names each missing field
//! (`expected key missing: …`) and each unknown one, which is how the
//! notification schema was recovered. No images are sent: a malformed asset
//! handle is known to abort the agent.

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use rmpv::Value;
use tactus_odr::{Client, OdrError, Registry};

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
    if let Ok(mut f) = std::fs::File::open(log_path()) {
        if f.seek(SeekFrom::Start(offset)).is_ok() {
            let _ = f.read_to_string(&mut out);
        }
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
    println!("one field at a time on top of {{name}}:\n");

    let s = |t: &str| Value::String(t.into());
    let m = |pairs: Vec<(&str, Value)>| {
        Value::Map(pairs.into_iter().map(|(k, v)| (Value::String(k.into()), v)).collect())
    };
    let u = |n: u32| Value::from(n);
    let one = |id: Value, entry: Value| Value::Map(vec![(id, entry)]);
    // (label, extra fields inside plugin_data, extra fields beside it)
    let cp = |inner: Vec<(&str, Value)>| {
        vec![("parameters", one(u(0), m(vec![("continuous_parameter", m(inner))])))]
    };
    let pv = || m(vec![("value", Value::F64(0.5)), ("display_value", s("50%"))]);
    let with_style = |style: Option<Value>| {
        let mut inner = vec![("parameter_value", pv())];
        if let Some(st) = style {
            inner.push(("parameter_style", st));
        }
        cp(inner)
    };
    let st_nil = |tag: &str| Some(m(vec![(tag, Value::Nil)]));
    let variants: Vec<(&str, Vec<(&str, Value)>, Vec<(&str, Value)>)> = vec![
        ("style = {knob_parameter_style: nil}", with_style(st_nil("knob_parameter_style")), vec![]),
        ("style = {value_parameter_style: nil}", with_style(st_nil("value_parameter_style")), vec![]),
    ];
    let base_missing = [
        "background", "parameters", "plugin_color", "nks_control_color",
        "device_owned_viewstate", "host_owned_viewstate",
    ];

    for (label, inner, outer) in variants {
        let reg = Registry::from_json(&text)?;
        let mut client = Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, reg)?;
        client.instance_hello("tactus-page", "0.1.0")?;
        client.connect_device(serial)?;
        client.request_focus(serial)?;

        let mut data = vec![("name", s("TACTUS"))];
        data.extend(inner);
        let mut top = vec![("plugin_data", m(data))];
        top.extend(outer);
        let payload = m(top);
        let mark = log_len();
        let args = vec![s(client.instance_id()), s(serial), payload];
        client.notify_symbol("client_parameter_page_set_data", args)?;
        std::thread::sleep(Duration::from_millis(2200));
        let log = log_since(mark);

        if log.contains("cannot be parsed") {
            println!("  {label:<30} FATAL — wrong type");
            continue;
        }
        let mut missing: Vec<&str> = log
            .lines()
            .filter_map(|l| l.split("expected key missing: ").nth(1))
            .map(str::trim)
            .filter(|k| !base_missing.contains(k))
            .collect();
        missing.dedup();
        let unexpected: Vec<&str> = log
            .lines()
            .filter_map(|l| l.split("unexpected key found: ").nth(1))
            .map(str::trim)
            .collect();
        println!(
            "  {label:<30} ok   new-missing={missing:?} unexpected={unexpected:?}"
        );
    }
    Ok(())
}
