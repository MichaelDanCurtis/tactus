//! Client for Native Instruments' ODR agent.
//!
//! ODR is the channel the vendor's own software uses to drive Kontrol MK3
//! hardware. It matters because **the MK3's display is unreachable without
//! it**: in MIDI mode the keyboard renders its own assignment list and a host
//! cannot put anything on screen. See `docs/protocol/mk3-odr.md`.
//!
//! This crate speaks to the agent that ships with the vendor's software, over
//! its local socket. That is a deliberate first step rather than the end
//! state: talking to the agent proves the protocol end to end without first
//! having to claim USB interface 3 and reimplement the transport.
//!
//! # Wire format
//!
//! Every frame is a little-endian `u32` byte count followed by a msgpack
//! value, and the values are msgpack-RPC:
//!
//! | Form | Shape |
//! |---|---|
//! | request | `[0, msgid, method, params]` |
//! | response | `[1, msgid, error, result]` |
//! | notification | `[2, method, params]` |
//!
//! **Methods are integers, not strings** — indices into a symbol registry the
//! agent sends in its greeting. Map keys are interned the same way, so a
//! payload arrives as `{"218": 220}` and means `{page: browser}`. [`Registry`]
//! exists to make that readable.
//!
//! # The confirmed sequence
//!
//! Verified on an S88 MK3 against agent `2.0.7 (R15)`, with Komplete Kontrol
//! closed:
//!
//! 1. Write the 16-byte UUID prelude — done by [`Client::connect`].
//! 2. [`Client::instance_hello`].
//! 3. [`Client::connect_device`] — attaches the device; the keyboard leaves
//!    MIDI mode for its host UI.
//! 4. [`Client::request_focus`] — required, and it lapses when idle.
//! 5. Device calls: [`Client::set_leds`], [`Client::notification`], …
//!
//! Every device call takes `(instance_uuid, serial, …)` in that order.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub mod events;

use rmpv::Value;
use thiserror::Error;

/// Where the vendor's agent listens.
pub const DEFAULT_SOCKET: &str =
    "/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks";

/// `ipc_protocol_version` to declare when the agent's greeting does not say.
///
/// Matches agent `2.0.7 (R15)`, the version the working sequence was
/// confirmed against. Newer agents announce their own, which takes precedence.
pub const FALLBACK_IPC_VERSION: &str = "2.1.0";

/// Largest frame we will accept, so a corrupt length cannot ask us to
/// allocate the heap.
const MAX_FRAME: u32 = 64 * 1024 * 1024;

/// Errors talking to the agent.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OdrError {
    /// The socket could not be reached. Usually means the vendor's agent is
    /// not running.
    #[error("cannot reach the ODR agent at {path}: {source}")]
    Connect {
        /// Socket path tried.
        path: PathBuf,
        /// Underlying cause.
        source: std::io::Error,
    },

    /// An I/O error during a read or write.
    #[error("{op} failed: {source}")]
    Io {
        /// What was being attempted.
        op: &'static str,
        /// Underlying cause.
        source: std::io::Error,
    },

    /// A frame declared a length we refuse to allocate.
    #[error("frame declares {len} bytes, over the {MAX_FRAME} limit")]
    FrameTooLarge {
        /// Declared length.
        len: u32,
    },

    /// A frame's bytes were not valid msgpack.
    #[error("frame was not valid msgpack: {0}")]
    Decode(String),

    /// The agent returned an error for a request.
    ///
    /// Its two common messages are worth distinguishing: *"Method not
    /// registered"* means the method does not exist on this endpoint at all,
    /// while *"Arguments cannot be parsed"* means it does and the parameters
    /// were wrong.
    #[error("agent rejected {method}: {message}")]
    Rejected {
        /// Method name, resolved through the registry where possible.
        method: String,
        /// The agent's own message.
        message: String,
    },

    /// A method name was not in the registry the agent sent.
    #[error("method {0:?} is not in this agent's symbol registry")]
    UnknownMethod(String),

    /// No response arrived before the deadline.
    #[error("no response to {method} within {}ms", timeout.as_millis())]
    Timeout {
        /// Method that went unanswered.
        method: String,
        /// How long we waited.
        timeout: Duration,
    },
}

/// The agent's symbol table: the names behind the integers on the wire.
///
/// Index is meaning — position *is* the identifier — so this is both a name
/// lookup and the wire encoding.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

impl Registry {
    /// Build from the ordered list the agent sends in its greeting.
    pub fn from_names(names: Vec<String>) -> Self {
        let index = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i as u32))
            .collect();
        Self { names, index }
    }

    /// How many symbols the agent declared.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the registry is empty.
    ///
    /// An agent run with `NI_ODR_DISABLE_SYMBOL_REGISTRY` sends an empty one.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The wire index for a method name.
    pub fn id(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }

    /// The name for a wire index.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.names.get(id as usize).map(String::as_str)
    }

    /// Every symbol, in index order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Parse a registry from a JSON array of names, in index order.
    ///
    /// Needed because **the agent's IPC greeting does not include the
    /// registry** — only the device-facing endpoint sends one. A client on
    /// the local socket therefore has to be given the table, and capturing it
    /// once from the device endpoint is the reproducible way to get it. See
    /// `docs/protocol/mk3-odr.md`.
    pub fn from_json(text: &str) -> Result<Self, OdrError> {
        let t = text.trim();
        let inner = t
            .strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .ok_or_else(|| OdrError::Decode("registry JSON must be an array".into()))?;
        let mut names = Vec::new();
        for raw in inner.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            let unquoted = item
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .ok_or_else(|| OdrError::Decode(format!("registry entry not a string: {item}")))?;
            names.push(unquoted.to_owned());
        }
        Ok(Self::from_names(names))
    }

    /// Rewrite interned integer **keys** into their names, recursively.
    ///
    /// The agent sends `{218: 220}` where it means `{page: 220}`. Values are
    /// deliberately left alone: some are symbols (`page: 220` is `browser`)
    /// but many are plain numbers — an encoder step of `1`, a `root_key`
    /// note — and converting those turns `1` into `pedal_expression_connected`.
    /// An earlier version of this did exactly that. A caller that knows a
    /// field is an enum can resolve its value with [`Registry::name`].
    pub fn humanise(&self, v: &Value) -> Value {
        match v {
            Value::Array(items) => Value::Array(items.iter().map(|i| self.humanise(i)).collect()),
            Value::Map(pairs) => Value::Map(
                pairs
                    .iter()
                    .map(|(k, val)| (self.humanise_key(k), self.humanise(val)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn humanise_key(&self, k: &Value) -> Value {
        match k.as_u64().and_then(|u| u32::try_from(u).ok()) {
            Some(i) => match self.name(i) {
                Some(s) => Value::String(s.into()),
                None => k.clone(),
            },
            None => k.clone(),
        }
    }
}

/// A message from the agent that is not a reply to one of our requests.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Method index on the wire.
    pub id: u32,
    /// Resolved name, when the registry has one.
    pub name: Option<String>,
    /// Parameters, as sent.
    pub params: Value,
}

pub use events::{Event, TransportAction};

/// Generate a version-4 UUID from the system CSPRNG.
///
/// Hand-rolled to keep this crate's dependency list to msgpack and an error
/// derive. Returns the raw 16 bytes and the canonical hyphenated text, because
/// the protocol needs **both**: the bytes open the connection and the text
/// identifies the instance inside it.
pub fn new_uuid() -> Result<([u8; 16], String), OdrError> {
    use std::io::Read as _;
    let mut raw = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut raw))
        .map_err(|source| OdrError::Io {
            op: "read /dev/urandom",
            source,
        })?;
    // RFC 4122: version 4, variant 1.
    if let Some(b) = raw.get_mut(6) {
        *b = (*b & 0x0F) | 0x40;
    }
    if let Some(b) = raw.get_mut(8) {
        *b = (*b & 0x3F) | 0x80;
    }
    let h: Vec<String> = raw.iter().map(|b| format!("{b:02x}")).collect();
    let text = format!(
        "{}-{}-{}-{}-{}",
        h.get(0..4).unwrap_or_default().concat(),
        h.get(4..6).unwrap_or_default().concat(),
        h.get(6..8).unwrap_or_default().concat(),
        h.get(8..10).unwrap_or_default().concat(),
        h.get(10..16).unwrap_or_default().concat(),
    );
    Ok((raw, text))
}

/// One knob on a parameter page.
///
/// `value` is the normalised position, `0.0..=1.0`; `display` is the text the
/// screen shows for it, such as `"50%"` or `"-6.0 dB"`. The device draws both.
#[derive(Debug, Clone, Copy)]
pub struct Knob<'a> {
    /// Parameter id. Placements on the page refer to the parameter by this.
    pub id: u32,
    /// The label under the knob.
    pub name: &'a str,
    /// Normalised position.
    pub value: f64,
    /// Text shown for the current value.
    pub display: &'a str,
}

/// How long one socket read waits for the agent by default.
const READ_TIMEOUT: Duration = Duration::from_millis(250);

/// The most knobs one NKS1 section can place.
pub const KNOBS_PER_SECTION: usize = 8;

/// The most pages an NKS1 layout can address: the view address is a byte.
pub const MAX_PAGES: usize = 256;

/// An image in the agent's asset store, addressed by the SHA-256 of its bytes.
///
/// Exactly 32 bytes by construction, and only made by hashing an image. The
/// agent deserialises a handle into a fixed 32-byte type and **aborts the whole
/// service** on any other shape, so a handle is never built from untyped input.
/// See `docs/protocol/mk3-odr-ipc.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetHandle([u8; 32]);

impl AssetHandle {
    /// The handle of these image bytes.
    pub fn of(image: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self(Sha256::digest(image).into())
    }

    /// The 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn to_value(self) -> Value {
        Value::Binary(self.0.to_vec())
    }
}

/// Largest image [`Client::add_asset`] will send. A 1280x212 header banner is
/// typically under 300 KB.
pub const MAX_ASSET_BYTES: usize = 4 * 1024 * 1024;

/// An image format the agent decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// WebP, lossy or lossless.
    WebP,
}

impl ImageFormat {
    /// Recognise an image by its signature.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Some(Self::WebP)
        } else {
            None
        }
    }
}

/// Check an image before it goes to the agent.
fn check_image(image: &[u8]) -> Result<ImageFormat, OdrError> {
    if image.len() > MAX_ASSET_BYTES {
        return Err(OdrError::Decode(format!(
            "image is {} bytes, over the {MAX_ASSET_BYTES}-byte limit",
            image.len()
        )));
    }
    ImageFormat::sniff(image).ok_or_else(|| OdrError::Decode("not a PNG or WebP image".into()))
}

/// A colour, as the parameter page takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// The blue every page used before colours were configurable.
    pub const DEFAULT: Self = Self {
        r: 40,
        g: 110,
        b: 230,
    };

    fn to_value(self) -> Value {
        map(vec![
            ("r", Value::from(self.r)),
            ("g", Value::from(self.g)),
            ("b", Value::from(self.b)),
        ])
    }
}

/// Everything one parameter-page publish carries.
#[derive(Debug, Clone)]
pub struct ParameterPage<'a> {
    /// The name across the top of the screen.
    pub plugin_name: &'a str,
    /// The NKS1 layout: pages, each a run of sections.
    pub pages: &'a [Vec<KnobSection<'a>>],
    /// The page to show, counting from 0.
    pub show_page: usize,
    /// Header image, from [`Client::add_asset`]. `None` draws none.
    pub background: Option<AssetHandle>,
    /// `plugin_color`. `None` sends `nil`, which the agent accepts.
    pub plugin_color: Option<Rgb>,
    /// `nks_control_color`. `None` sends `nil`.
    pub control_color: Option<Rgb>,
}

/// A section header over a run of knob slots, as an NKS1 page shows it.
///
/// A page is a list of these; its slots together fill at most eight knobs.
#[derive(Debug, Clone)]
pub struct KnobSection<'a> {
    /// Header text. Empty draws no header.
    pub name: &'a str,
    /// Knob slots in order. `None` leaves a knob unassigned.
    pub knobs: Vec<Option<Knob<'a>>>,
}

/// Transport position, as the device shows it on the Play/Stop buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// `stopped`.
    Stopped,
    /// `paused`.
    Paused,
    /// `playing`.
    Playing,
}

/// Record button state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record {
    /// `off`.
    Off,
    /// `armed`.
    Armed,
    /// `recording`.
    Recording,
}

/// The three-way state of a toggle button's LED: not available, available but
/// inactive, or active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// `disabled` — the function is not available.
    Disabled,
    /// `off` — available, inactive.
    Off,
    /// `on` — active.
    On,
}

impl Transport {
    fn wire(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Paused => "paused",
            Self::Playing => "playing",
        }
    }
}
impl Record {
    fn wire(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Armed => "armed",
            Self::Recording => "recording",
        }
    }
}
impl Control {
    fn wire(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Off => "off",
            Self::On => "on",
        }
    }
}

/// What the host reports about itself: transport, undo/quantize availability,
/// and the names shown on screen.
///
/// The enum spellings were read from the agent's own lookup tables
/// (`lookup_enum_from_string<transport_state | record_state |
/// control_state>`), not guessed.
#[derive(Debug, Clone, Copy)]
pub struct HostState<'a> {
    /// Transport position.
    pub transport: Transport,
    /// Record state.
    pub record: Record,
    /// Metronome.
    pub metro: Control,
    /// Loop.
    pub loop_: Control,
    /// Tap tempo.
    pub tap_tempo: Control,
    /// Count-in.
    pub count_in: Control,
    /// Whether undo is available.
    pub can_undo: bool,
    /// Whether redo is available.
    pub can_redo: bool,
    /// Whether quantize is available.
    pub can_quantize: bool,
    /// Whether half-quantize is available.
    pub can_quantize_half: bool,
    /// Tempo in BPM.
    pub tempo: f64,
    /// Track name.
    pub track_name: &'a str,
    /// Preset name.
    pub preset_name: &'a str,
}

/// A connected ODR client.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    buf: Vec<u8>,
    next_id: u32,
    registry: Registry,
    greeting: Option<Value>,
    pending: Vec<Notification>,
    instance_id: String,
    identified: bool,
}

impl Client {
    /// Connect to the agent and complete the session opening.
    ///
    /// Reads the greeting, adopts the symbol registry from it, and spends the
    /// priming request the agent swallows — so the returned client is ready
    /// for real calls.
    pub fn connect(path: impl AsRef<Path>) -> Result<Self, OdrError> {
        let path = path.as_ref();
        let client = Self::connect_raw(path)?;
        Ok(client)
    }

    /// Connect and read the greeting, without spending the priming request.
    /// Connect to the agent's default socket path.
    pub fn connect_default() -> Result<Self, OdrError> {
        Self::connect(DEFAULT_SOCKET)
    }

    /// Connect with a registry supplied by the caller.
    ///
    /// The IPC handshake reply carries no symbol registry — only the
    /// device-facing endpoint sends one — so a client on this socket must be
    /// given the table. A registry the agent *does* send still wins.
    pub fn connect_with_registry(
        path: impl AsRef<Path>,
        registry: Registry,
    ) -> Result<Self, OdrError> {
        let mut c = Self::connect_raw(path)?;
        if c.registry.is_empty() {
            c.registry = registry;
        }
        Ok(c)
    }

    fn connect_raw(path: impl AsRef<Path>) -> Result<Self, OdrError> {
        let path = path.as_ref();
        let stream = UnixStream::connect(path).map_err(|source| OdrError::Connect {
            path: path.to_path_buf(),
            source,
        })?;
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .map_err(|source| OdrError::Io {
                op: "set_read_timeout",
                source,
            })?;
        // The connection opens with 16 RAW UUID bytes, before any framing.
        // Without them the agent consumes the first 16 bytes of the first
        // frame as the connection id and that frame is silently lost.
        let (raw_id, text_id) = new_uuid()?;
        (&stream)
            .write_all(&raw_id)
            .map_err(|source| OdrError::Io {
                op: "write connection id",
                source,
            })?;

        let mut client = Self {
            stream,
            buf: Vec::new(),
            next_id: 0,
            registry: Registry::default(),
            greeting: None,
            pending: Vec::new(),
            instance_id: text_id,
            identified: false,
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && client.greeting.is_none() {
            if let Some(v) = client.read_frame()? {
                client.absorb_greeting(&v);
            }
        }
        Ok(client)
    }

    /// The symbol registry the agent sent, or the one supplied on connect.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The handshake payload: `agent_version`, `ipc_protocol_version` and the
    /// attached devices.
    pub fn greeting(&self) -> Option<&Value> {
        self.greeting.as_ref()
    }

    /// Notifications received while waiting for replies.
    pub fn drain_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.pending)
    }

    fn take_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    /// Call a method by name and wait for its reply.
    pub fn call(
        &mut self,
        method: &str,
        params: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, OdrError> {
        let id = self
            .registry
            .id(method)
            .ok_or_else(|| OdrError::UnknownMethod(method.to_owned()))?;
        let msgid = self.take_id();
        self.send(Value::Array(vec![
            Value::from(0u32),
            Value::from(msgid),
            Value::from(id),
            Value::Array(params),
        ]))?;

        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let Some(frame) = self.read_frame()? else {
                continue;
            };
            match self.classify(frame, msgid) {
                Classified::Reply(Ok(v)) => return Ok(v),
                Classified::Reply(Err(message)) => {
                    return Err(OdrError::Rejected {
                        method: method.to_owned(),
                        message,
                    });
                }
                Classified::Other => {}
            }
        }
        Err(OdrError::Timeout {
            method: method.to_owned(),
            timeout,
        })
    }

    /// Identify this connection as a client instance.
    ///
    /// Step one of the sequence confirmed on hardware — see
    /// `docs/protocol/mk3-odr-ipc.md`. Sent by **string name**; everything
    /// after it is sent by registry id.
    ///
    /// `ipc_protocol_version` is taken from the agent's own greeting where
    /// available, so the client declares the version the agent speaks. A
    /// mismatch here was one of four differences between this crate's
    /// failing attempts and the working sequence.
    pub fn instance_hello(&mut self, name: &str, version: &str) -> Result<Value, OdrError> {
        let ipc = self
            .agent_ipc_version()
            .unwrap_or_else(|| FALLBACK_IPC_VERSION.to_owned());
        let args = vec![
            Value::String(self.instance_id.clone().into()),
            Value::Map(vec![
                (
                    Value::String("client_info".into()),
                    Value::Map(vec![
                        (Value::String("name".into()), Value::String(name.into())),
                        (
                            Value::String("version".into()),
                            Value::String(version.into()),
                        ),
                        (
                            Value::String("type".into()),
                            Value::String("standalone".into()),
                        ),
                    ]),
                ),
                (
                    Value::String("ipc_protocol_version".into()),
                    Value::String(ipc.into()),
                ),
            ]),
        ];
        // Matched by msgid, so this is a real answer to the request -- not
        // the unsolicited connect greeting, which has the same shape.
        let reply = self.call_named("instance_hello", args, Duration::from_secs(3))?;
        self.identified = true;
        Ok(reply)
    }

    /// The `ipc_protocol_version` the agent announced on connect.
    fn agent_ipc_version(&self) -> Option<String> {
        let Some(Value::Map(fields)) = &self.greeting else {
            return None;
        };
        fields.iter().find_map(|(k, v)| {
            if k.as_str() == Some("ipc_protocol_version") {
                v.as_str().map(str::to_owned)
            } else {
                None
            }
        })
    }

    /// Whether [`Client::instance_hello`] was answered on this connection.
    ///
    /// Answered, not attached: until [`Client::connect_device`] succeeds the
    /// agent has no device for this instance.
    pub fn is_identified(&self) -> bool {
        self.identified
    }

    /// This connection's instance id, which every device call carries first.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Attach a device to this instance.
    ///
    /// **The step this crate's earlier attempts never made.** Without it the
    /// agent has no device for the instance, and every display or LED call
    /// fails with `no such device <instance>`. Together with
    /// [`Client::request_focus`] it is what moves the keyboard out of MIDI
    /// mode into its host UI — observed on hardware as the screen changing to
    /// "no instrument loaded".
    pub fn connect_device(&mut self, serial: &str) -> Result<Value, OdrError> {
        let args = self.device_args(serial, vec![]);
        self.call("connect_device", args, Duration::from_secs(3))
    }

    /// Ask the agent for control of the device's surface.
    ///
    /// Required: without focus, LED and page messages are discarded. Focus
    /// also **lapses after a short idle period**, so a long-running host
    /// should re-assert it — about once a second works, per prior art.
    pub fn request_focus(&mut self, serial: &str) -> Result<(), OdrError> {
        let args = self.device_args(serial, vec![]);
        self.notify_symbol("client_request_focus", args)
    }

    /// Set the light guide.
    ///
    /// `leds` pairs a **MIDI note** (not a key position; the array has 128
    /// slots) with a colour byte. Confirmed values: green `0x1C`, red `0x04`,
    /// off `0x00`.
    pub fn set_leds(&mut self, serial: &str, leds: &[(u8, u8)]) -> Result<(), OdrError> {
        let key = self
            .registry
            .id("client_midi_addressing")
            .ok_or_else(|| OdrError::UnknownMethod("client_midi_addressing".to_owned()))?;
        let array = Value::Array(
            leds.iter()
                .map(|&(note, colour)| Value::Array(vec![Value::from(note), Value::from(colour)]))
                .collect(),
        );
        let args = self.device_args(serial, vec![Value::Map(vec![(Value::from(key), array)])]);
        self.notify_symbol("client_lightguide_set_leds", args)
    }

    /// Select which page the device shows.
    ///
    /// `page` is a symbol index: `browser`, `mixer`, `parameter` and so on
    /// resolve through [`Client::registry`].
    pub fn set_page(&mut self, serial: &str, page: u32) -> Result<(), OdrError> {
        let args = self.device_args(serial, vec![Value::from(page)]);
        self.notify_symbol("client_set_page", args)
    }

    /// Show a notification on the device's screen.
    ///
    /// The six fields are the ones the agent itself listed as required.
    pub fn notification(
        &mut self,
        serial: &str,
        title: &str,
        description: &str,
    ) -> Result<(), OdrError> {
        let s = |t: &str| Value::String(t.into());
        let body = Value::Map(vec![(
            s("notification"),
            Value::Map(vec![
                (s("level"), s("info")),
                (s("title"), s(title)),
                (s("description"), s(description)),
                (s("url_message"), s("")),
                (s("url"), s("")),
                (s("blocking_state"), s("non_blocking")),
            ]),
        )]);
        let args = self.device_args(serial, vec![body]);
        self.notify_symbol("send_global_notification", args)
    }

    /// Publish a parameter page: a plugin name and up to eight labelled knobs.
    ///
    /// Replaces the device's "no instrument loaded" page. Every field was
    /// recovered from the agent's own warnings and confirmed to parse; see
    /// `docs/protocol/mk3-odr-ipc.md`. No image is sent, so no asset handle is
    /// involved — a malformed handle is known to abort the agent.
    pub fn set_parameter_page(
        &mut self,
        serial: &str,
        plugin_name: &str,
        section_name: &str,
        knobs: &[Knob<'_>],
    ) -> Result<(), OdrError> {
        let section = KnobSection {
            name: section_name,
            knobs: knobs.iter().copied().map(Some).collect(),
        };
        self.set_parameter_layout(serial, plugin_name, &[vec![section]], 0)
    }

    /// Publish a full NKS1 layout: several pages, each a run of sections.
    ///
    /// This is the shape an NKS preset describes, so a preset's pages can be
    /// sent as they are. `show_page` selects the page on screen.
    pub fn set_parameter_layout(
        &mut self,
        serial: &str,
        plugin_name: &str,
        pages: &[Vec<KnobSection<'_>>],
        show_page: usize,
    ) -> Result<(), OdrError> {
        self.publish_parameter_page(
            serial,
            &ParameterPage {
                plugin_name,
                pages,
                show_page,
                background: None,
                plugin_color: Some(Rgb::DEFAULT),
                control_color: Some(Rgb::DEFAULT),
            },
        )
    }

    /// Publish a parameter page, header image included.
    pub fn publish_parameter_page(
        &mut self,
        serial: &str,
        page: &ParameterPage<'_>,
    ) -> Result<(), OdrError> {
        let body = parameter_page_body(page)?;
        let args = self.device_args(serial, vec![body]);
        self.notify_symbol("client_parameter_page_set_data", args)
    }

    /// Store an image in the agent's asset store and return its handle.
    ///
    /// The image must be PNG or WebP; the agent decodes it. The store is
    /// content-addressed and outlives the connection, so sending the same
    /// image again is harmless. A notification, not a request: the agent
    /// answers `Method not registered` if it is sent as a request.
    pub fn add_asset(&mut self, image: &[u8]) -> Result<AssetHandle, OdrError> {
        check_image(image)?;
        let handle = AssetHandle::of(image);
        self.notify_symbol(
            "add_asset",
            vec![handle.to_value(), Value::Binary(image.to_vec())],
        )?;
        Ok(handle)
    }

    /// Update one knob's value on the published parameter page.
    ///
    /// **The device does not own parameter values; the host does.** A knob
    /// turn arrives as a relative step (`plugin_parameter_value_change
    /// [id, delta]`) and nothing on screen moves until the host applies it and
    /// sends the result back through this call. Four arguments, confirmed by
    /// the agent's own `args_to_tuple size mismatch: expected 4`; the value is
    /// kind-tagged exactly as in the page.
    pub fn set_parameter_value(
        &mut self,
        serial: &str,
        id: u32,
        value: f64,
        display: &str,
    ) -> Result<(), OdrError> {
        let s = |t: &str| Value::String(t.into());
        let tagged = Value::Map(vec![(
            s("continuous_parameter"),
            Value::Map(vec![
                (s("value"), Value::F64(value)),
                (s("display_value"), s(display)),
            ]),
        )]);
        let args = self.device_args(serial, vec![Value::from(id), tagged]);
        self.notify_symbol("client_parameter_page_set_parameter_value", args)
    }

    /// Report host state: transport, undo/quantize availability, names.
    ///
    /// This is what lights the transport buttons. The device does not keep
    /// its own transport; it shows whatever the host reports.
    pub fn set_host_state(&mut self, serial: &str, st: &HostState<'_>) -> Result<(), OdrError> {
        let s = |t: &str| Value::String(t.into());
        let map = |pairs: Vec<(&str, Value)>| {
            Value::Map(pairs.into_iter().map(|(k, v)| (s(k), v)).collect())
        };
        let body = map(vec![
            (
                "transport_section_state",
                map(vec![
                    ("transport", s(st.transport.wire())),
                    ("recording", s(st.record.wire())),
                    ("metro", s(st.metro.wire())),
                    ("loop", s(st.loop_.wire())),
                    ("tap_tempo", s(st.tap_tempo.wire())),
                    ("count_in", s(st.count_in.wire())),
                ]),
            ),
            (
                "undo_state",
                map(vec![
                    ("can_undo", Value::Boolean(st.can_undo)),
                    ("can_redo", Value::Boolean(st.can_redo)),
                ]),
            ),
            (
                "quantize_state",
                map(vec![
                    ("can_quantize", Value::Boolean(st.can_quantize)),
                    ("can_quantize_half", Value::Boolean(st.can_quantize_half)),
                ]),
            ),
            ("smartplay_enabled", Value::Boolean(false)),
            ("browser_enabled", Value::Boolean(true)),
            ("tempo", Value::F64(st.tempo)),
            ("current_track_name", s(st.track_name)),
            ("current_preset_name", s(st.preset_name)),
        ]);
        let args = self.device_args(serial, vec![body]);
        self.notify_symbol("client_set_host_owned_instance_client_state", args)
    }

    /// `[instance_uuid, serial, extra...]` — the order every device call uses.
    ///
    /// This crate originally sent `(serial, instance)`, which the agent reads
    /// as "the device is this instance uuid" and rejects. Centralised so the
    /// order cannot drift between calls again.
    fn device_args(&self, serial: &str, extra: Vec<Value>) -> Vec<Value> {
        let mut v = vec![
            Value::String(self.instance_id.clone().into()),
            Value::String(serial.into()),
        ];
        v.extend(extra);
        v
    }

    /// The wire form of a method: its registry id when known, else its name.
    fn method_value(&self, name: &str) -> Value {
        match self.registry.id(name) {
            Some(id) => Value::from(id),
            None => Value::String(name.into()),
        }
    }

    /// Send a notification, by registry id where the registry has one.
    pub fn notify_symbol(&mut self, method: &str, params: Vec<Value>) -> Result<(), OdrError> {
        let m = self.method_value(method);
        self.send(Value::Array(vec![
            Value::from(2u32),
            m,
            Value::Array(params),
        ]))
    }

    /// Send a notification with a method named by string.
    ///
    /// The IPC side exchanges no symbol registry, so its method names go on
    /// the wire as strings — unlike the device side, which interns them.
    pub fn notify_named(&mut self, method: &str, params: Vec<Value>) -> Result<(), OdrError> {
        self.send(Value::Array(vec![
            Value::from(2u32),
            Value::String(method.into()),
            Value::Array(params),
        ]))
    }

    /// Call a method named by string and wait for its reply.
    pub fn call_named(
        &mut self,
        method: &str,
        params: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, OdrError> {
        let msgid = self.take_id();
        self.send(Value::Array(vec![
            Value::from(0u32),
            Value::from(msgid),
            Value::String(method.into()),
            Value::Array(params),
        ]))?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let Some(frame) = self.read_frame()? else {
                continue;
            };
            match self.classify(frame, msgid) {
                Classified::Reply(Ok(v)) => return Ok(v),
                Classified::Reply(Err(message)) => {
                    return Err(OdrError::Rejected {
                        method: method.to_owned(),
                        message,
                    });
                }
                Classified::Other => {}
            }
        }
        Err(OdrError::Timeout {
            method: method.to_owned(),
            timeout,
        })
    }

    /// Collect notifications for a fixed window, for observing the agent.
    pub fn listen(&mut self, window: Duration) -> Result<Vec<Notification>, OdrError> {
        let deadline = Instant::now() + window;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            // A read must not outlast the window. Without this a 5 ms listen
            // blocked for the socket's full READ_TIMEOUT whenever the agent
            // was quiet, and a host polling between listens lagged by that.
            self.set_wait(left.clamp(Duration::from_millis(1), READ_TIMEOUT))?;
            if let Some(frame) = self.read_frame()? {
                let _ = self.classify(frame, u32::MAX);
            }
        }
        self.set_wait(READ_TIMEOUT)?;
        Ok(self.drain_notifications())
    }

    fn set_wait(&self, wait: Duration) -> Result<(), OdrError> {
        self.stream
            .set_read_timeout(Some(wait))
            .map_err(|source| OdrError::Io {
                op: "set_read_timeout",
                source,
            })
    }

    fn classify(&mut self, frame: Value, want: u32) -> Classified {
        let Value::Array(items) = &frame else {
            return Classified::Other;
        };
        match items.first().and_then(|v| v.as_u64()) {
            // [1, msgid, error, result]
            Some(1) => {
                let msgid = items.get(1).and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
                if msgid != u64::from(want) {
                    return Classified::Other;
                }
                let err = items.get(2).filter(|v| !v.is_nil());
                match err {
                    Some(e) => Classified::Reply(Err(match e.as_str() {
                        Some(s) => s.to_owned(),
                        None => format!("{e}"),
                    })),
                    None => Classified::Reply(Ok(items.get(3).cloned().unwrap_or(Value::Nil))),
                }
            }
            // [2, method, params]
            Some(2) => {
                let id = items
                    .get(1)
                    .and_then(|v| v.as_u64())
                    .and_then(|u| u32::try_from(u).ok());
                if let Some(id) = id {
                    let name = self.registry.name(id).map(str::to_owned);
                    self.pending.push(Notification {
                        id,
                        name,
                        params: items.get(2).cloned().unwrap_or(Value::Nil),
                    });
                }
                Classified::Other
            }
            _ => Classified::Other,
        }
    }

    fn absorb_greeting(&mut self, v: &Value) {
        let Value::Array(items) = v else { return };
        if items.first().and_then(|x| x.as_u64()) != Some(1) {
            return;
        }
        let Some(Value::Map(fields)) = items.get(3) else {
            return;
        };
        for (k, val) in fields {
            if k.as_str() == Some("symbol_registry")
                && let Value::Array(entries) = val
            {
                let names = entries
                    .iter()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect();
                self.registry = Registry::from_names(names);
            }
        }
        self.greeting = Some(items.get(3).cloned().unwrap_or(Value::Nil));
    }

    fn send(&mut self, v: Value) -> Result<(), OdrError> {
        let mut body = Vec::new();
        rmpv::encode::write_value(&mut body, &v).map_err(|e| OdrError::Decode(e.to_string()))?;
        let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
        self.stream
            .write_all(&len.to_le_bytes())
            .and_then(|()| self.stream.write_all(&body))
            .map_err(|source| OdrError::Io {
                op: "write",
                source,
            })
    }

    /// Read one frame if a complete one is available, else `Ok(None)`.
    fn read_frame(&mut self) -> Result<Option<Value>, OdrError> {
        loop {
            if self.buf.len() >= 4 {
                let mut len_bytes = [0u8; 4];
                len_bytes.copy_from_slice(self.buf.get(..4).unwrap_or(&[0; 4]));
                let len = u32::from_le_bytes(len_bytes);
                if len > MAX_FRAME {
                    return Err(OdrError::FrameTooLarge { len });
                }
                let total = 4 + len as usize;
                if self.buf.len() >= total {
                    let body: Vec<u8> = self.buf.drain(..total).skip(4).collect();
                    let mut slice = body.as_slice();
                    let v = rmpv::decode::read_value(&mut slice)
                        .map_err(|e| OdrError::Decode(e.to_string()))?;
                    return Ok(Some(v));
                }
            }
            let mut chunk = [0u8; 65536];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(None),
                Ok(n) => self.buf.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(source) => return Err(OdrError::Io { op: "read", source }),
            }
        }
    }
}

enum Classified {
    Reply(Result<Value, String>),
    Other,
}

fn s(t: &str) -> Value {
    Value::String(t.into())
}

fn map(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(pairs.into_iter().map(|(k, v)| (s(k), v)).collect())
}

/// Build the `client_parameter_page_set_data` body. Kept apart from the
/// socket so its shape can be tested.
fn parameter_page_body(page: &ParameterPage<'_>) -> Result<Value, OdrError> {
    let ParameterPage {
        plugin_name,
        pages,
        show_page,
        background,
        plugin_color,
        control_color,
    } = *page;
    if pages.len() > MAX_PAGES {
        return Err(OdrError::Decode(format!(
            "{} pages given; an NKS1 view address selects at most {MAX_PAGES}",
            pages.len()
        )));
    }
    if show_page >= pages.len().max(1) {
        return Err(OdrError::Decode(format!(
            "page {show_page} requested of {}",
            pages.len()
        )));
    }
    for (i, page) in pages.iter().enumerate() {
        let slots: usize = page.iter().map(|sec| sec.knobs.len()).sum();
        if slots > KNOBS_PER_SECTION {
            return Err(OdrError::Decode(format!(
                "page {i} has {slots} knob slots; an NKS1 page holds at most {KNOBS_PER_SECTION}"
            )));
        }
    }

    // parameters: {id: {continuous_parameter: {parameter_value, parameter_style}}}
    // One entry per id; a preset may place the same parameter on two pages.
    let mut seen = std::collections::HashSet::new();
    let parameters = Value::Map(
        pages
            .iter()
            .flatten()
            .flat_map(|sec| sec.knobs.iter().flatten())
            .filter(|k| seen.insert(k.id))
            .map(|k| {
                let entry = map(vec![(
                    "continuous_parameter",
                    map(vec![
                        (
                            "parameter_value",
                            map(vec![
                                ("value", Value::F64(k.value)),
                                ("display_value", s(k.display)),
                            ]),
                        ),
                        (
                            "parameter_style",
                            map(vec![("knob_parameter_style", Value::Nil)]),
                        ),
                    ]),
                )]);
                (Value::from(k.id), entry)
            })
            .collect(),
    );

    // nks1_layout: [page], a page is [section], a section is
    // {section_name, parameters: [{id, name} | nil]}.
    let layout_pages = pages
        .iter()
        .map(|page| {
            Value::Array(
                page.iter()
                    .map(|sec| {
                        let placements = sec
                            .knobs
                            .iter()
                            .map(|k| match k {
                                Some(k) => {
                                    map(vec![("id", Value::from(k.id)), ("name", s(k.name))])
                                }
                                None => Value::Nil,
                            })
                            .collect();
                        map(vec![
                            ("section_name", s(sec.name)),
                            ("parameters", Value::Array(placements)),
                        ])
                    })
                    .collect(),
            )
        })
        .collect();
    let layout = map(vec![
        ("nks1_layout", Value::Array(layout_pages)),
        ("nks2_layout", Value::Nil),
    ]);

    // Which view the device shows. Decompiled
    // (convert_variant_with_symbol_registry<…nks1/nks2_view_address…>,
    // 0x1002e0b64): a one-entry map whose value, for NKS1, must be an integer
    // <= 255 — the page index. force_host_viewstate asks the device to show
    // the host's view rather than its own.
    let viewstate = map(vec![
        (
            "view_address",
            map(vec![("nks1_view_address", Value::from(show_page as u64))]),
        ),
        ("force_host_viewstate", Value::Boolean(true)),
    ]);

    Ok(map(vec![
        (
            "plugin_data",
            map(vec![
                ("name", s(plugin_name)),
                (
                    "background",
                    background.map_or(Value::Nil, AssetHandle::to_value),
                ),
                ("parameters", parameters),
                (
                    "plugin_color",
                    plugin_color.map_or(Value::Nil, Rgb::to_value),
                ),
                (
                    "nks_control_color",
                    control_color.map_or(Value::Nil, Rgb::to_value),
                ),
            ]),
        ),
        ("device_owned_viewstate", viewstate),
        ("host_owned_viewstate", layout),
    ]))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    fn knob(id: u32, name: &str) -> Knob<'_> {
        Knob {
            id,
            name,
            value: 0.5,
            display: "",
        }
    }

    fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
        match v {
            Value::Map(m) => m
                .iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .map(|(_, v)| v)
                .unwrap_or_else(|| panic!("no key {key}")),
            _ => panic!("not a map looking for {key}"),
        }
    }

    fn page<'a>(
        plugin_name: &'a str,
        pages: &'a [Vec<KnobSection<'a>>],
        show_page: usize,
        background: Option<AssetHandle>,
    ) -> ParameterPage<'a> {
        ParameterPage {
            plugin_name,
            pages,
            show_page,
            background,
            plugin_color: Some(Rgb::DEFAULT),
            control_color: Some(Rgb::DEFAULT),
        }
    }

    #[test]
    fn an_asset_handle_is_the_sha256_of_the_image() {
        // FIPS 180-2 test vector for "abc".
        let h = AssetHandle::of(b"abc");
        assert_eq!(
            h.as_bytes()[..4],
            [0xba, 0x78, 0x16, 0xbf],
            "sha256(abc) starts ba7816bf"
        );
        assert_eq!(h.as_bytes()[28..], [0xf2, 0x00, 0x15, 0xad]);
        assert!(matches!(h.to_value(), Value::Binary(b) if b.len() == 32));
    }

    #[test]
    fn only_png_and_webp_are_sent() {
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\nrest"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8L"),
            Some(ImageFormat::WebP)
        );
        assert_eq!(ImageFormat::sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(ImageFormat::sniff(b"GIF89a"), None);
        assert!(check_image(b"not an image").is_err());
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_ASSET_BYTES + 1, 0);
        assert!(check_image(&big).is_err());
    }

    #[test]
    fn a_background_is_sent_as_its_32_byte_handle() {
        let section = KnobSection {
            name: "One",
            knobs: vec![Some(knob(1, "A"))],
        };
        let pages = [vec![section]];
        let h = AssetHandle::of(b"\x89PNG\r\n\x1a\n");
        let body = parameter_page_body(&page("x", &pages, 0, Some(h))).unwrap();
        let bg = get(get(&body, "plugin_data"), "background");
        assert!(matches!(bg, Value::Binary(b) if b.as_slice() == h.as_bytes()));
        let body = parameter_page_body(&page("x", &pages, 0, None)).unwrap();
        assert!(get(get(&body, "plugin_data"), "background").is_nil());
    }

    #[test]
    fn a_layout_carries_every_page_its_sections_and_empty_slots() {
        let pages = vec![
            vec![
                KnobSection {
                    name: "Filter",
                    knobs: vec![Some(knob(10, "Cutoff")), None],
                },
                KnobSection {
                    name: "Amp",
                    knobs: vec![Some(knob(20, "Attack"))],
                },
            ],
            vec![KnobSection {
                name: "Global",
                knobs: vec![Some(knob(0, "Volume"))],
            }],
        ];
        let body = parameter_page_body(&page("Super 8", &pages, 1, None)).unwrap();

        let layout = get(get(&body, "host_owned_viewstate"), "nks1_layout");
        let Value::Array(layout) = layout else {
            panic!("layout is not a list")
        };
        assert_eq!(layout.len(), 2);
        let Value::Array(first) = &layout[0] else {
            panic!("page is not a list")
        };
        assert_eq!(first.len(), 2);
        assert_eq!(get(&first[0], "section_name").as_str(), Some("Filter"));
        let Value::Array(slots) = get(&first[0], "parameters") else {
            panic!("parameters is not a list")
        };
        assert_eq!(get(&slots[0], "name").as_str(), Some("Cutoff"));
        assert!(slots[1].is_nil());

        let view = get(get(&body, "device_owned_viewstate"), "view_address");
        assert_eq!(get(view, "nks1_view_address").as_u64(), Some(1));
    }

    #[test]
    fn a_parameter_placed_twice_is_defined_once() {
        let pages = vec![
            vec![KnobSection {
                name: "A",
                knobs: vec![Some(knob(7, "Cutoff"))],
            }],
            vec![KnobSection {
                name: "B",
                knobs: vec![Some(knob(7, "Cutoff"))],
            }],
        ];
        let body = parameter_page_body(&page("x", &pages, 0, None)).unwrap();
        let Value::Map(params) = get(get(&body, "plugin_data"), "parameters") else {
            panic!("parameters is not a map")
        };
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn a_page_over_eight_slots_is_refused_before_sending() {
        let section = KnobSection {
            name: "Too many",
            knobs: vec![None; KNOBS_PER_SECTION + 1],
        };
        assert!(parameter_page_body(&page("x", &[vec![section]], 0, None)).is_err());
    }

    #[test]
    fn showing_a_page_that_does_not_exist_is_refused() {
        let section = KnobSection {
            name: "One",
            knobs: vec![Some(knob(1, "A"))],
        };
        assert!(parameter_page_body(&page("x", &[vec![section]], 1, None)).is_err());
        // An empty layout is valid, showing page 0.
        assert!(parameter_page_body(&page("x", &[], 0, None)).is_ok());
    }

    #[test]
    fn registry_maps_both_directions() {
        let r = Registry::from_names(vec!["page".into(), "browser".into(), "mixer".into()]);
        assert_eq!(r.id("browser"), Some(1));
        assert_eq!(r.name(1), Some("browser"));
        assert_eq!(r.id("nope"), None);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn humanise_resolves_keys_and_leaves_values_alone() {
        // {218: 220} is {page: 220}. The key is interned; the value is left
        // raw, because values are often plain numbers -- an encoder step of 1
        // is not symbol #1.
        let mut names = vec![String::new(); 221];
        names[1] = "pedal_expression_connected".into();
        names[218] = "page".into();
        names[220] = "browser".into();
        let r = Registry::from_names(names);
        let raw = Value::Map(vec![
            (Value::from(218u32), Value::from(220u32)),
            (
                Value::String("browser_navigate_sound".into()),
                Value::from(1u32),
            ),
        ]);
        let Value::Map(pairs) = r.humanise(&raw) else {
            unreachable!("humanise keeps the map shape")
        };
        assert_eq!(pairs[0].0.as_str(), Some("page"));
        assert_eq!(pairs[0].1, Value::from(220u32), "values stay raw");
        assert_eq!(pairs[1].1, Value::from(1u32), "a step of 1 is not a symbol");
    }

    #[test]
    fn an_integer_with_no_symbol_is_left_alone() {
        // Not every integer is interned; rewriting one would corrupt data.
        let r = Registry::from_names(vec!["a".into()]);
        assert_eq!(r.humanise(&Value::from(9999u32)), Value::from(9999u32));
    }

    #[test]
    fn an_empty_registry_is_reported_as_such() {
        // What an agent run with NI_ODR_DISABLE_SYMBOL_REGISTRY sends.
        let r = Registry::from_names(vec![]);
        assert!(r.is_empty());
        assert_eq!(r.id("handshake"), None);
    }
}
