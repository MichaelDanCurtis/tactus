//! Load an NKS preset into its plugin and put the plugin on the keyboard: the
//! preset's pages with the plugin's **own** values and value text, knobs that
//! set the plugin's parameters, preset up/down through the folder, and the
//! keys playing the plugin through the default audio output.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-plugin-host --example kontrol -- <preset.nksf> [seconds]
//! ```
//!
//! The audio device's callback drives the plugin. Keys arrive on the
//! keyboard's main MIDI port; knob turns go to both the plugin's controller
//! (for its text) and its processor (for the sound).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tactus_io::{MidiIn, PortRole};

use tactus_nks::{Preset, artwork};
use tactus_odr::events::{self, Event};
use tactus_odr::{Client, Knob, KnobSection, ParameterPage, Registry, Rgb};
use tactus_plugin_host::{
    Audio, Input, Module, PARAM_VALUES_CHANGED, Plugin, class_id, pump_main_run_loop,
};

const SERIAL: &str = "99FE7169";
const BLOCK: usize = 512;
/// Largest block the plugin is set up for; device callbacks are chunked to it.
const MAX_BLOCK: usize = 4096;

/// Keeps the plugin's engine running in real time when no audio device is
/// driving it: call often. With an audio stream running it does nothing.
struct Clock {
    start: Instant,
    blocks: u64,
    rate: f64,
    driven_by_device: bool,
}

impl Clock {
    fn tick(&mut self, plugin: &mut Plugin<'_>) {
        if self.driven_by_device {
            return;
        }
        let due = (self.start.elapsed().as_secs_f64() * self.rate / BLOCK as f64) as u64;
        // Never try to catch up more than a few blocks at once.
        self.blocks = self.blocks.max(due.saturating_sub(4));
        while self.blocks < due {
            let _ = plugin.process_block(BLOCK);
            self.blocks += 1;
        }
    }
}

/// Load a preset's state and wait, engine running, until the plugin says its
/// values changed (`restartComponent` with `kParamValuesChanged`).
fn load(plugin: &mut Plugin<'_>, clock: &mut Clock, preset: &Preset) -> Result<f32, String> {
    plugin.take_restart_flags();
    plugin
        .load_state(&preset.plugin_state)
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        clock.tick(plugin);
        pump_main_run_loop(Duration::from_millis(5));
        if plugin.take_restart_flags() & PARAM_VALUES_CHANGED != 0 {
            return Ok(t.elapsed().as_secs_f32());
        }
    }
    Err("the plugin did not apply the preset within 5 s".into())
}

/// Publish the preset's pages with the plugin's values and text.
fn publish(
    client: &mut Client,
    plugin: &Plugin<'_>,
    preset: &Preset,
) -> Result<Vec<String>, String> {
    let e = |e: tactus_odr::OdrError| e.to_string();
    let product = preset.info.bankchain.first().cloned().unwrap_or_default();
    let root = Path::new(artwork::MAC_RESOURCE_ROOT);
    let background = match artwork::hardware_banner(root, &product) {
        Some(p) => std::fs::read(p)
            .ok()
            .map(|img| client.add_asset(&img))
            .transpose()
            .map_err(e)?,
        None => None,
    };
    let arc = artwork::control_color(root, &product).map(|[r, g, b]| Rgb { r, g, b });

    let pages: Vec<_> = preset.pages.iter().filter(|p| !p.is_empty()).collect();
    let texts: HashMap<u32, String> = pages
        .iter()
        .flat_map(|p| p.slots.iter().flatten())
        .map(|s| {
            (
                s.id,
                plugin.display(s.id, plugin.value(s.id)).unwrap_or_default(),
            )
        })
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
                                value: plugin.value(s.id),
                                display: texts.get(&s.id).map_or("", String::as_str),
                            })
                        })
                        .collect(),
                })
                .collect()
        })
        .collect();
    let name = preset.plugin.name.clone().unwrap_or(product);
    client
        .publish_parameter_page(
            SERIAL,
            &ParameterPage {
                plugin_name: &name,
                pages: &layout,
                show_page: 0,
                background,
                plugin_color: Some(Rgb::DEFAULT),
                control_color: arc,
            },
        )
        .map_err(e)?;
    Ok(layout
        .iter()
        .map(|p| p.iter().map(|s| s.name).collect::<Vec<_>>().join(" | "))
        .collect())
}

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

/// Open the default output device and drive the plugin from its callback.
/// Inputs (notes, knob moves) reach the callback through `rx`.
fn open_output(audio: Audio, rx: Receiver<Input>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no audio output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!(
            "output wants {:?} samples; only f32 is handled",
            config.sample_format()
        ));
    }
    let channels = usize::from(config.channels());
    let mut pending: Vec<Input> = Vec::with_capacity(256);
    device
        .build_output_stream(
            config.config(),
            move |data: &mut [f32], _| {
                pending.clear();
                while let Ok(i) = rx.try_recv() {
                    pending.push(i);
                }
                if audio.render(data, channels, &pending).is_err() {
                    data.fill(0.0);
                }
            },
            |err| eprintln!("audio: {err}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// The output device's sample rate, so the plugin can be set up to match.
fn output_rate() -> Option<f64> {
    let device = cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    Some(f64::from(config.sample_rate()))
}

/// Translate one MIDI message from the keys into a plugin input.
fn note(bytes: &[u8]) -> Option<Input> {
    match *bytes {
        [status, pitch, velocity] if status & 0xF0 == 0x90 && velocity > 0 => {
            Some(Input::NoteOn { pitch, velocity })
        }
        [status, pitch, velocity] if status & 0xF0 == 0x90 || status & 0xF0 == 0x80 => {
            Some(Input::NoteOff { pitch, velocity })
        }
        _ => None,
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut path = args
        .iter()
        .find(|a| a.ends_with(".nksf"))
        .map(PathBuf::from)
        .ok_or("usage: kontrol <preset.nksf> [seconds]")?;
    let seconds: u64 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(300);
    let registry = std::env::var("TACTUS_ODR_REGISTRY")
        .map_err(|_| "Set TACTUS_ODR_REGISTRY to the symbol registry JSON.")?;
    let text = std::fs::read_to_string(&registry).map_err(|e| e.to_string())?;

    let mut preset = Preset::read(&path).map_err(|e| e.to_string())?;
    let uid = preset
        .plugin
        .vst3_uid
        .ok_or("this preset names no VST3 class")?;
    let name = preset
        .plugin
        .name
        .clone()
        .ok_or("this preset does not name its plugin")?;
    let bundle = PathBuf::from(format!("/Library/Audio/Plug-Ins/VST3/{name}.vst3"));
    let module = Module::load(&bundle).map_err(|e| e.to_string())?;
    let mut plugin = module
        .instantiate(&class_id(uid))
        .map_err(|e| e.to_string())?;
    let rate = output_rate().unwrap_or(48_000.0);
    plugin
        .start_audio(rate, MAX_BLOCK)
        .map_err(|e| e.to_string())?;
    let (tx, rx): (SyncSender<Input>, Receiver<Input>) = sync_channel(1024);
    let audio = plugin.audio().ok_or("engine not started")?;
    let stream = open_output(audio, rx);
    let mut clock = Clock {
        start: Instant::now(),
        blocks: 0,
        rate,
        driven_by_device: stream.is_ok(),
    };
    match &stream {
        Ok(s) => {
            s.play().map_err(|e| e.to_string())?;
            println!("audio: default output at {rate} Hz");
        }
        Err(err) => println!("audio: {err}; running silent"),
    }
    let keys = MidiIn::open(PortRole::Main, tactus_io::KONTROL_FILTER);
    match &keys {
        Ok(k) => println!("keys: {}", k.identity().display_name()),
        Err(err) => println!("keys: {err}"),
    }
    let mut played = false;
    let secs = load(&mut plugin, &mut clock, &preset)?;
    println!("{name}: {} loaded in {secs:.2}s", preset.info.name);

    let folder = siblings(&path);
    let mut client = Client::connect_with_registry(
        tactus_odr::DEFAULT_SOCKET,
        Registry::from_json(&text).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let e = |e: tactus_odr::OdrError| e.to_string();
    client
        .instance_hello("tactus-kontrol", "0.1.0")
        .map_err(e)?;
    client.connect_device(SERIAL).map_err(e)?;
    client.request_focus(SERIAL).map_err(e)?;
    if let Some(page) = client.registry().id("parameter") {
        client.set_page(SERIAL, page).map_err(e)?;
    }
    let mut headers = publish(&mut client, &plugin, &preset)?;
    println!("on screen. Knobs set the plugin; page and preset buttons work — {seconds}s.");

    let start = Instant::now();
    let mut last_focus = Instant::now();
    // Log each knob's first turn only, to show the knobs reach the plugin.
    let mut turned = std::collections::HashSet::new();
    let stamp = |t: Instant| format!("[{:6.2}s]", t.elapsed().as_secs_f32());
    while start.elapsed() < Duration::from_secs(seconds) {
        clock.tick(&mut plugin);
        if let Ok(keys) = &keys {
            while let Some(bytes) = keys.try_recv() {
                if let Some(input) = note(&bytes) {
                    if !played {
                        played = true;
                        println!("{} first note: {input:?}", stamp(start));
                    }
                    let _ = tx.try_send(input);
                }
            }
        }
        for n in client.listen(Duration::from_millis(5)).map_err(e)? {
            for event in events::decode(client.registry(), &n) {
                match event {
                    Event::KnobTurn { id, delta } => {
                        let v = (plugin.value(id) + delta).clamp(0.0, 1.0);
                        if plugin.set_value(id, v).is_ok() {
                            let v = plugin.value(id);
                            let _ = tx.try_send(Input::Param { id, value: v });
                            let text = plugin.display(id, v).unwrap_or_default();
                            client
                                .set_parameter_value(SERIAL, id, v, &text)
                                .map_err(e)?;
                            if turned.insert(id) {
                                println!("{} knob {id} turned: now {text}", stamp(start));
                            }
                        }
                    }
                    Event::ParameterPage(i) => println!(
                        "{} page {}: {}",
                        stamp(start),
                        i + 1,
                        headers.get(i as usize).map_or("?", String::as_str)
                    ),
                    Event::PresetStep(step) => {
                        let Some(here) = folder.iter().position(|p| *p == path) else {
                            continue;
                        };
                        let len = folder.len().max(1) as i64;
                        let next = (here as i64 + i64::from(step)).rem_euclid(len) as usize;
                        let Some(p) = folder.get(next).cloned() else {
                            continue;
                        };
                        match Preset::read(&p) {
                            Ok(np) if np.plugin.vst3_uid == preset.plugin.vst3_uid => {
                                match load(&mut plugin, &mut clock, &np) {
                                    Ok(secs) => {
                                        println!(
                                            "{} {} loaded in {secs:.2}s",
                                            stamp(start),
                                            np.info.name
                                        );
                                        path = p;
                                        preset = np;
                                        headers = publish(&mut client, &plugin, &preset)?;
                                    }
                                    Err(err) => println!("{} {}: {err}", stamp(start), p.display()),
                                }
                            }
                            Ok(_) => println!(
                                "{} {} is for another plugin; skipped",
                                stamp(start),
                                p.display()
                            ),
                            Err(err) => println!("{} {}: {err}", stamp(start), p.display()),
                        }
                    }
                    _ => {}
                }
            }
        }
        // The plugin can change its own values; show them when it says so.
        if plugin.take_restart_flags() & PARAM_VALUES_CHANGED != 0 {
            headers = publish(&mut client, &plugin, &preset)?;
        }
        if last_focus.elapsed() >= Duration::from_secs(1) {
            client.request_focus(SERIAL).map_err(e)?;
            pump_main_run_loop(Duration::from_millis(1));
            last_focus = Instant::now();
        }
    }
    Ok(())
}
