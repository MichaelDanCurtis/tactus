//! Drive the transport button LEDs, then listen for transport presses.
//!
//! ```text
//! TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
//!   cargo run -p tactus-odr --example transport
//! ```

use std::time::{Duration, Instant};

use tactus_odr::{Client, Control, HostState, Knob, OdrError, Record, Registry, Transport};

fn hold(client: &mut Client, serial: &str, st: &HostState<'_>, secs: u64) -> Result<(), OdrError> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        client.request_focus(serial)?;
        client.set_host_state(serial, st)?;
        for n in client.listen(Duration::from_millis(500))? {
            let name = n.name.unwrap_or_else(|| format!("#{}", n.id));
            // Print everything except the focus bookkeeping. An earlier version
            // printed only `plugin_action`, which would silently drop any
            // control that reports on a different channel.
            if matches!(
                name.as_str(),
                "focus_changed"
                    | "active_instance_client_changed"
                    | "device_owned_instance_client_state_changed"
            ) {
                continue;
            }
            println!("      {name}: {}", client.registry().humanise(&n.params));
        }
    }
    Ok(())
}

fn main() -> Result<(), OdrError> {
    let Ok(path) = std::env::var("TACTUS_ODR_REGISTRY") else {
        eprintln!("Set TACTUS_ODR_REGISTRY to the symbol registry JSON.");
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| OdrError::Io {
        op: "read registry",
        source,
    })?;
    let mut client =
        Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, Registry::from_json(&text)?)?;
    let serial = "99FE7169";
    client.instance_hello("tactus-transport", "0.1.0")?;
    println!("connect_device: {}", client.connect_device(serial)?);
    client.request_focus(serial)?;
    let knobs = [Knob {
        id: 0,
        name: "Cutoff",
        value: 0.5,
        display: "50%",
    }];
    client.set_parameter_page(serial, "TACTUS", "Transport test", &knobs)?;

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
        track_name: "TACTUS",
        preset_name: "Transport test",
    };
    let steps: Vec<(&str, HostState<'_>)> = vec![
        ("1  everything off", base),
        (
            "2  PLAY",
            HostState {
                transport: Transport::Playing,
                ..base
            },
        ),
        (
            "3  PLAY + METRO + LOOP",
            HostState {
                transport: Transport::Playing,
                metro: Control::On,
                loop_: Control::On,
                ..base
            },
        ),
        (
            "4  RECORD armed",
            HostState {
                record: Record::Armed,
                ..base
            },
        ),
        (
            "5  RECORDING",
            HostState {
                transport: Transport::Playing,
                record: Record::Recording,
                ..base
            },
        ),
        (
            "6  UNDO + REDO + QUANTIZE available",
            HostState {
                can_undo: true,
                can_redo: true,
                can_quantize: true,
                ..base
            },
        ),
        (
            "7  everything DISABLED",
            HostState {
                metro: Control::Disabled,
                loop_: Control::Disabled,
                tap_tempo: Control::Disabled,
                count_in: Control::Disabled,
                ..base
            },
        ),
    ];
    println!("\nWatch the transport buttons. Each step holds 4s.\n");
    for (label, st) in &steps {
        println!("  {label}");
        hold(&mut client, serial, st, 4)?;
    }

    println!(
        "\nNow PRESS the transport buttons yourself (Play, Stop, Rec, Loop, Metro, Undo...) — 45s. Leave the knobs alone.\n"
    );
    let idle = HostState {
        can_undo: true,
        can_redo: true,
        can_quantize: true,
        ..base
    };
    hold(&mut client, serial, &idle, 45)?;
    println!("\ndone");
    Ok(())
}
