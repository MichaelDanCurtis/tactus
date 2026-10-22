//! The keyboard side: one thread that owns the connection to the agent.
//!
//! It never touches the plugin. The frame sends it [`Command`]s; it sends
//! back the device's [`Event`]s. Network waits happen here, so neither the
//! DAW's main thread nor its audio thread ever waits on the agent.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tactus_odr::events::{self, Event};
use tactus_odr::{
    Client, Control, HostState, Knob, KnobSection, ParameterPage, Record, Registry, Rgb, Transport,
};

/// One knob, owned, for sending across threads.
#[derive(Debug, Clone)]
pub struct KnobData {
    /// Parameter id.
    pub id: u32,
    /// Label.
    pub name: String,
    /// Normalised value.
    pub value: f64,
    /// The plugin's text for the value.
    pub display: String,
}

/// A whole screen: the plugin's name, its pages, its banner and colour.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    /// The name across the top.
    pub plugin_name: String,
    /// Pages of (section name, knob slots).
    pub pages: Vec<Vec<(String, Vec<Option<KnobData>>)>>,
    /// The product banner, PNG or WebP bytes.
    pub banner: Option<Vec<u8>>,
    /// Knob arc colour.
    pub arc: Option<Rgb>,
    /// The preset name for the host state.
    pub preset_name: String,
}

/// What the frame asks of the keyboard.
#[derive(Debug, Clone)]
pub enum Command {
    /// Take the keyboard (if not already ours) and show this screen.
    Show(Box<Screen>),
    /// One knob's new value and text.
    Value {
        /// Parameter id.
        id: u32,
        /// Normalised value.
        value: f64,
        /// Text.
        display: String,
    },
    /// Light-guide colours for every key that changed.
    Leds(Vec<(u8, u8)>),
    /// The DAW's transport, for the transport lights.
    Transport {
        /// Playing or not.
        playing: bool,
        /// Tempo in BPM.
        tempo: f64,
    },
    /// Take the keyboard back from another instance.
    TakeFocus,
    /// Stop the thread.
    Quit,
}

/// The running keyboard thread.
pub struct Keyboard {
    /// Commands in.
    pub commands: Sender<Command>,
    /// Device events out.
    pub events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Keyboard {
    /// Start the thread. It connects in the background; until it does,
    /// commands queue up.
    pub fn start(registry: PathBuf, client_name: String) -> Self {
        let (commands, cmd_rx) = channel();
        let (ev_tx, events) = channel();
        let thread = std::thread::Builder::new()
            .name("tactus-keyboard".into())
            .spawn(move || {
                if let Err(e) = run(&registry, &client_name, &cmd_rx, &ev_tx) {
                    eprintln!("tactus-shim keyboard: {e}");
                }
            })
            .ok();
        Self {
            commands,
            events,
            thread,
        }
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn host_state<'a>(playing: bool, tempo: f64, preset: &'a str) -> HostState<'a> {
    HostState {
        transport: if playing {
            Transport::Playing
        } else {
            Transport::Stopped
        },
        record: Record::Off,
        metro: Control::Disabled,
        loop_: Control::Disabled,
        tap_tempo: Control::Disabled,
        count_in: Control::Disabled,
        can_undo: false,
        can_redo: false,
        can_quantize: false,
        can_quantize_half: false,
        tempo,
        track_name: "",
        preset_name: preset,
    }
}

fn publish(client: &mut Client, serial: &str, screen: &Screen) -> Result<(), String> {
    let e = |e: tactus_odr::OdrError| e.to_string();
    let background = match &screen.banner {
        Some(img) => Some(client.add_asset(img).map_err(e)?),
        None => None,
    };
    let pages: Vec<Vec<KnobSection<'_>>> = screen
        .pages
        .iter()
        .map(|page| {
            page.iter()
                .map(|(name, knobs)| KnobSection {
                    name,
                    knobs: knobs
                        .iter()
                        .map(|k| {
                            k.as_ref().map(|k| Knob {
                                id: k.id,
                                name: &k.name,
                                value: k.value,
                                display: &k.display,
                            })
                        })
                        .collect(),
                })
                .collect()
        })
        .collect();
    client
        .publish_parameter_page(
            serial,
            &ParameterPage {
                plugin_name: &screen.plugin_name,
                pages: &pages,
                show_page: 0,
                background,
                plugin_color: Some(Rgb::DEFAULT),
                control_color: screen.arc,
            },
        )
        .map_err(e)
}

fn run(
    registry: &std::path::Path,
    client_name: &str,
    commands: &Receiver<Command>,
    events_out: &Sender<Event>,
) -> Result<(), String> {
    let e = |e: tactus_odr::OdrError| e.to_string();
    let text = std::fs::read_to_string(registry)
        .map_err(|err| format!("{}: {err}", registry.display()))?;
    let mut client = Client::connect_with_registry(
        tactus_odr::DEFAULT_SOCKET,
        Registry::from_json(&text).map_err(e)?,
    )
    .map_err(e)?;
    client
        .instance_hello(client_name, env!("CARGO_PKG_VERSION"))
        .map_err(e)?;
    let device = client
        .devices()
        .map_err(e)?
        .into_iter()
        .next()
        .ok_or("no keyboard connected")?;
    let serial = device.serial;
    client.connect_device(&serial).map_err(e)?;
    let me = client.instance_id().to_owned();

    let mut focused = false;
    let mut screen: Option<Box<Screen>> = None;
    let mut transport = (false, 120.0);
    let mut last_focus = Instant::now();
    let take = |client: &mut Client,
                screen: &Option<Box<Screen>>,
                transport: (bool, f64)|
     -> Result<(), String> {
        client.request_focus(&serial).map_err(e)?;
        if let Some(page) = client.registry().id("parameter") {
            client.set_page(&serial, page).map_err(e)?;
        }
        if let Some(s) = screen {
            let preset = s.preset_name.clone();
            client
                .set_host_state(&serial, &host_state(transport.0, transport.1, &preset))
                .map_err(e)?;
            publish(client, &serial, s)?;
        }
        Ok(())
    };

    loop {
        for n in client.listen(Duration::from_millis(5)).map_err(e)? {
            for ev in events::decode(client.registry(), &n) {
                if let Event::Focus { owner } = &ev {
                    focused = *owner == me;
                }
                if events_out.send(ev).is_err() {
                    return Ok(());
                }
            }
        }
        loop {
            match commands.try_recv() {
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => break,
                Ok(Command::Show(s)) => {
                    screen = Some(s);
                    take(&mut client, &screen, transport)?;
                    focused = true;
                }
                Ok(Command::TakeFocus) => {
                    take(&mut client, &screen, transport)?;
                    focused = true;
                }
                Ok(Command::Value { id, value, display }) => {
                    if focused {
                        client
                            .set_parameter_value(&serial, id, value, &display)
                            .map_err(e)?;
                    }
                }
                Ok(Command::Leds(leds)) => {
                    if focused {
                        client.set_leds(&serial, &leds).map_err(e)?;
                    }
                }
                Ok(Command::Transport { playing, tempo }) => {
                    transport = (playing, tempo);
                    if focused {
                        let preset = screen
                            .as_ref()
                            .map(|s| s.preset_name.clone())
                            .unwrap_or_default();
                        client
                            .set_host_state(&serial, &host_state(playing, tempo, &preset))
                            .map_err(e)?;
                    }
                }
            }
        }
        // Focus lapses when idle; hold it while it is ours.
        if focused && last_focus.elapsed() >= Duration::from_secs(1) {
            client.request_focus(&serial).map_err(e)?;
            last_focus = Instant::now();
        }
    }
}
