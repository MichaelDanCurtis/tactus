//! Reader for NKS preset files (`.nksf`).
//!
//! An NKS preset is what a host needs to put a plugin on a Kontrol keyboard
//! without knowing anything about the plugin: its browser metadata, which
//! plugin it belongs to, and the knob pages the hardware shows. This crate
//! reads exactly those three things.
//!
//! It deliberately does **not** interpret the plugin's own state. That chunk
//! (`PCHK`) is an opaque blob handed back to the plugin when it loads; only its
//! length is recorded here.
//!
//! # Container
//!
//! A RIFF file of form `NIKS`. Each chunk body is a little-endian `u32`
//! version followed by one msgpack value, and odd-sized chunks are padded to
//! an even length as RIFF requires.
//!
//! | Chunk | Holds |
//! |---|---|
//! | `NISI` | browser metadata: name, vendor, author, bank chain, types |
//! | `NICA` | knob pages, under the key `ni8`: pages of 8 slots |
//! | `PLID` | plugin id: a VST2 magic, and for VST3 plugins a 4-word uid |
//! | `PCHK` | plugin state, opaque |
//!
//! Every one of the 625 presets surveyed on a development machine (Super 8
//! R2 and Massive X) has these four chunks in this order.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

pub mod artwork;

use std::path::Path;

use rmpv::Value;
use thiserror::Error;

/// Knob slots on one NKS1 page.
pub const SLOTS_PER_PAGE: usize = 8;

/// Largest file [`Preset::read`] will load. Real presets are tens to hundreds
/// of kilobytes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Why a preset could not be read.
#[derive(Debug, Error)]
pub enum NksError {
    /// Reading the file failed.
    #[error("reading preset: {0}")]
    Io(#[from] std::io::Error),
    /// Not a RIFF file of form `NIKS`.
    #[error("not an NKS preset")]
    NotNks,
    /// The file is larger than [`MAX_FILE_BYTES`].
    #[error("preset is {0} bytes, over the {MAX_FILE_BYTES}-byte limit")]
    TooLarge(u64),
    /// A header or chunk claims more bytes than the file holds.
    #[error("truncated: {0}")]
    Truncated(String),
    /// A required chunk is absent.
    #[error("missing {0} chunk")]
    Missing(&'static str),
    /// A chunk is too short to carry its version word.
    #[error("{0} chunk has no version word")]
    NoVersion(String),
    /// A chunk's msgpack body did not decode.
    #[error("{chunk} chunk: {source}")]
    Msgpack {
        /// Chunk id.
        chunk: String,
        /// Decoder error.
        source: rmpv::decode::Error,
    },
    /// A chunk decoded but is not the expected shape.
    #[error("{0}")]
    Shape(String),
}

/// Browser metadata, from `NISI`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// Preset name.
    pub name: String,
    /// Plugin vendor.
    pub vendor: String,
    /// Sound designer.
    pub author: String,
    /// Free-text comment.
    pub comment: String,
    /// Product, then bank, then sub-bank.
    pub bankchain: Vec<String>,
    /// Type tags; each entry is a type and optional sub-type.
    pub types: Vec<Vec<String>>,
    /// Mode tags.
    pub modes: Vec<String>,
    /// Character tags. Newer presets only.
    pub characters: Vec<String>,
    /// `INST` or `FX`.
    pub device_type: String,
    /// Preset uuid. Newer presets only.
    pub uuid: Option<String>,
}

/// Which plugin a preset belongs to, from `PLID`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginId {
    /// VST2 unique id. `0` in presets for VST3-only plugins.
    pub vst_magic: u32,
    /// VST3 class id as four 32-bit words.
    pub vst3_uid: Option<[u32; 4]>,
    /// Plugin name, when the preset names it.
    pub name: Option<String>,
    /// Plugin vendor, when the preset names it.
    pub vendor: Option<String>,
}

impl PluginId {
    /// The VST2 id as its four-character code, e.g. `NIS8`.
    ///
    /// `None` when the id is zero or not printable ASCII.
    pub fn vst_magic_fourcc(&self) -> Option<String> {
        if self.vst_magic == 0 {
            return None;
        }
        let bytes = self.vst_magic.to_be_bytes();
        bytes
            .iter()
            .all(|b| b.is_ascii_graphic() || *b == b' ')
            .then(|| bytes.iter().map(|&b| char::from(b)).collect())
    }
}

/// One knob assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// Plugin parameter id.
    pub id: u32,
    /// Label shown under the knob.
    pub name: String,
    /// Section header that starts at this slot, if any.
    pub section: Option<String>,
    /// Whether the name came from the plugin rather than the preset author.
    pub autoname: bool,
    /// Vendor flag. Its meaning is not documented.
    pub vflag: bool,
}

/// A run of slots under one section header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section<'a> {
    /// Header text; empty for slots before the page's first header.
    pub name: &'a str,
    /// The slots, empty ones included so positions are kept.
    pub slots: Vec<Option<&'a Slot>>,
}

/// One page of knobs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    /// Slots in order. `None` is an unassigned knob.
    pub slots: Vec<Option<Slot>>,
}

impl Page {
    /// Group the slots under their section headers.
    ///
    /// A slot with a `section` opens a new section. Slots before the first
    /// header form an unnamed one. This is the shape the keyboard's NKS1
    /// layout wants: one entry per section, each holding its slots.
    pub fn sections(&self) -> Vec<Section<'_>> {
        let mut out: Vec<Section<'_>> = Vec::new();
        for slot in &self.slots {
            let header = slot.as_ref().and_then(|s| s.section.as_deref());
            match (header, out.last_mut()) {
                (None, Some(current)) => current.slots.push(slot.as_ref()),
                (name, _) => out.push(Section {
                    name: name.unwrap_or(""),
                    slots: vec![slot.as_ref()],
                }),
            }
        }
        out
    }

    /// Whether every slot is unassigned.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

/// A parsed preset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preset {
    /// Browser metadata.
    pub info: Info,
    /// The plugin it belongs to.
    pub plugin: PluginId,
    /// Knob pages. Empty when the preset defines none.
    pub pages: Vec<Page>,
    /// Size of the opaque plugin state, in bytes. Never interpreted.
    pub state_len: usize,
}

impl Preset {
    /// Read and parse a preset file.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, NksError> {
        let path = path.as_ref();
        let len = std::fs::metadata(path)?.len();
        if len > MAX_FILE_BYTES {
            return Err(NksError::TooLarge(len));
        }
        Self::parse(&std::fs::read(path)?)
    }

    /// Parse a preset from its bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, NksError> {
        let (Some(b"RIFF"), Some(b"NIKS")) = (bytes.get(0..4), bytes.get(8..12)) else {
            return Err(NksError::NotNks);
        };
        let declared = u32_at(bytes, 4).ok_or(NksError::NotNks)? as usize;
        // The RIFF size counts everything after the size word itself.
        let end = declared
            .checked_add(8)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| {
                NksError::Truncated(format!(
                    "header claims {declared} bytes, file has {}",
                    bytes.len().saturating_sub(8)
                ))
            })?;

        let mut info = None;
        let mut plugin = None;
        let mut pages = Vec::new();
        let mut state_len = 0;

        let mut off = 12;
        while off + 8 <= end {
            let id = bytes.get(off..off + 4).unwrap_or_default();
            let len = u32_at(bytes, off + 4).unwrap_or_default() as usize;
            let start = off + 8;
            let body = start
                .checked_add(len)
                .filter(|&e| e <= end)
                .and_then(|e| bytes.get(start..e))
                .ok_or_else(|| {
                    NksError::Truncated(format!(
                        "{} chunk claims {len} bytes",
                        String::from_utf8_lossy(id)
                    ))
                })?;
            match id {
                b"NISI" => info = Some(parse_info(&chunk_value("NISI", body)?)?),
                b"NICA" => pages = parse_pages(&chunk_value("NICA", body)?)?,
                b"PLID" => plugin = Some(parse_plugin(&chunk_value("PLID", body)?)?),
                b"PCHK" => state_len = len.saturating_sub(4),
                _ => {}
            }
            off = start + len + (len & 1);
        }

        Ok(Self {
            info: info.ok_or(NksError::Missing("NISI"))?,
            plugin: plugin.ok_or(NksError::Missing("PLID"))?,
            pages,
            state_len,
        })
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let b: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(b))
}

/// Skip a chunk's version word and decode its msgpack body.
fn chunk_value(chunk: &str, body: &[u8]) -> Result<Value, NksError> {
    let mut rest = body
        .get(4..)
        .ok_or_else(|| NksError::NoVersion(chunk.into()))?;
    rmpv::decode::read_value(&mut rest).map_err(|source| NksError::Msgpack {
        chunk: chunk.into(),
        source,
    })
}

fn as_map<'a>(v: &'a Value, what: &str) -> Result<&'a [(Value, Value)], NksError> {
    match v {
        Value::Map(m) => Ok(m),
        _ => Err(NksError::Shape(format!("{what} is not a map"))),
    }
}

fn field<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

fn text(map: &[(Value, Value)], key: &str) -> Option<String> {
    field(map, key).and_then(Value::as_str).map(str::to_owned)
}

fn texts(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn flag(map: &[(Value, Value)], key: &str) -> bool {
    field(map, key).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_info(v: &Value) -> Result<Info, NksError> {
    let m = as_map(v, "NISI")?;
    let types = match field(m, "types") {
        Some(Value::Array(a)) => a.iter().map(|t| texts(Some(t))).collect(),
        _ => Vec::new(),
    };
    Ok(Info {
        name: text(m, "name").unwrap_or_default(),
        vendor: text(m, "vendor").unwrap_or_default(),
        author: text(m, "author").unwrap_or_default(),
        comment: text(m, "comment").unwrap_or_default(),
        bankchain: texts(field(m, "bankchain")),
        types,
        modes: texts(field(m, "modes")),
        characters: texts(field(m, "characters")),
        device_type: text(m, "deviceType").unwrap_or_default(),
        uuid: text(m, "uuid"),
    })
}

fn parse_plugin(v: &Value) -> Result<PluginId, NksError> {
    let m = as_map(v, "PLID")?;
    let word = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
    let vst_magic = match field(m, "VST.magic") {
        None => 0,
        Some(v) => word(v).ok_or_else(|| NksError::Shape("VST.magic is not a u32".into()))?,
    };
    let vst3_uid = match field(m, "VST3.uid") {
        None => None,
        Some(Value::Array(a)) if a.len() == 4 => {
            let mut uid = [0u32; 4];
            for (slot, v) in uid.iter_mut().zip(a) {
                *slot =
                    word(v).ok_or_else(|| NksError::Shape("VST3.uid word is not a u32".into()))?;
            }
            Some(uid)
        }
        Some(_) => return Err(NksError::Shape("VST3.uid is not four words".into())),
    };
    Ok(PluginId {
        vst_magic,
        vst3_uid,
        name: text(m, "pluginName"),
        vendor: text(m, "pluginVendor"),
    })
}

fn parse_pages(v: &Value) -> Result<Vec<Page>, NksError> {
    let m = as_map(v, "NICA")?;
    // Four surveyed presets carry an empty NICA map: no pages, not an error.
    let Some(raw) = field(m, "ni8") else {
        return Ok(Vec::new());
    };
    let Value::Array(raw) = raw else {
        return Err(NksError::Shape("NICA ni8 is not a list of pages".into()));
    };
    raw.iter()
        .enumerate()
        .map(|(p, page)| {
            let Value::Array(slots) = page else {
                return Err(NksError::Shape(format!("page {p} is not a list")));
            };
            let slots = slots
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    parse_slot(s).map_err(|e| NksError::Shape(format!("page {p} slot {i}: {e}")))
                })
                .collect::<Result<_, _>>()?;
            Ok(Page { slots })
        })
        .collect()
}

/// A slot without an `id` is an unassigned knob: `{autoname, vflag}`.
fn parse_slot(v: &Value) -> Result<Option<Slot>, String> {
    let Value::Map(m) = v else {
        return Err("not a map".into());
    };
    let Some(id) = field(m, "id") else {
        return Ok(None);
    };
    let id = id
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("id is not a u32")?;
    Ok(Some(Slot {
        id,
        name: text(m, "name").unwrap_or_default(),
        section: text(m, "section"),
        autoname: flag(m, "autoname"),
        vflag: flag(m, "vflag"),
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    fn s(t: &str) -> Value {
        Value::String(t.into())
    }

    fn map(pairs: Vec<(&str, Value)>) -> Value {
        Value::Map(pairs.into_iter().map(|(k, v)| (s(k), v)).collect())
    }

    fn slot(id: u32, name: &str, section: Option<&str>) -> Value {
        let mut f = vec![
            ("autoname", Value::Boolean(false)),
            ("id", Value::from(id)),
            ("name", s(name)),
        ];
        if let Some(sec) = section {
            f.push(("section", s(sec)));
        }
        f.push(("vflag", Value::Boolean(false)));
        map(f)
    }

    fn empty_slot() -> Value {
        map(vec![
            ("autoname", Value::Boolean(false)),
            ("vflag", Value::Boolean(false)),
        ])
    }

    fn body(v: &Value) -> Vec<u8> {
        let mut out = 1u32.to_le_bytes().to_vec();
        rmpv::encode::write_value(&mut out, v).unwrap();
        out
    }

    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut inner = b"NIKS".to_vec();
        for (id, b) in chunks {
            inner.extend_from_slice(*id);
            inner.extend_from_slice(&(b.len() as u32).to_le_bytes());
            inner.extend_from_slice(b);
            if b.len() % 2 == 1 {
                inner.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(inner.len() as u32).to_le_bytes());
        out.extend(inner);
        out
    }

    fn nisi() -> Value {
        // Shaped like Super 8 R2's "Circuits".
        map(vec![
            ("author", s("Jamil Samad")),
            ("bankchain", Value::Array(vec![s("Super 8 R2")])),
            ("comment", s("")),
            ("deviceType", s("INST")),
            ("modes", Value::Array(vec![s("Synthetic")])),
            ("name", s("Circuits")),
            (
                "types",
                Value::Array(vec![Value::Array(vec![s("Bass"), s("Synth")])]),
            ),
            ("vendor", s("Native Instruments")),
        ])
    }

    fn plid() -> Value {
        map(vec![("VST.magic", Value::from(1_313_428_280u32))])
    }

    fn nica(pages: Vec<Vec<Value>>) -> Value {
        map(vec![(
            "ni8",
            Value::Array(pages.into_iter().map(Value::Array).collect()),
        )])
    }

    fn page() -> Vec<Value> {
        vec![
            slot(10, "Cutoff", Some("Filter")),
            slot(11, "Reso", None),
            slot(15, "Env Amt", None),
            empty_slot(),
            slot(20, "Attack", Some("Amp")),
            slot(21, "Decay", None),
            slot(22, "Sustain", None),
            slot(23, "Release", None),
        ]
    }

    fn preset(pages: Vec<Vec<Value>>) -> Vec<u8> {
        riff(&[
            (b"NISI", body(&nisi())),
            (b"NICA", body(&nica(pages))),
            (b"PLID", body(&plid())),
            (b"PCHK", vec![1, 0, 0, 0, 0x85, 0xa1, 0x78]),
        ])
    }

    #[test]
    fn a_complete_preset_reads_back() {
        let p = Preset::parse(&preset(vec![page()])).unwrap();
        assert_eq!(p.info.name, "Circuits");
        assert_eq!(p.info.author, "Jamil Samad");
        assert_eq!(p.info.bankchain, ["Super 8 R2"]);
        assert_eq!(p.info.types, [["Bass", "Synth"]]);
        assert_eq!(p.info.device_type, "INST");
        assert_eq!(p.info.uuid, None);
        assert_eq!(p.plugin.vst_magic_fourcc().as_deref(), Some("NIS8"));
        assert_eq!(p.pages.len(), 1);
        assert_eq!(p.pages[0].slots.len(), SLOTS_PER_PAGE);
        assert_eq!(p.pages[0].slots[0].as_ref().unwrap().name, "Cutoff");
    }

    #[test]
    fn plugin_state_is_measured_never_decoded() {
        // A five-entry map cut off after one key; decoding PCHK would fail.
        let p = Preset::parse(&preset(vec![page()])).unwrap();
        assert_eq!(p.state_len, 3);
    }

    #[test]
    fn a_slot_without_an_id_is_an_unassigned_knob() {
        let p = Preset::parse(&preset(vec![page()])).unwrap();
        assert!(p.pages[0].slots[3].is_none());
    }

    #[test]
    fn sections_open_at_each_header_and_keep_empty_positions() {
        let p = Preset::parse(&preset(vec![page()])).unwrap();
        let sections = p.pages[0].sections();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "Filter");
        assert_eq!(sections[0].slots.len(), 4);
        assert!(sections[0].slots[3].is_none());
        assert_eq!(sections[1].name, "Amp");
        let names: Vec<_> = sections[1]
            .slots
            .iter()
            .map(|s| s.unwrap().name.as_str())
            .collect();
        assert_eq!(names, ["Attack", "Decay", "Sustain", "Release"]);
    }

    #[test]
    fn slots_before_the_first_header_form_an_unnamed_section() {
        let pg = Page {
            slots: vec![
                None,
                Some(Slot {
                    id: 1,
                    name: "Vol".into(),
                    section: None,
                    autoname: false,
                    vflag: false,
                }),
                Some(Slot {
                    id: 2,
                    name: "Pan".into(),
                    section: Some("Mix".into()),
                    autoname: false,
                    vflag: false,
                }),
            ],
        };
        let sections = pg.sections();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "");
        assert_eq!(sections[0].slots.len(), 2);
        assert_eq!(sections[1].name, "Mix");
        assert!(!pg.is_empty());
        assert!(
            Page {
                slots: vec![None; 8]
            }
            .is_empty()
        );
    }

    #[test]
    fn an_empty_nica_map_means_no_pages() {
        let bytes = riff(&[
            (b"NISI", body(&nisi())),
            (b"NICA", body(&map(vec![]))),
            (b"PLID", body(&plid())),
        ]);
        assert!(Preset::parse(&bytes).unwrap().pages.is_empty());
    }

    #[test]
    fn a_vst3_plugin_id_reads_with_its_names() {
        // Shaped like Massive X.
        let id = map(vec![
            ("VST.magic", Value::from(0u32)),
            (
                "VST3.uid",
                Value::Array(
                    [
                        1_448_301_646u32,
                        1_763_985_517,
                        1_634_956_137,
                        1_986_338_936,
                    ]
                    .map(Value::from)
                    .to_vec(),
                ),
            ),
            ("pluginName", s("Massive X")),
            ("pluginVendor", s("Native Instruments")),
        ]);
        let bytes = riff(&[(b"NISI", body(&nisi())), (b"PLID", body(&id))]);
        let p = Preset::parse(&bytes).unwrap();
        assert_eq!(p.plugin.vst_magic_fourcc(), None);
        assert_eq!(
            p.plugin.vst3_uid,
            Some([1_448_301_646, 1_763_985_517, 1_634_956_137, 1_986_338_936])
        );
        assert_eq!(p.plugin.name.as_deref(), Some("Massive X"));
    }

    #[test]
    fn non_nks_input_is_rejected() {
        assert!(matches!(Preset::parse(b""), Err(NksError::NotNks)));
        assert!(matches!(
            Preset::parse(b"RIFF\0\0\0\0WAVE"),
            Err(NksError::NotNks)
        ));
    }

    #[test]
    fn a_header_longer_than_the_file_is_truncated() {
        let mut bytes = preset(vec![page()]);
        bytes.truncate(bytes.len() - 10);
        assert!(matches!(Preset::parse(&bytes), Err(NksError::Truncated(_))));
    }

    #[test]
    fn a_chunk_longer_than_the_file_is_truncated() {
        let mut bytes = riff(&[(b"NISI", body(&nisi()))]);
        // Inflate the NISI length past the end.
        bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(Preset::parse(&bytes), Err(NksError::Truncated(_))));
    }

    #[test]
    fn missing_metadata_is_reported() {
        let bytes = riff(&[(b"PLID", body(&plid()))]);
        assert!(matches!(
            Preset::parse(&bytes),
            Err(NksError::Missing("NISI"))
        ));
        let bytes = riff(&[(b"NISI", body(&nisi()))]);
        assert!(matches!(
            Preset::parse(&bytes),
            Err(NksError::Missing("PLID"))
        ));
    }

    #[test]
    fn a_chunk_without_a_version_word_is_rejected() {
        let bytes = riff(&[(b"NISI", vec![1, 0])]);
        assert!(matches!(Preset::parse(&bytes), Err(NksError::NoVersion(c)) if c == "NISI"));
    }

    #[test]
    fn bad_msgpack_names_its_chunk() {
        // A five-entry map with no entries.
        let bytes = riff(&[(b"NISI", vec![1, 0, 0, 0, 0x85])]);
        assert!(
            matches!(Preset::parse(&bytes), Err(NksError::Msgpack { chunk, .. }) if chunk == "NISI")
        );
    }

    #[test]
    fn a_non_integer_parameter_id_is_a_shape_error() {
        let bad = map(vec![("id", s("ten")), ("name", s("Cutoff"))]);
        let err = Preset::parse(&preset(vec![vec![bad]])).unwrap_err();
        assert!(matches!(&err, NksError::Shape(m) if m.contains("page 0 slot 0")));
    }
}
