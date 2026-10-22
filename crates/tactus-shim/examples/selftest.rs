//! Goal item 1: tactus's own host loads the shim, the shim loads an
//! instrument, and it plays. The shim does all the keyboard work itself.
//!
//! ```text
//! tools/bundle-shim.sh debug
//! TACTUS_SHIM_PRESET="/Library/Application Support/Native Instruments/Massive X/Presets/Wob Wolfer.nksf" \
//!   cargo run -p tactus-shim --example selftest -- [offline|live] [seconds]
//! ```
//!
//! `offline` plays middle C through both layers and reports the level.
//! `live` plays from the keyboard through the default output.

use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tactus_io::{MidiIn, PortRole};
use tactus_plugin_host::{Input, MidiMap, Module, midi_to_input, pump_main_run_loop};
use tactus_shim::FRAME_CID;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let live = args.iter().any(|a| a == "live");
    let seconds: u64 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(300);
    let bundle = Path::new("target/debug/Tactus Shim.vst3");
    let module = Module::load(bundle)?;
    let names: Vec<String> = module
        .classes()?
        .into_iter()
        .map(|c| format!("{} ({})", c.name, c.category))
        .collect();
    println!("shim classes: {}", names.join(", "));
    let cid = FRAME_CID.map(|b| b as u8);
    let mut shim = module.instantiate(&cid)?;
    println!(
        "shim params: {} (the instrument's, forwarded)",
        shim.parameters().len()
    );

    let rate = if live {
        cpal::default_host()
            .default_output_device()
            .and_then(|d| d.default_output_config().ok())
            .map_or(48_000.0, |c| f64::from(c.sample_rate()))
    } else {
        48_000.0
    };
    shim.start_audio(rate, 4096)?;
    let mut audio = shim.take_audio().ok_or("no engine")?;

    if !live {
        // Let the instrument apply its preset, servicing the shim's timer.
        let mut block = vec![0.0f32; 512 * 2];
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            audio.render(&mut block, 2, &[]);
            pump_main_run_loop(Duration::from_millis(10));
        }
        let mut held = 0.0f32;
        let mut tail = 0.0f32;
        for i in 0..188 {
            let input = match i {
                0 => vec![Input::NoteOn {
                    pitch: 60,
                    velocity: 100,
                }],
                94 => vec![Input::NoteOff {
                    pitch: 60,
                    velocity: 64,
                }],
                _ => vec![],
            };
            let peak = audio.render(&mut block, 2, &input);
            if i < 94 {
                held = held.max(peak)
            } else {
                tail = tail.max(peak)
            }
            pump_main_run_loop(Duration::from_millis(1));
        }
        println!("through the shim: middle C held peak {held:.4}, release peak {tail:.4}");
        let state = shim.save_state()?;
        println!(
            "shim state: {} bytes, starts {:?}",
            state.len(),
            state.get(..8).map(String::from_utf8_lossy)
        );
        shim.put_audio(audio);

        // Reopen: a fresh shim with no default preset, given the saved state,
        // as a DAW does when it loads a project.
        drop(shim);
        // SAFETY: single-threaded here; the shim reads this at initialize.
        unsafe { std::env::remove_var("TACTUS_SHIM_PRESET") };
        let mut again = module.instantiate(&cid)?;
        let before = again.parameters().len();
        again.load_state(&state)?;
        again.start_audio(48_000.0, 4096)?;
        let mut audio = again.take_audio().ok_or("no engine")?;
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            audio.render(&mut block, 2, &[]);
            pump_main_run_loop(Duration::from_millis(10));
        }
        let mut peak = 0.0f32;
        for i in 0..94 {
            let input = if i == 0 {
                vec![Input::NoteOn {
                    pitch: 60,
                    velocity: 100,
                }]
            } else {
                vec![]
            };
            peak = peak.max(audio.render(&mut block, 2, &input));
        }
        println!(
            "restored: params {before} before state, {} after; middle C peak {peak:.4}",
            again.parameters().len()
        );
        again.put_audio(audio);
        return Ok(());
    }

    // Live: the output device's callback drives the shim; keys go in as notes.
    let (tx, rx) = sync_channel::<Input>(1024);
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no output device")?;
    let config = device.default_output_config()?;
    let channels = usize::from(config.channels());
    let mut sc = config.config();
    if let cpal::SupportedBufferSize::Range { min, max } = *config.buffer_size() {
        sc.buffer_size = cpal::BufferSize::Fixed(256u32.clamp(min, max));
    }
    let mut pending = Vec::with_capacity(256);
    let stream = device.build_output_stream(
        sc,
        move |data: &mut [f32], _| {
            pending.clear();
            while let Ok(i) = rx.try_recv() {
                pending.push(i);
            }
            audio.render(data, channels, &pending);
        },
        |e| eprintln!("audio: {e}"),
        None,
    )?;
    stream.play()?;
    let keys = MidiIn::open(PortRole::Main, tactus_io::KONTROL_FILTER)?;
    println!(
        "live: keys {} -> host -> shim -> instrument, {rate} Hz — {seconds}s",
        keys.identity().display_name()
    );
    std::thread::spawn(move || {
        // Notes go to the shim as events; it forwards them. Controllers the
        // shim maps reach it as parameters through the DAW path, which this
        // test does not emulate, so only notes and pressure are sent.
        let none = MidiMap::new();
        loop {
            if let Some(bytes) = keys.recv_timeout(Duration::from_secs(1))
                && let Some(i) = midi_to_input(&bytes, &none)
            {
                let _ = tx.try_send(i);
            }
        }
    });
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        pump_main_run_loop(Duration::from_millis(5));
    }
    drop(stream);
    Ok(())
}
