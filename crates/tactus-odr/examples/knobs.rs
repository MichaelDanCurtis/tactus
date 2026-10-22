//! Eight labelled knobs on the MK3 screen, then listen for what turning them
//! sends.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example knobs -- [seconds]
//! ```

use std::time::{Duration, Instant};

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

    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        for n in client.listen(Duration::from_millis(250))? {
            let name = n.name.unwrap_or_else(|| format!("#{}", n.id));
            println!(
                "[{:6.2}s] {name}: {}",
                start.elapsed().as_secs_f32(),
                client.registry().humanise(&n.params)
            );
        }
        if last.elapsed() >= Duration::from_secs(1) {
            // Focus lapses when idle, and the page shows only while held.
            client.request_focus(serial)?;
            last = Instant::now();
        }
    }
    Ok(())
}
