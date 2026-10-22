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
//! # Two quirks worth knowing
//!
//! - **The first request on a connection is consumed.** It establishes the
//!   session and never gets a response. [`Client::connect`] spends one
//!   automatically so callers never see it.
//! - **The agent's greeting is a response to a request nobody made**, carrying
//!   `ipc_protocol_version` and the attached devices.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rmpv::Value;
use thiserror::Error;

/// Where the vendor's agent listens.
pub const DEFAULT_SOCKET: &str =
    "/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks";

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

    /// Rewrite interned integer keys and values into their names, recursively.
    ///
    /// The agent sends `{218: 220}` where it means `{page: browser}`. Without
    /// this a caller is reading index soup.
    pub fn humanise(&self, v: &Value) -> Value {
        match v {
            Value::Integer(n) => match n.as_u64().and_then(|u| u32::try_from(u).ok()) {
                Some(i) => match self.name(i) {
                    Some(s) => Value::String(s.into()),
                    None => v.clone(),
                },
                None => v.clone(),
            },
            Value::Array(items) => Value::Array(items.iter().map(|i| self.humanise(i)).collect()),
            Value::Map(pairs) => Value::Map(
                pairs
                    .iter()
                    .map(|(k, val)| (self.humanise(k), self.humanise(val)))
                    .collect(),
            ),
            other => other.clone(),
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
            .set_read_timeout(Some(Duration::from_millis(250)))
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
    /// **Required before the display API exists.** An unidentified connection
    /// is served only six device-management methods; `instance_hello`
    /// registers the client role and with it `client_set_page`,
    /// `send_global_notification`, `client_mixer_set_trackinfo` and the rest.
    ///
    /// The reply carries the agent's version, its `ipc_protocol_version` and
    /// the attached devices — the same payload this crate also reads as the
    /// connection greeting, because that *is* what it is.
    pub fn instance_hello(&mut self, name: &str, version: &str) -> Result<Value, OdrError> {
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
                    ]),
                ),
                (
                    Value::String("ipc_protocol_version".into()),
                    Value::String(version.into()),
                ),
            ]),
        ];
        let reply = self.call_named("instance_hello", args, Duration::from_secs(3))?;
        self.identified = true;
        Ok(reply)
    }

    /// Whether [`Client::instance_hello`] has succeeded on this connection.
    pub fn is_identified(&self) -> bool {
        self.identified
    }

    /// This connection's instance id, which every display call carries.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Select which page the device shows.
    ///
    /// `page` is a symbol index: `browser`, `mixer`, `parameter` and so on
    /// resolve through [`Client::registry`].
    pub fn set_page(&mut self, serial: &str, page: u32) -> Result<(), OdrError> {
        self.notify_named(
            "client_set_page",
            vec![
                Value::String(serial.into()),
                Value::String(self.instance_id.clone().into()),
                Value::from(page),
            ],
        )
    }

    /// Show a notification on the device.
    ///
    /// The smallest thing that puts host-supplied content on screen, which
    /// makes it the natural first call to try against new hardware.
    pub fn notification(&mut self, serial: &str, text: &str) -> Result<(), OdrError> {
        self.notify_named(
            "send_global_notification",
            vec![
                Value::String(serial.into()),
                Value::String(self.instance_id.clone().into()),
                Value::Map(vec![(
                    Value::String("text".into()),
                    Value::String(text.into()),
                )]),
            ],
        )
    }

    /// Ask the agent for control of the device's surface.
    pub fn request_focus(&mut self, serial: &str) -> Result<(), OdrError> {
        self.notify_named(
            "client_request_focus",
            vec![
                Value::String(serial.into()),
                Value::String(self.instance_id.clone().into()),
            ],
        )
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
        while Instant::now() < deadline {
            if let Some(frame) = self.read_frame()? {
                let _ = self.classify(frame, u32::MAX);
            }
        }
        Ok(self.drain_notifications())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_maps_both_directions() {
        let r = Registry::from_names(vec!["page".into(), "browser".into(), "mixer".into()]);
        assert_eq!(r.id("browser"), Some(1));
        assert_eq!(r.name(1), Some("browser"));
        assert_eq!(r.id("nope"), None);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn humanise_resolves_interned_keys_and_values() {
        // The agent sends {218: 220} meaning {page: browser}; a caller reading
        // raw indices is reading noise.
        let mut names = vec![String::new(); 221];
        names[218] = "page".into();
        names[220] = "browser".into();
        let r = Registry::from_names(names);
        let raw = Value::Map(vec![(Value::from(218u32), Value::from(220u32))]);
        let out = r.humanise(&raw);
        let Value::Map(pairs) = out else {
            unreachable!("humanise keeps the map shape")
        };
        assert_eq!(pairs[0].0.as_str(), Some("page"));
        assert_eq!(pairs[0].1.as_str(), Some("browser"));
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
