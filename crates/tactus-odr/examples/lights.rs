//! Step through the transport LED states one change at a time, paced by the
//! person watching: preset **down** goes to the next step, preset **up** goes
//! back. The screen names the step and what changed.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example lights -- [minutes] [tempo]
//! ```
//!
//! `tempo` runs the blink set instead: the same lights with the transport
//! playing, at two tempos, to see whether the keyboard blinks them itself.
//! Its last step has the host blink Tempo, the fallback if it does not.

use std::time::{Duration, Instant};

use tactus_odr::events::{self, Event};
use tactus_odr::{Client, Control, HostState, Knob, OdrError, Record, Registry, Transport};

const SERIAL: &str = "99FE7169";

fn main() -> Result<(), OdrError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let minutes: u64 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(10);
    let tempo_set = args.iter().any(|a| a == "tempo");
    let Ok(path) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;

    // Every step but the last two changes exactly one thing from `base`.
    let base = HostState {
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
        preset_name: "Light check",
    };
    let plain: Vec<(&str, HostState<'_>)> = vec![
        ("Baseline: all off", base),
        (
            "PLAY: playing",
            HostState {
                transport: Transport::Playing,
                ..base
            },
        ),
        (
            "PLAY: paused",
            HostState {
                transport: Transport::Paused,
                ..base
            },
        ),
        (
            "REC: armed",
            HostState {
                record: Record::Armed,
                ..base
            },
        ),
        (
            "REC: recording",
            HostState {
                transport: Transport::Playing,
                record: Record::Recording,
                ..base
            },
        ),
        (
            "LOOP: on",
            HostState {
                loop_: Control::On,
                ..base
            },
        ),
        (
            "METRO: on",
            HostState {
                metro: Control::On,
                ..base
            },
        ),
        (
            "TEMPO: on",
            HostState {
                tap_tempo: Control::On,
                ..base
            },
        ),
        (
            "COUNT-IN: on",
            HostState {
                count_in: Control::On,
                ..base
            },
        ),
        (
            "UNDO: available",
            HostState {
                can_undo: true,
                ..base
            },
        ),
        (
            "REDO: available",
            HostState {
                can_redo: true,
                ..base
            },
        ),
        (
            "QUANTIZE: available",
            HostState {
                can_quantize: true,
                ..base
            },
        ),
        (
            "All DISABLED",
            HostState {
                metro: Control::Disabled,
                loop_: Control::Disabled,
                tap_tempo: Control::Disabled,
                count_in: Control::Disabled,
                ..base
            },
        ),
        (
            "Everything ON",
            HostState {
                transport: Transport::Playing,
                record: Record::Recording,
                metro: Control::On,
                loop_: Control::On,
                tap_tempo: Control::On,
                count_in: Control::On,
                can_undo: true,
                can_redo: true,
                can_quantize: true,
                can_quantize_half: true,
                ..base
            },
        ),
    ];

    let playing = HostState {
        transport: Transport::Playing,
        ..base
    };
    let slow = HostState {
        tempo: 60.0,
        ..playing
    };
    let blink: Vec<(&str, HostState<'_>)> = vec![
        ("Playing, all off", playing),
        (
            "Playing, TEMPO on, 120",
            HostState {
                tap_tempo: Control::On,
                ..playing
            },
        ),
        (
            "Playing, TEMPO on, 60",
            HostState {
                tap_tempo: Control::On,
                ..slow
            },
        ),
        (
            "Playing, METRO on, 120",
            HostState {
                metro: Control::On,
                ..playing
            },
        ),
        (
            "Playing, METRO on, 60",
            HostState {
                metro: Control::On,
                ..slow
            },
        ),
        (
            "Playing, COUNT-IN + REC armed",
            HostState {
                count_in: Control::On,
                record: Record::Armed,
                ..playing
            },
        ),
        // The host toggles Tempo on each half beat; see the loop below.
        ("HOST blinks TEMPO, 120", playing),
    ];
    let host_blinks = |label: &str| label.starts_with("HOST blinks");
    let steps = if tempo_set { blink } else { plain };

    let mut client =
        Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
    client.instance_hello("tactus-lights", "0.1.0")?;
    println!("connect_device: {}", client.connect_device(SERIAL)?);
    client.request_focus(SERIAL)?;
    if let Some(page) = client.registry().id("parameter") {
        client.set_page(SERIAL, page)?;
    }

    let show = |client: &mut Client, i: usize| -> Result<(), OdrError> {
        let Some((label, st)) = steps.get(i) else {
            return Ok(());
        };
        let title = format!("Light check {}/{}", i + 1, steps.len());
        let knob = [Knob {
            id: 0,
            name: "Down = next",
            value: 0.0,
            display: "Up = back",
        }];
        client.set_parameter_page(SERIAL, &title, label, &knob)?;
        client.set_host_state(SERIAL, st)
    };

    let start = Instant::now();
    let stamp = |t: Instant| format!("[{:6.2}s]", t.elapsed().as_secs_f32());
    let mut step = 0usize;
    show(&mut client, step)?;
    println!("{} step {:>2}: {}", stamp(start), step + 1, steps[0].0);

    let mut last = Instant::now();
    let mut lit = Control::Off;
    while start.elapsed() < Duration::from_secs(minutes * 60) {
        for n in client.listen(Duration::from_millis(50))? {
            for event in events::decode(client.registry(), &n) {
                match event {
                    Event::PresetStep(d) => {
                        let next = step.saturating_add_signed(d as isize).min(steps.len() - 1);
                        if next != step {
                            step = next;
                            show(&mut client, step)?;
                            let label = steps.get(step).map_or("", |s| s.0);
                            println!("{} step {:>2}: {label}", stamp(start), step + 1);
                        }
                    }
                    Event::Transport(t) => println!("{}   pressed {t:?}", stamp(start)),
                    Event::Focus { .. } | Event::Page { .. } | Event::KnobTouch { .. } => {}
                    Event::Other { ref name, .. } if name == "active_instance_client_changed" => {}
                    other => println!("{}   {other:?}", stamp(start)),
                }
            }
        }
        let Some(&(label, st)) = steps.get(step) else {
            continue;
        };
        if host_blinks(label) {
            // On for the first half of each beat, off for the second.
            let beat = 60.0 / st.tempo;
            let phase = start.elapsed().as_secs_f64() % beat;
            let tap_tempo = if phase < beat / 2.0 {
                Control::On
            } else {
                Control::Off
            };
            if tap_tempo != lit {
                lit = tap_tempo;
                client.set_host_state(SERIAL, &HostState { tap_tempo, ..st })?;
            }
        }
        // Keep focus, and keep re-asserting the state in case the device
        // drops it.
        if last.elapsed() >= Duration::from_millis(500) {
            client.request_focus(SERIAL)?;
            if !host_blinks(label) {
                client.set_host_state(SERIAL, &st)?;
            }
            last = Instant::now();
        }
    }
    Ok(())
}
