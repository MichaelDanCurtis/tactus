//! Show an NKS preset on the MK3: its plugin name, every knob page with the
//! preset's own section headers and labels, and knobs that turn. The preset
//! up/down buttons step through the other presets in the same folder. When
//! the product ships a hardware banner, it is shown above the knobs.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example preset -- <preset.nksf> [seconds]
//! ```
//!
//! This shows a preset; it does not host the plugin. Knob values are kept
//! here, starting at 50%, and are not the plugin's. The page buttons need no
//! help: the device turns pages itself and reports where it went.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tactus_nks::{Preset, artwork};
use tactus_odr::events::{self, Event};
use tactus_odr::{
    Client, Control, HostState, Knob, KnobSection, OdrError, ParameterPage, Record, Registry, Rgb,
    Transport,
};

const SERIAL: &str = "99FE7169";

/// What the loop needs once a preset is on screen.
struct Shown {
    values: HashMap<u32, f64>,
    headers: Vec<String>,
}

fn shown(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// Send a preset's layout and name to the device, on its first page.
fn show(client: &mut Client, preset: &Preset) -> Result<Shown, OdrError> {
    let plugin = preset
        .plugin
        .name
        .clone()
        .or_else(|| preset.info.bankchain.first().cloned())
        .unwrap_or_else(|| "NKS".into());
    let pages: Vec<_> = preset.pages.iter().filter(|p| !p.is_empty()).collect();
    let slots = || pages.iter().flat_map(|p| p.slots.iter().flatten());
    let values: HashMap<u32, f64> = slots().map(|s| (s.id, 0.5)).collect();
    let display = shown(0.5);

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
                                display: &display,
                            })
                        })
                        .collect(),
                })
                .collect()
        })
        .collect();
    let headers = layout
        .iter()
        .map(|page| page.iter().map(|s| s.name).collect::<Vec<_>>().join(" | "))
        .collect();

    client.set_host_state(
        SERIAL,
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
    // The banner for the preset's product, if it ships one.
    let banner = preset.info.bankchain.first().and_then(|product| {
        artwork::hardware_banner(Path::new(artwork::MAC_RESOURCE_ROOT), product)
    });
    let background = match &banner {
        Some(path) => match std::fs::read(path) {
            Ok(image) => Some(client.add_asset(&image)?),
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                None
            }
        },
        None => None,
    };
    client.publish_parameter_page(
        SERIAL,
        &ParameterPage {
            plugin_name: &plugin,
            pages: &layout,
            show_page: 0,
            background,
            plugin_color: Some(Rgb::DEFAULT),
            control_color: Some(Rgb::DEFAULT),
        },
    )?;
    println!(
        "\n{} — {plugin}, {} pages, {} bytes of plugin state left untouched",
        preset.info.name,
        layout.len(),
        preset.state_len
    );
    match &banner {
        Some(path) => println!("banner: {}", path.display()),
        None => println!("banner: none for this product"),
    }
    Ok(Shown { values, headers })
}

/// The presets beside `path`, sorted by file name.
fn siblings(path: &Path) -> Vec<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "nksf"))
                .collect()
        })
        .unwrap_or_default();
    all.sort();
    all
}

fn main() -> Result<(), OdrError> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        eprintln!("usage: preset <preset.nksf> [seconds]");
        return Ok(());
    };
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);
    let Ok(registry) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&registry).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;
    let read = |p: &Path| Preset::read(p).map_err(|e| OdrError::Decode(e.to_string()));

    let mut path = PathBuf::from(first);
    let folder = siblings(&path);
    let mut client =
        Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
    client.instance_hello("tactus-preset", "0.1.0")?;
    println!("connect_device: {}", client.connect_device(SERIAL)?);
    client.request_focus(SERIAL)?;
    if let Some(page) = client.registry().id("parameter") {
        client.set_page(SERIAL, page)?;
    }
    let mut on_screen = show(&mut client, &read(&path)?)?;
    println!(
        "{} presets in this folder. Knobs, page buttons, preset up/down — {seconds}s.",
        folder.len()
    );

    let start = Instant::now();
    let mut last = Instant::now();
    let stamp = |t: Instant| format!("[{:6.2}s]", t.elapsed().as_secs_f32());
    while start.elapsed() < Duration::from_secs(seconds) {
        for n in client.listen(Duration::from_millis(50))? {
            for event in events::decode(client.registry(), &n) {
                match event {
                    Event::KnobTurn { id, delta } => {
                        if let Some(v) = on_screen.values.get_mut(&id) {
                            *v = (*v + delta).clamp(0.0, 1.0);
                            client.set_parameter_value(SERIAL, id, *v, &shown(*v))?;
                        }
                    }
                    Event::KnobTouch { .. } => {}
                    Event::ParameterPage(i) => println!(
                        "{} page {}: {}",
                        stamp(start),
                        i + 1,
                        on_screen
                            .headers
                            .get(i as usize)
                            .map_or("?", String::as_str)
                    ),
                    // Preset up/down: step through the folder, wrapping.
                    Event::PresetStep(step) => {
                        let Some(here) = folder.iter().position(|p| *p == path) else {
                            continue;
                        };
                        let len = folder.len() as i64;
                        let next = (here as i64 + i64::from(step)).rem_euclid(len.max(1)) as usize;
                        if let Some(p) = folder.get(next) {
                            print!("{} preset {step:+} →", stamp(start));
                            match read(p) {
                                Ok(preset) => {
                                    path = p.clone();
                                    on_screen = show(&mut client, &preset)?;
                                }
                                Err(e) => println!(" {}: {e}", p.display()),
                            }
                        }
                    }
                    other => println!("{} {other:?}", stamp(start)),
                }
            }
        }
        if last.elapsed() >= Duration::from_secs(1) {
            client.request_focus(SERIAL)?;
            last = Instant::now();
        }
    }
    Ok(())
}
