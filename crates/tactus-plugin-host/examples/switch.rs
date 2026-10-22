//! Does the plugin's "values changed" signal on a preset *switch* mean the
//! new values are in place? Load one preset, then a second, and compare the
//! second's knob values at the signal against two seconds later.
//!
//! ```text
//! cargo run -p tactus-plugin-host --example switch -- <first.nksf> <second.nksf>
//! ```

use std::time::{Duration, Instant};

use tactus_nks::Preset;
use tactus_plugin_host::{Module, PARAM_VALUES_CHANGED, Plugin, class_id, pump_main_run_loop};

/// Process in real time until the plugin signals, or `limit` passes.
fn wait_for_signal(plugin: &mut Plugin<'_>, limit: Duration) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < limit {
        let _ = plugin.process_block(512);
        pump_main_run_loop(Duration::from_millis(10));
        if plugin.take_restart_flags() & PARAM_VALUES_CHANGED != 0 {
            return Some(t.elapsed());
        }
    }
    None
}

fn run_for(plugin: &mut Plugin<'_>, d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        let _ = plugin.process_block(512);
        pump_main_run_loop(Duration::from_millis(10));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [first, second] = args.as_slice() else {
        return Err("usage: switch <first.nksf> <second.nksf>".into());
    };
    let a = Preset::read(first)?;
    let b = Preset::read(second)?;
    let uid = a.plugin.vst3_uid.ok_or("no VST3 class")?;
    let name = a.plugin.name.clone().ok_or("no plugin name")?;
    let module = Module::load(format!("/Library/Audio/Plug-Ins/VST3/{name}.vst3").as_ref())?;
    let mut plugin = module.instantiate(&class_id(uid))?;
    plugin.start_audio(48_000.0, 512)?;

    plugin.take_restart_flags();
    plugin.load_state(&a.plugin_state)?;
    let t = wait_for_signal(&mut plugin, Duration::from_secs(5));
    println!("{}: signal after {t:?}", a.info.name);
    run_for(&mut plugin, Duration::from_secs(2));

    let knobs: Vec<_> = b
        .pages
        .iter()
        .flat_map(|p| p.slots.iter().flatten())
        .collect();
    plugin.take_restart_flags();
    plugin.load_state(&b.plugin_state)?;
    let t = wait_for_signal(&mut plugin, Duration::from_secs(5));
    let at_signal: Vec<f64> = knobs.iter().map(|s| plugin.value(s.id)).collect();
    run_for(&mut plugin, Duration::from_secs(2));
    let settled: Vec<f64> = knobs.iter().map(|s| plugin.value(s.id)).collect();
    println!("{}: signal after {t:?}", b.info.name);
    let differ = at_signal
        .iter()
        .zip(&settled)
        .filter(|(x, y)| (**x - **y).abs() > 1e-6)
        .count();
    println!("{differ} of {} knobs moved after the signal", knobs.len());
    for (s, (x, y)) in knobs.iter().zip(at_signal.iter().zip(&settled)).take(8) {
        println!("  {:<12} at signal {x:.3}  settled {y:.3}", s.name);
    }
    Ok(())
}
