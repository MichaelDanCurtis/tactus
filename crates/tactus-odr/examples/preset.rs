//! Show an NKS preset on the MK3: its plugin name, every knob page with the
//! preset's own section headers and labels, and knobs that turn.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example preset -- <preset.nksf> [seconds]
//! ```
//!
//! This shows a preset; it does not host the plugin. Knob values are kept
//! here, starting at 50%, and are not the plugin's. Anything the device sends
//! that is not a knob is printed, which is how the page buttons get mapped.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tactus_nks::Preset;
use tactus_odr::events::{self, Event};
use tactus_odr::{
    Client, Control, HostState, Knob, KnobSection, OdrError, Record, Registry, Transport,
};

fn main() -> Result<(), OdrError> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: preset <preset.nksf> [seconds]");
        return Ok(());
    };
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);
    let preset = Preset::read(&path).map_err(|e| OdrError::Decode(e.to_string()))?;
    let Ok(registry) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&registry).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;

    // What the screen calls the plugin: its own name, else the product.
    let plugin = preset
        .plugin
        .name
        .clone()
        .or_else(|| preset.info.bankchain.first().cloned())
        .unwrap_or_else(|| "NKS".into());
    let pages: Vec<_> = preset.pages.iter().filter(|p| !p.is_empty()).collect();
    println!(
        "{} — {plugin}, {} pages, {} bytes of plugin state left untouched",
        preset.info.name,
        pages.len(),
        preset.state_len
    );

    // Local values, one per parameter id, and the text shown for each.
    let mut values: HashMap<u32, f64> = HashMap::new();
    for slot in pages.iter().flat_map(|p| p.slots.iter().flatten()) {
        values.insert(slot.id, 0.5);
    }
    let shown = |v: f64| format!("{:.0}%", v * 100.0);
    let displays: HashMap<u32, String> = values.iter().map(|(&id, &v)| (id, shown(v))).collect();
    let names: HashMap<u32, &str> = pages
        .iter()
        .flat_map(|p| p.slots.iter().flatten())
        .map(|s| (s.id, s.name.as_str()))
        .collect();

    let layout: Vec<Vec<KnobSection<'_>>> = pages
        .iter()
        .map(|page| {
            page.sections()
                .into_iter()
                .map(|sec| KnobSection {
                    name: sec.name,
                    knobs: sec
                        .slots
                        .iter()
                        .map(|s| {
                            s.map(|s| Knob {
                                id: s.id,
                                name: &s.name,
                                value: 0.5,
                                display: displays.get(&s.id).map_or("", String::as_str),
                            })
                        })
                        .collect(),
                })
                .collect()
        })
        .collect();

    let mut client =
        Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
    let serial = "99FE7169";
    client.instance_hello("tactus-preset", "0.1.0")?;
    println!("connect_device: {}", client.connect_device(serial)?);
    client.request_focus(serial)?;
    if let Some(page) = client.registry().id("parameter") {
        client.set_page(serial, page)?;
    }
    client.set_host_state(
        serial,
        &HostState {
            transport: Transport::Stopped,
            record: Record::Off,
            metro: Control::Off,
            loop_: Control::Off,
            tap_tempo: Control::Off,
            count_in: Control::Off,
            can_undo: false,
            can_redo: false,
            can_quantize: false,
            can_quantize_half: false,
            tempo: 120.0,
            track_name: "tactus",
            preset_name: &preset.info.name,
        },
    )?;
    client.set_parameter_layout(serial, &plugin, &layout, 0)?;
    println!(
        "layout sent, page 1 of {}. Turn knobs, press page buttons — {seconds}s.\n",
        layout.len()
    );
    for (i, page) in layout.iter().enumerate() {
        let heads: Vec<&str> = page.iter().map(|s| s.name).collect();
        println!("  page {:>2}: {}", i + 1, heads.join(" | "));
    }
    println!();

    let start = Instant::now();
    let mut last = Instant::now();
    let stamp = |t: Instant| format!("[{:6.2}s]", t.elapsed().as_secs_f32());
    while start.elapsed() < Duration::from_secs(seconds) {
        for n in client.listen(Duration::from_millis(50))? {
            for event in events::decode(client.registry(), &n) {
                match event {
                    Event::KnobTurn { id, delta } => {
                        if let Some(v) = values.get_mut(&id) {
                            *v = (*v + delta).clamp(0.0, 1.0);
                            client.set_parameter_value(serial, id, *v, &shown(*v))?;
                        }
                    }
                    Event::KnobTouch { id, touched } => println!(
                        "{} {} {}",
                        stamp(start),
                        if touched { "touch  " } else { "release" },
                        names.get(&id).copied().unwrap_or("?")
                    ),
                    other => println!("{} {other:?}", stamp(start)),
                }
            }
        }
        if last.elapsed() >= Duration::from_secs(1) {
            client.request_focus(serial)?;
            last = Instant::now();
        }
    }
    Ok(())
}
