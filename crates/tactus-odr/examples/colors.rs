//! Find out what `plugin_color` and `nks_control_color` tint, one change at a
//! time, paced by the person watching: preset **down** goes to the next step,
//! preset **up** goes back.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example colors -- [minutes]
//! ```

use std::path::Path;
use std::time::{Duration, Instant};

use tactus_nks::artwork;
use tactus_odr::events::{self, Event};
use tactus_odr::{Client, Knob, KnobSection, OdrError, ParameterPage, Registry, Rgb};

const SERIAL: &str = "99FE7169";

const BLUE: Rgb = Rgb::DEFAULT;
const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
const GREEN: Rgb = Rgb { r: 0, g: 255, b: 0 };
const WHITE: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

fn main() -> Result<(), OdrError> {
    let minutes: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let Ok(path) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;

    // (screen label, plugin_color, nks_control_color)
    let steps: [(&str, Option<Rgb>, Option<Rgb>); 8] = [
        ("Baseline: both blue", Some(BLUE), Some(BLUE)),
        ("PLUGIN red", Some(RED), Some(BLUE)),
        ("PLUGIN green", Some(GREEN), Some(BLUE)),
        ("CONTROL red", Some(BLUE), Some(RED)),
        ("CONTROL green", Some(BLUE), Some(GREEN)),
        ("Both white", Some(WHITE), Some(WHITE)),
        ("Both black", Some(BLACK), Some(BLACK)),
        ("Both nil (device default)", None, None),
    ];

    let mut client =
        Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
    client.instance_hello("tactus-colors", "0.1.0")?;
    println!("connect_device: {}", client.connect_device(SERIAL)?);
    client.request_focus(SERIAL)?;
    if let Some(page) = client.registry().id("parameter") {
        client.set_page(SERIAL, page)?;
    }
    let background =
        match artwork::hardware_banner(Path::new(artwork::MAC_RESOURCE_ROOT), "Super 8 R2") {
            Some(p) => std::fs::read(&p)
                .ok()
                .map(|img| client.add_asset(&img))
                .transpose()?,
            None => None,
        };

    // Knobs at spread-out positions, so value arcs are easy to see.
    let names = [
        "Cutoff", "Reso", "Drive", "Attack", "Decay", "Sustain", "Release", "Volume",
    ];
    let displays: Vec<String> = (1..=8).map(|i| format!("{}%", i * 12)).collect();
    let show = |client: &mut Client, i: usize| -> Result<(), OdrError> {
        let Some(&(label, plugin_color, control_color)) = steps.get(i) else {
            return Ok(());
        };
        let title = format!("Colour {}/{}", i + 1, steps.len());
        let knobs = names
            .iter()
            .zip(&displays)
            .enumerate()
            .map(|(k, (name, display))| {
                Some(Knob {
                    id: k as u32,
                    name,
                    value: (k as f64 + 1.0) * 0.12,
                    display,
                })
            })
            .collect();
        let pages = [vec![KnobSection { name: label, knobs }]];
        client.publish_parameter_page(
            SERIAL,
            &ParameterPage {
                plugin_name: &title,
                pages: &pages,
                show_page: 0,
                background,
                plugin_color,
                control_color,
            },
        )
    };

    let start = Instant::now();
    let stamp = |t: Instant| format!("[{:6.2}s]", t.elapsed().as_secs_f32());
    let mut step = 0usize;
    show(&mut client, step)?;
    println!("{} step 1: {}", stamp(start), steps[0].0);
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(minutes * 60) {
        for n in client.listen(Duration::from_millis(50))? {
            for event in events::decode(client.registry(), &n) {
                if let Event::PresetStep(d) = event {
                    let next = step.saturating_add_signed(d as isize).min(steps.len() - 1);
                    if next != step {
                        step = next;
                        show(&mut client, step)?;
                        println!("{} step {}: {}", stamp(start), step + 1, steps[step].0);
                    }
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
