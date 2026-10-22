//! What the frame saves in a DAW project.
//!
//! Our own small header, then the instrument's state **as opaque bytes**:
//!
//! ```text
//! "TACTSHIM"  u32 version
//! u32 len, preset path (UTF-8, may be empty)
//! u32 len, plugin bundle path (UTF-8)
//! 16 bytes   VST3 class id
//! u32 len, the instrument's own state
//! ```
//!
//! All integers little-endian. Nothing inside the instrument's state is read.

use std::path::PathBuf;

const MAGIC: &[u8; 8] = b"TACTSHIM";
const VERSION: u32 = 1;
/// Largest field accepted when reading. Real states are well under a megabyte.
const MAX_FIELD: usize = 64 * 1024 * 1024;

/// The frame's saved state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedState {
    /// The NKS preset the instrument was loaded from, if any.
    pub preset: Option<PathBuf>,
    /// The instrument's `.vst3` bundle.
    pub bundle: PathBuf,
    /// The instrument's class id.
    pub cid: [u8; 16],
    /// The instrument's own state, opaque.
    pub plugin_state: Vec<u8>,
}

impl SavedState {
    /// Serialise.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.plugin_state.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        let field = |out: &mut Vec<u8>, b: &[u8]| {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        };
        let preset = self
            .preset
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        field(&mut out, preset.as_bytes());
        field(&mut out, self.bundle.to_string_lossy().as_bytes());
        out.extend_from_slice(&self.cid);
        field(&mut out, &self.plugin_state);
        out
    }

    /// Parse. `None` for anything that is not a state this frame wrote.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader(bytes);
        if r.take(8)? != MAGIC || r.u32()? != VERSION {
            return None;
        }
        let preset = String::from_utf8(r.field()?.to_vec()).ok()?;
        let bundle = String::from_utf8(r.field()?.to_vec()).ok()?;
        let cid: [u8; 16] = r.take(16)?.try_into().ok()?;
        let plugin_state = r.field()?.to_vec();
        Some(Self {
            preset: (!preset.is_empty()).then(|| PathBuf::from(preset)),
            bundle: PathBuf::from(bundle),
            cid,
            plugin_state,
        })
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn field(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > MAX_FIELD {
            return None;
        }
        self.take(n)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sample() -> SavedState {
        SavedState {
            preset: Some("/p/Wob Wolfer.nksf".into()),
            bundle: "/Library/Audio/Plug-Ins/VST3/Massive X.vst3".into(),
            cid: [7; 16],
            plugin_state: vec![0xde, 0xad, 0xbe, 0xef],
        }
    }

    #[test]
    fn a_state_round_trips() {
        let s = sample();
        assert_eq!(SavedState::from_bytes(&s.to_bytes()), Some(s));
        let none = SavedState {
            preset: None,
            ..sample()
        };
        assert_eq!(SavedState::from_bytes(&none.to_bytes()), Some(none));
    }

    #[test]
    fn foreign_or_damaged_bytes_are_refused() {
        assert_eq!(SavedState::from_bytes(b""), None);
        assert_eq!(SavedState::from_bytes(b"NOTSHIMS\x01\0\0\0"), None);
        let mut b = sample().to_bytes();
        b.truncate(b.len() - 1);
        assert_eq!(SavedState::from_bytes(&b), None, "truncated");
        let mut huge = MAGIC.to_vec();
        huge.extend_from_slice(&VERSION.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(SavedState::from_bytes(&huge), None, "absurd length");
    }
}
