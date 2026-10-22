//! Button mapping: hold focus and print every event the keyboard sends.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example buttons -- [seconds]
//! ```
//!
//! Under ODR the device reports controls as **named** events to whichever
//! instance holds focus, so this is the whole button map: press each control
//! once and read the names off the screen. Focus is re-asserted every second
//! because it lapses when idle.

use std::time::{Duration, Instant};

use rmpv::Value;
use tactus_odr::{Client, OdrError, Registry};

fn main() -> Result<(), OdrError> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let Ok(path) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;
    let registry = Registry::from_json(&text)?;
    let mut client = Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, registry)?;

    let devices = client.call("get_devices", vec![], Duration::from_secs(3))?;
    let serial = match &devices {
        Value::Array(list) => list.iter().find_map(|d| match d {
            Value::Map(f) => f.iter().find_map(|(k, v)| {
                (k.as_str() == Some("serialnumber")).then(|| v.as_str().map(str::to_owned))?
            }),
            _ => None,
        }),
        _ => None,
    };
    let Some(serial) = serial else {
        eprintln!("no keyboard attached");
        return Ok(());
    };

    client.instance_hello("tactus-buttons", "0.1.0")?;
    let attached = client.connect_device(&serial)?;
    client.request_focus(&serial)?;
    println!("attached={attached}, focus held. Press every control once — {seconds}s.\n");

    let start = Instant::now();
    let mut last_focus = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        for n in client.listen(Duration::from_millis(250))? {
            let name = n.name.unwrap_or_else(|| format!("#{}", n.id));
            let params = client.registry().humanise(&n.params);
            println!("[{:6.2}s] {name}: {params}", start.elapsed().as_secs_f32());
        }
        if last_focus.elapsed() >= Duration::from_secs(1) {
            client.request_focus(&serial)?;
            last_focus = Instant::now();
        }
    }
    println!("\ndone");
    Ok(())
}
