//! Pilot: talk to Native Instruments' ODR agent with no vendor application.
//!
//! ```text
//! cargo run -p tactus-odr --example pilot
//! ```
//!
//! Proves the whole stack end to end — framing, msgpack-RPC, the interned
//! symbol registry, the session quirks — by connecting to the agent and
//! reading live state out of the keyboard.
//!
//! The first half is read-only — enumerate devices, read properties and
//! settings. The second half **writes**: it identifies as a client instance,
//! asks for focus, and pushes a notification and a page selection.
//!
//! Note the difference between the agent accepting a call and the device
//! showing something. These calls return no error, which means the arguments
//! parsed; whether pixels change can only be settled by looking at the
//! hardware.

use std::time::Duration;

use rmpv::Value;
use tactus_odr::{Client, OdrError, Registry};

const TIMEOUT: Duration = Duration::from_secs(3);

fn show(label: &str, v: &Value) {
    let text = format!("{v}");
    if text.len() > 900 {
        println!("  {label}: {}…", &text[..900]);
    } else {
        println!("  {label}: {text}");
    }
}

fn main() -> Result<(), OdrError> {
    // The IPC greeting carries no symbol registry -- only the device-facing
    // endpoint sends one -- so the table has to come from the caller. Capture
    // it once as described in docs/protocol/mk3-odr.md.
    let registry = match std::env::var("TACTUS_ODR_REGISTRY") {
        Ok(path) => match std::fs::read_to_string(&path) {
            Ok(text) => Registry::from_json(&text)?,
            Err(e) => {
                eprintln!("cannot read registry at {path}: {e}");
                return Ok(());
            }
        },
        Err(_) => {
            eprintln!("Set TACTUS_ODR_REGISTRY to a JSON array of symbol names.");
            eprintln!("docs/protocol/mk3-odr.md explains how to capture it once.");
            return Ok(());
        }
    };

    println!("connecting to the ODR agent…");
    let mut client = match Client::connect_with_registry(tactus_odr::DEFAULT_SOCKET, registry) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("\nThe vendor's agent must be running. It starts with their software,");
            eprintln!("or directly from NIHardwareConnectionService.app.");
            return Err(e);
        }
    };

    let reg = client.registry();
    println!("connected. symbol registry: {} entries", reg.len());
    if reg.is_empty() {
        println!("  (empty — nothing to resolve method names against)");
    }

    if let Some(Value::Map(fields)) = client.greeting() {
        for (k, v) in fields {
            if matches!(
                k.as_str(),
                Some("agent_version") | Some("ipc_protocol_version")
            ) {
                show(k.as_str().unwrap_or("?"), v);
            }
        }
    }

    // ---- devices -----------------------------------------------------
    println!("\nget_devices:");
    let devices = client.call("get_devices", vec![], TIMEOUT)?;
    show("devices", &devices);

    // Pull the first serial out, so the rest of the run is about a real unit.
    let serial = match &devices {
        Value::Array(list) => list.iter().find_map(|d| match d {
            Value::Map(fields) => fields.iter().find_map(|(k, v)| {
                (k.as_str() == Some("serialnumber")).then(|| v.as_str().map(str::to_owned))?
            }),
            _ => None,
        }),
        _ => None,
    };
    let Some(serial) = serial else {
        println!("\nno keyboard attached — nothing further to read");
        return Ok(());
    };
    println!("\nusing serial {serial}");

    // Both of these take (serial, client_name). The client name is ours to
    // choose; the agent uses it to attribute the session.
    let args = || {
        vec![
            Value::String(serial.clone().into()),
            Value::String("tactus-pilot".into()),
        ]
    };

    println!("\nget_device_properties:");
    match client.call("get_device_properties", args(), TIMEOUT) {
        Ok(v) => show("properties", &v),
        Err(e) => println!("  {e}"),
    }

    println!("\nget_device_settings:");
    match client.call("get_device_settings", args(), TIMEOUT) {
        Ok(v) => show("settings", &v),
        Err(e) => println!("  {e}"),
    }

    // ---- identify, then drive the display ----------------------------
    //
    // Until instance_hello succeeds the agent serves only six
    // device-management methods. It is what turns on the display API.
    println!("\ninstance_hello (instance {}):", client.instance_id());
    match client.instance_hello("tactus-pilot", "2.2.0") {
        // Answered is not the same as registered; the agent can reply and
        // still not enter us into its instance manager.
        Ok(_) => println!("  answered (NOT proof of registration — see docs)"),
        Err(e) => {
            println!("  {e}");
            return Ok(());
        }
    }

    println!("\nclient_request_focus:");
    match client.request_focus(&serial) {
        Ok(()) => println!("  sent"),
        Err(e) => println!("  {e}"),
    }

    // The smallest host-supplied thing that can appear on the screen.
    println!("\nsend_global_notification \"hello from tactus\":");
    match client.notification(&serial, "hello from tactus") {
        Ok(()) => println!("  sent (the agent's own log is what says whether it worked)"),
        Err(e) => println!("  {e}"),
    }

    // Page indices are symbols: browser / mixer / parameter / smartplay.
    if let Some(page) = client.registry().id("browser") {
        println!("\nclient_set_page -> browser ({page}):");
        match client.set_page(&serial, page) {
            Ok(()) => println!("  sent (the agent's own log is what says whether it worked)"),
            Err(e) => println!("  {e}"),
        }
    }

    // ---- what the agent volunteers -----------------------------------
    println!("\nlistening 4s for notifications (touch the keyboard):");
    let notes = client.listen(Duration::from_secs(4))?;
    if notes.is_empty() {
        println!("  (none)");
    }
    for n in notes {
        let name = n.name.unwrap_or_else(|| format!("#{}", n.id));
        let params = client.registry().humanise(&n.params);
        println!("  {name}: {params}");
    }

    println!("\ndone");
    println!("\nThis run asked for focus and set a page, so it DID change device");
    println!("state. Reads alone are the earlier section; these last three calls");
    println!("are writes, and \"accepted without error\" means the agent took the");
    println!("call — not that anything appeared. Look at the keyboard to judge.");
    Ok(())
}
