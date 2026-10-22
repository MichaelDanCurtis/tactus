//! Eight labelled knobs on the MK3 screen, then listen for what turning them
//! sends.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example knobs -- [seconds]
//! ```

use std::time::{Duration, Instant};

use rmpv::Value;
use tactus_odr::{Client, Knob, OdrError, Registry};

fn main() -> Result<(), OdrError> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    let Ok(path) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;
    let mut client =
        Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
    let serial = "99FE7169";

    client.instance_hello("tactus-knobs", "0.1.0")?;
    println!("connect_device: {}", client.connect_device(serial)?);
    client.request_focus(serial)?;
    if let Some(page) = client.registry().id("parameter") {
        client.set_page(serial, page)?;
    }

    let names = [
        "Cutoff",
        "Resonance",
        "Drive",
        "Attack",
        "Decay",
        "Sustain",
        "Release",
        "Volume",
    ];
    let displays = ["10%", "20%", "30%", "40%", "50%", "60%", "70%", "80%"];
    let knobs: Vec<Knob<'_>> = names
        .iter()
        .zip(displays.iter())
        .enumerate()
        .map(|(i, (name, display))| Knob {
            id: i as u32,
            name,
            value: (i as f64 + 1.0) / 10.0,
            display,
        })
        .collect();
    client.set_parameter_page(serial, "TACTUS", "Synth", &knobs)?;
    println!("page sent: TACTUS / Synth / 8 knobs. Turn the knobs — {seconds}s.\n");

    // The host owns the values: apply each step and send the result back.
    let mut values: Vec<f64> = knobs.iter().map(|k| k.value).collect();

    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        for n in client.listen(Duration::from_millis(50))? {
            let params = client.registry().humanise(&n.params);
            // plugin_action [serial, {action: payload}, bool]
            let Value::Array(items) = &params else {
                continue;
            };
            let Some(Value::Map(action)) = items.get(1) else {
                continue;
            };
            for (key, payload) in action {
                match (key.as_str(), payload) {
                    (Some("plugin_parameter_value_change"), Value::Array(p)) => {
                        let id = p.first().and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                        let step = p.get(1).and_then(Value::as_f64).unwrap_or(0.0);
                        if let Some(v) = values.get_mut(id) {
                            *v = (*v + step).clamp(0.0, 1.0);
                            let display = format!("{:.0}%", *v * 100.0);
                            client.set_parameter_value(serial, id as u32, *v, &display)?;
                        }
                    }
                    (Some("plugin_parameter_erp_touched"), Value::Array(p)) => {
                        let id = p.first().and_then(Value::as_u64).unwrap_or(0);
                        let on = p.get(1).and_then(Value::as_bool).unwrap_or(false);
                        let name = names.get(id as usize).copied().unwrap_or("?");
                        println!(
                            "[{:6.2}s] {} {name}",
                            start.elapsed().as_secs_f32(),
                            if on { "touch  " } else { "release" }
                        );
                    }
                    _ => {}
                }
            }
        }
        if last.elapsed() >= Duration::from_secs(1) {
            client.request_focus(serial)?;
            last = Instant::now();
        }
    }
    println!("\nfinal values:");
    for (name, v) in names.iter().zip(values.iter()) {
        println!("  {name:<10} {:>4.0}%", v * 100.0);
    }
    Ok(())
}
