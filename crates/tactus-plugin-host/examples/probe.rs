//! Load the plugin an NKS preset belongs to, give it the preset's state, and
//! print each NKS knob beside the plugin's own value and text for it.
//!
//! ```text
//! cargo run -p tactus-plugin-host --example probe -- <preset.nksf> [bundle.vst3]
//! ```
//!
//! No audio and no keyboard: this checks that the plugin loads, accepts the
//! preset's state, and that NKS knob ids are the plugin's parameter ids.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use tactus_nks::Preset;
use tactus_plugin_host::{AUDIO_MODULE_CLASS, Module, class_id, pump_main_run_loop};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let preset_path = args
        .next()
        .ok_or("usage: probe <preset.nksf> [bundle.vst3]")?;
    let preset = Preset::read(&preset_path)?;
    let uid = preset
        .plugin
        .vst3_uid
        .ok_or("this preset names no VST3 class (VST2-only plugin)")?;
    let name = preset
        .plugin
        .name
        .clone()
        .ok_or("this preset does not name its plugin")?;
    let bundle = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/Library/Audio/Plug-Ins/VST3/{name}.vst3")));
    println!(
        "preset  {} ({} bytes of state)",
        preset.info.name,
        preset.plugin_state.len()
    );
    println!("bundle  {}", bundle.display());

    let t = Instant::now();
    let module = Module::load(&bundle)?;
    println!("loaded  in {:.2}s", t.elapsed().as_secs_f32());
    let cid = class_id(uid);
    for c in module.classes()? {
        let mark = if c.cid == cid {
            "  <- the preset's class"
        } else {
            ""
        };
        println!("class   {:<24} {}{mark}", c.category, c.name);
    }
    if !module
        .classes()?
        .iter()
        .any(|c| c.cid == cid && c.category == AUDIO_MODULE_CLASS)
    {
        return Err("the preset's class is not an audio class in this bundle".into());
    }

    let t = Instant::now();
    let mut plugin = module.instantiate(&cid)?;
    pump_main_run_loop(Duration::from_millis(200));
    println!("created in {:.2}s", t.elapsed().as_secs_f32());

    let params = plugin.parameters();
    println!("params  {}", params.len());
    let by_id: HashMap<u32, &tactus_plugin_host::Param> =
        params.iter().map(|p| (p.id, p)).collect();

    // Values before and after loading the preset, for the first page's knobs.
    let first: Vec<_> = preset
        .pages
        .iter()
        .flat_map(|p| p.slots.iter().flatten())
        .take(8)
        .collect();
    let before: Vec<f64> = first.iter().map(|s| plugin.value(s.id)).collect();
    println!(
        "split   controller is a separate object: {}",
        plugin.separate_controller()
    );
    let snapshot = |plugin: &tactus_plugin_host::Plugin<'_>| -> Vec<f64> {
        params.iter().map(|p| plugin.value(p.id)).collect()
    };
    let all_before = snapshot(&plugin);
    if std::env::var_os("PROBE_NO_AUDIO").is_none() {
        plugin.start_audio(48_000.0, 512)?;
        println!("engine  started at 48 kHz, 512-frame blocks");
    }
    let t = Instant::now();
    plugin.load_state(&preset.plugin_state)?;
    let mut peak = 0.0f32;
    if std::env::var_os("PROBE_NO_AUDIO").is_none() {
        for _ in 0..50 {
            peak = peak.max(plugin.process_block(512)?);
        }
        println!("engine  ran 50 blocks after the state, output peak {peak:.4}");
    }
    pump_main_run_loop(Duration::from_millis(200));
    println!("state   loaded in {:.2}s", t.elapsed().as_secs_f32());
    let changed = |now: &[f64]| {
        all_before
            .iter()
            .zip(now)
            .filter(|(a, b)| (*a - *b).abs() > 1e-9)
            .count()
    };
    println!(
        "changed {} of {} parameters right after setState",
        changed(&snapshot(&plugin)),
        params.len()
    );
    pump_main_run_loop(Duration::from_secs(2));
    println!(
        "changed {} of {} after 2s more on the run loop",
        changed(&snapshot(&plugin)),
        params.len()
    );
    if let Some(secs) = std::env::var("PROBE_WAIT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        // Real time: one 512-frame block every ~10.7 ms, run loop serviced.
        let start = Instant::now();
        let mut next_report = 1;
        while start.elapsed() < Duration::from_secs(secs) {
            if plugin.start_audio(48_000.0, 512).is_ok() {
                let _ = plugin.process_block(512);
            }
            pump_main_run_loop(Duration::from_millis(10));
            if start.elapsed().as_secs() >= next_report {
                let saved = plugin.save_state().map(|v| v.len()).unwrap_or(0);
                println!(
                    "wait    {next_report:>2}s: {} changed, getState {saved} bytes, restart flags {:#x}",
                    changed(&snapshot(&plugin)),
                    plugin.take_restart_flags()
                );
                next_report += 1;
            }
        }
    }
    let saved = plugin.save_state()?;
    println!(
        "getState gives {} bytes (preset had {}); identical: {}\n",
        saved.len(),
        preset.plugin_state.len(),
        saved == preset.plugin_state
    );
    if std::env::var_os("PROBE_FRAMING").is_some() {
        // Packaging only: do both blobs open the same way?
        let head = |b: &[u8]| {
            b.iter()
                .take(32)
                .map(|x| format!("{x:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("framing preset : {}", head(&preset.plugin_state));
        println!("framing fresh  : {}\n", head(&saved));
    }

    println!(
        "{:<14} {:>5}  {:<24} {:>7} {:>7}  text",
        "NKS knob", "id", "plugin parameter", "before", "after"
    );
    let mut matched = 0;
    for (slot, was) in first.iter().zip(before) {
        let now = plugin.value(slot.id);
        let (title, text) = match by_id.get(&slot.id) {
            Some(p) => {
                matched += 1;
                (
                    p.title.as_str(),
                    plugin.display(slot.id, now).unwrap_or_default(),
                )
            }
            None => ("-- no such parameter --", String::new()),
        };
        println!(
            "{:<14} {:>5}  {:<24} {:>7.3} {:>7.3}  {text}",
            slot.name, slot.id, title, was, now
        );
    }
    let total = preset
        .pages
        .iter()
        .flat_map(|p| p.slots.iter().flatten())
        .count();
    let all = preset
        .pages
        .iter()
        .flat_map(|p| p.slots.iter().flatten())
        .filter(|s| by_id.contains_key(&s.id))
        .count();
    println!(
        "\nfirst page: {matched}/{} knob ids are plugin parameters; whole preset: {all}/{total}",
        first.len()
    );
    Ok(())
}
