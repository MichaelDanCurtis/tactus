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
    if args.iter().any(|a| a == "two") {
        return two_shims();
    }
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

    // Goal item 5: the shim's window is the instrument's own. Ask the shim
    // for its editor, as a DAW does, and see whose view comes back.
    {
        use tactus_plugin_host::vst3::ComPtr;
        use tactus_plugin_host::vst3::Steinberg::Vst::IEditControllerTrait;
        use tactus_plugin_host::vst3::Steinberg::{IPlugView, IPlugViewTrait, ViewRect, kResultOk};
        // SAFETY: plain VST3 calls on a live controller.
        unsafe {
            let raw = shim.edit_controller().createView(c"editor".as_ptr());
            match ComPtr::<IPlugView>::from_raw(raw) {
                Some(view) => {
                    let nsview = view.isPlatformTypeSupported(c"NSView".as_ptr()) == kResultOk;
                    let mut r = ViewRect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    let sized = view.getSize(&mut r) == kResultOk;
                    println!(
                        "shim editor: a view, NSView supported: {nsview}, size {}x{} ({})",
                        r.right - r.left,
                        r.bottom - r.top,
                        if sized {
                            "from the instrument"
                        } else {
                            "unknown"
                        }
                    );
                }
                None => println!("shim editor: none"),
            }
        }
    }

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
        // An empty path overrides shim.conf: the fresh shim starts empty.
        unsafe { std::env::set_var("TACTUS_SHIM_PRESET", "") };
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

/// Goal item 4: two shims, one keyboard. Play only the second; it should take
/// the keyboard from the first. Run with TACTUS_SHIM_DEBUG=1 to see focus.
fn two_shims() -> Result<(), Box<dyn std::error::Error>> {
    let module = Module::load(Path::new("target/debug/Tactus Shim.vst3"))?;
    let cid = FRAME_CID.map(|b| b as u8);
    let mut a = module.instantiate(&cid)?;
    let pump = |d: Duration| {
        let t = Instant::now();
        while t.elapsed() < d {
            pump_main_run_loop(Duration::from_millis(10));
        }
    };
    pump(Duration::from_secs(3));
    let mut b = module.instantiate(&cid)?;
    pump(Duration::from_secs(3));
    a.start_audio(48_000.0, 4096)?;
    b.start_audio(48_000.0, 4096)?;
    let mut audio_a = a.take_audio().ok_or("no engine")?;
    let mut audio_b = b.take_audio().ok_or("no engine")?;
    let mut block = vec![0.0f32; 512 * 2];
    let run = |a: &mut tactus_plugin_host::Audio,
               b: &mut tactus_plugin_host::Audio,
               block: &mut [f32],
               to_b: &[Input],
               secs: u64| {
        let t = Instant::now();
        let mut first = true;
        while t.elapsed() < Duration::from_secs(secs) {
            a.render(block, 2, &[]);
            b.render(block, 2, if first { to_b } else { &[] });
            first = false;
            pump_main_run_loop(Duration::from_millis(10));
        }
    };
    println!("both shims up; the second was shown last");
    run(&mut audio_a, &mut audio_b, &mut block, &[], 2);
    println!("now playing the FIRST shim only");
    run(
        &mut audio_b,
        &mut audio_a,
        &mut block,
        &[Input::NoteOn {
            pitch: 60,
            velocity: 100,
        }],
        2,
    );
    run(
        &mut audio_b,
        &mut audio_a,
        &mut block,
        &[Input::NoteOff {
            pitch: 60,
            velocity: 0,
        }],
        1,
    );
    println!("now playing the SECOND shim only");
    run(
        &mut audio_a,
        &mut audio_b,
        &mut block,
        &[Input::NoteOn {
            pitch: 64,
            velocity: 100,
        }],
        2,
    );
    run(
        &mut audio_a,
        &mut audio_b,
        &mut block,
        &[Input::NoteOff {
            pitch: 64,
            velocity: 0,
        }],
        1,
    );
    run(&mut audio_a, &mut audio_b, &mut block, &[], 2);
    println!("(two idle seconds passed; tearing down)");
    a.put_audio(audio_a);
    b.put_audio(audio_b);
    Ok(())
}
