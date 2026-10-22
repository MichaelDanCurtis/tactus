//! COM objects the host hands to a plugin.
//!
//! A VST3 plugin calls back into its host through a few small interfaces: the
//! host's identity (`IHostApplication`), byte streams for state (`IBStream`),
//! an edit handler (`IComponentHandler`), and messages between the plugin's
//! two halves (`IMessage`, `IAttributeList`). Each is implemented here with
//! the least behaviour the plugin needs.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;

use vst3::Steinberg::IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper, Interface};

/// Write `s` into a UTF-16 buffer, NUL-terminated, truncating to fit.
pub(crate) fn write_utf16(s: &str, dst: &mut [TChar]) {
    let Some(room) = dst.len().checked_sub(1) else {
        return;
    };
    let mut n = 0;
    for (unit, slot) in s.encode_utf16().take(room).zip(dst.iter_mut()) {
        *slot = unit as TChar;
        n += 1;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

/// Read a NUL-terminated UTF-16 buffer.
pub(crate) fn read_utf16(src: &[TChar]) -> String {
    let end = src.iter().position(|&c| c == 0).unwrap_or(src.len());
    let units = src.get(..end).unwrap_or_default();
    char::decode_utf16(units.iter().copied())
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Read a NUL-terminated C string buffer.
pub(crate) fn read_c(src: &[char8]) -> String {
    let end = src.iter().position(|&c| c == 0).unwrap_or(src.len());
    let bytes: Vec<u8> = src
        .get(..end)
        .unwrap_or_default()
        .iter()
        .map(|&b| b as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn same_id(a: &TUID, b: &[u8; 16]) -> bool {
    a.iter().zip(b).all(|(&x, &y)| x as u8 == y)
}

/// The host's identity, and the factory for the message objects a plugin's
/// two halves use to talk to each other.
pub(crate) struct Host {
    pub(crate) name: String,
}

impl Class for Host {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for Host {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        // SAFETY: the plugin passes a String128 it owns, or null.
        match unsafe { name.as_mut() } {
            Some(buf) => {
                write_utf16(&self.name, buf);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: checked non-null above; cid and iid are the plugin's TUIDs.
        unsafe {
            *obj = std::ptr::null_mut();
            let (Some(cid), Some(iid)) = (cid.as_ref(), iid.as_ref()) else {
                return kInvalidArgument;
            };
            if same_id(cid, &IMessage::IID)
                && same_id(iid, &IMessage::IID)
                && let Some(p) = ComWrapper::new(Message::default()).to_com_ptr::<IMessage>()
            {
                *obj = p.into_raw().cast();
                return kResultOk;
            }
            if same_id(cid, &IAttributeList::IID)
                && same_id(iid, &IAttributeList::IID)
                && let Some(p) =
                    ComWrapper::new(AttributeList::default()).to_com_ptr::<IAttributeList>()
            {
                *obj = p.into_raw().cast();
                return kResultOk;
            }
        }
        kNoInterface
    }
}

/// A growable in-memory byte stream.
#[derive(Default)]
pub(crate) struct MemoryStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl MemoryStream {
    pub(crate) fn new(data: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new((data, 0)),
        }
    }

    pub(crate) fn rewind(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.1 = 0;
        }
    }

    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.inner.lock().map(|g| g.0.clone()).unwrap_or_default()
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, read: *mut int32) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kInternalError;
        };
        let (data, pos) = &mut *g;
        let want = usize::try_from(num_bytes).unwrap_or(0);
        let n = want.min(data.len().saturating_sub(*pos));
        if n > 0
            && !buffer.is_null()
            && let Some(src) = data.get(*pos..*pos + n)
        {
            // SAFETY: the plugin's buffer holds at least num_bytes bytes.
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast(), n) };
        }
        *pos += n;
        // SAFETY: optional out-parameter.
        if let Some(r) = unsafe { read.as_mut() } {
            *r = n as int32;
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, written: *mut int32) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kInternalError;
        };
        let (data, pos) = &mut *g;
        let n = usize::try_from(num_bytes).unwrap_or(0);
        if n > 0 && !buffer.is_null() {
            // SAFETY: the plugin's buffer holds num_bytes bytes.
            let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
            let end = *pos + n;
            if data.len() < end {
                data.resize(end, 0);
            }
            if let Some(dst) = data.get_mut(*pos..end) {
                dst.copy_from_slice(src);
            }
            *pos = end;
        }
        // SAFETY: optional out-parameter.
        if let Some(w) = unsafe { written.as_mut() } {
            *w = n as int32;
        }
        kResultOk
    }

    unsafe fn seek(&self, offset: int64, mode: int32, result: *mut int64) -> tresult {
        let Ok(mut g) = self.inner.lock() else {
            return kInternalError;
        };
        let (data, pos) = &mut *g;
        let base = match mode as u32 {
            m if m == kIBSeekSet => 0i64,
            m if m == kIBSeekCur => *pos as i64,
            m if m == kIBSeekEnd => data.len() as i64,
            _ => return kInvalidArgument,
        };
        let Some(target) = base.checked_add(offset).filter(|t| *t >= 0) else {
            return kInvalidArgument;
        };
        *pos = target as usize;
        // SAFETY: optional out-parameter.
        if let Some(r) = unsafe { result.as_mut() } {
            *r = target;
        }
        kResultOk
    }

    unsafe fn tell(&self, out: *mut int64) -> tresult {
        let Ok(g) = self.inner.lock() else {
            return kInternalError;
        };
        // SAFETY: optional out-parameter.
        match unsafe { out.as_mut() } {
            Some(p) => {
                *p = g.1 as int64;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
}

/// Receives edits the plugin makes itself, for example from its own GUI,
/// and its requests to restart (for example after a preset applies).
#[derive(Default)]
pub(crate) struct ComponentHandler {
    pub(crate) edits: Mutex<Vec<(ParamID, ParamValue)>>,
    pub(crate) restarts: Mutex<int32>,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        if let Ok(mut e) = self.edits.lock() {
            e.push((id, value));
        }
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        if let Ok(mut r) = self.restarts.lock() {
            *r |= flags;
        }
        kResultOk
    }
}

/// One attribute value.
#[derive(Clone)]
enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

/// A message's attributes: a string-keyed map.
#[derive(Default)]
pub(crate) struct AttributeList {
    map: Mutex<HashMap<String, Attr>>,
}

fn key(id: AttrID) -> Option<String> {
    if id.is_null() {
        return None;
    }
    // SAFETY: the plugin passes a NUL-terminated attribute id.
    Some(unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned())
}

impl AttributeList {
    fn put(&self, id: AttrID, v: Attr) -> tresult {
        match (key(id), self.map.lock()) {
            (Some(k), Ok(mut m)) => {
                m.insert(k, v);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    fn get(&self, id: AttrID) -> Option<Attr> {
        let k = key(id)?;
        self.map.lock().ok()?.get(&k).cloned()
    }
}

impl Class for AttributeList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        self.put(id, Attr::Int(value))
    }

    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        match (self.get(id), unsafe { value.as_mut() }) {
            (Some(Attr::Int(v)), Some(out)) => {
                *out = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        self.put(id, Attr::Float(value))
    }

    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        match (self.get(id), unsafe { value.as_mut() }) {
            (Some(Attr::Float(v)), Some(out)) => {
                *out = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut s = Vec::new();
        // SAFETY: the plugin passes a NUL-terminated UTF-16 string.
        unsafe {
            let mut p = string;
            while *p != 0 {
                s.push(*p);
                p = p.add(1);
            }
        }
        self.put(id, Attr::Str(s))
    }

    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        let Some(Attr::Str(s)) = self.get(id) else {
            return kResultFalse;
        };
        let room = size_in_bytes as usize / std::mem::size_of::<TChar>();
        if string.is_null() || room == 0 {
            return kInvalidArgument;
        }
        // SAFETY: the plugin's buffer holds size_in_bytes bytes.
        let dst = unsafe { std::slice::from_raw_parts_mut(string, room) };
        let n = s.len().min(room - 1);
        if let (Some(d), Some(src)) = (dst.get_mut(..n), s.get(..n)) {
            d.copy_from_slice(src);
        }
        if let Some(end) = dst.get_mut(n) {
            *end = 0;
        }
        kResultOk
    }

    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size_in_bytes: uint32) -> tresult {
        let bytes = if data.is_null() || size_in_bytes == 0 {
            Vec::new()
        } else {
            // SAFETY: the plugin's buffer holds size_in_bytes bytes.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size_in_bytes as usize) }
                .to_vec()
        };
        self.put(id, Attr::Bin(bytes))
    }

    unsafe fn getBinary(
        &self,
        id: AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        // The pointer handed out must stay valid while the list lives, so it
        // points into the stored value, not a copy.
        let (Ok(m), Some(k)) = (self.map.lock(), key(id)) else {
            return kInvalidArgument;
        };
        let Some(Attr::Bin(b)) = m.get(&k) else {
            return kResultFalse;
        };
        // SAFETY: optional out-parameters.
        unsafe {
            if let Some(d) = data.as_mut() {
                *d = b.as_ptr().cast();
            }
            if let Some(s) = size_in_bytes.as_mut() {
                *s = b.len() as uint32;
            }
        }
        kResultOk
    }
}

/// A message between a plugin's processor and its controller.
pub(crate) struct Message {
    id: Mutex<Vec<c_char>>,
    attributes: vst3::ComPtr<IAttributeList>,
}

impl Default for Message {
    fn default() -> Self {
        let attributes = ComWrapper::new(AttributeList::default())
            .to_com_ptr::<IAttributeList>()
            .unwrap_or_else(|| unreachable!("AttributeList implements IAttributeList"));
        Self {
            id: Mutex::new(vec![0]),
            attributes,
        }
    }
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        // The id lives as long as the message; the Vec is only replaced by
        // setMessageID.
        self.id
            .lock()
            .map(|g| g.as_ptr())
            .unwrap_or(std::ptr::null())
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        if id.is_null() {
            return;
        }
        // SAFETY: the plugin passes a NUL-terminated id.
        let bytes = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
        if let Ok(mut g) = self.id.lock() {
            *g = bytes.iter().map(|&b| b as c_char).collect();
        }
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn utf16_round_trips_and_truncates() {
        let mut buf = [0 as TChar; 8];
        write_utf16("Massive X", &mut buf);
        assert_eq!(read_utf16(&buf), "Massive");
        write_utf16("Ø", &mut buf);
        assert_eq!(read_utf16(&buf), "Ø");
    }

    #[test]
    fn a_stream_reads_writes_and_seeks() {
        let s = MemoryStream::new(b"abcdef".to_vec());
        let mut out = [0u8; 4];
        let mut n = 0;
        unsafe {
            assert_eq!(s.read(out.as_mut_ptr().cast(), 4, &mut n), kResultOk);
        }
        assert_eq!((&out[..], n), (&b"abcd"[..], 4));
        unsafe {
            assert_eq!(s.read(out.as_mut_ptr().cast(), 4, &mut n), kResultOk);
        }
        assert_eq!(n, 2, "short read at the end");
        let mut pos = 0;
        unsafe {
            assert_eq!(s.seek(-1, kIBSeekEnd as int32, &mut pos), kResultOk);
            assert_eq!(pos, 5);
            assert_eq!(s.seek(-10, kIBSeekCur as int32, &mut pos), kInvalidArgument);
            let mut w = 0;
            assert_eq!(
                s.write(b"XYZ".as_ptr() as *mut c_void, 3, &mut w),
                kResultOk
            );
        }
        assert_eq!(s.bytes(), b"abcdeXYZ");
        s.rewind();
        let mut t = -1;
        unsafe { assert_eq!(s.tell(&mut t), kResultOk) };
        assert_eq!(t, 0);
    }

    #[test]
    fn attributes_keep_their_types() {
        let a = AttributeList::default();
        let k = c"key".as_ptr();
        let mut i = 0i64;
        let mut f = 0f64;
        unsafe {
            assert_eq!(a.setInt(k, 7), kResultOk);
            assert_eq!(a.getInt(k, &mut i), kResultOk);
            assert_eq!(a.getFloat(k, &mut f), kResultFalse, "an int is not a float");
            let data = [1u8, 2, 3];
            assert_eq!(a.setBinary(k, data.as_ptr().cast(), 3), kResultOk);
            let mut p: *const c_void = std::ptr::null();
            let mut n = 0u32;
            assert_eq!(a.getBinary(k, &mut p, &mut n), kResultOk);
            assert_eq!(
                std::slice::from_raw_parts(p.cast::<u8>(), n as usize),
                &data
            );
        }
        assert_eq!(i, 7);
    }

    #[test]
    fn a_message_keeps_its_id() {
        let m = Message::default();
        unsafe {
            m.setMessageID(c"Ping".as_ptr());
            assert_eq!(CStr::from_ptr(m.getMessageID()).to_str().unwrap(), "Ping");
            assert!(!m.getAttributes().is_null());
        }
    }
}

/// Notes for one processing block.
#[derive(Default)]
pub(crate) struct EventList {
    pub(crate) events: Mutex<Vec<Event>>,
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.events.lock().map(|e| e.len() as int32).unwrap_or(0)
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        let (Ok(events), Some(out)) = (self.events.lock(), unsafe { e.as_mut() }) else {
            return kInvalidArgument;
        };
        match usize::try_from(index).ok().and_then(|i| events.get(i)) {
            Some(ev) => {
                *out = *ev;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        match (self.events.lock(), unsafe { e.as_ref() }) {
            (Ok(mut events), Some(ev)) => {
                events.push(*ev);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }
}

/// One parameter's changes within a block: (sample offset, value) points.
pub(crate) struct ParamValueQueue {
    id: ParamID,
    points: Mutex<Vec<(int32, ParamValue)>>,
}

impl Class for ParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.points.lock().map(|p| p.len() as int32).unwrap_or(0)
    }

    unsafe fn getPoint(
        &self,
        index: int32,
        sample_offset: *mut int32,
        value: *mut ParamValue,
    ) -> tresult {
        let Ok(points) = self.points.lock() else {
            return kInternalError;
        };
        let Some(&(offset, v)) = usize::try_from(index).ok().and_then(|i| points.get(i)) else {
            return kInvalidArgument;
        };
        // SAFETY: optional out-parameters.
        unsafe {
            if let Some(o) = sample_offset.as_mut() {
                *o = offset;
            }
            if let Some(o) = value.as_mut() {
                *o = v;
            }
        }
        kResultOk
    }

    unsafe fn addPoint(
        &self,
        sample_offset: int32,
        value: ParamValue,
        index: *mut int32,
    ) -> tresult {
        let Ok(mut points) = self.points.lock() else {
            return kInternalError;
        };
        points.push((sample_offset, value));
        // SAFETY: optional out-parameter.
        if let Some(i) = unsafe { index.as_mut() } {
            *i = points.len() as int32 - 1;
        }
        kResultOk
    }
}

/// Every parameter change for one block, one queue per parameter.
#[derive(Default)]
pub(crate) struct ParameterChanges {
    queues: Mutex<Vec<vst3::ComPtr<IParamValueQueue>>>,
}

impl ParameterChanges {
    /// Replace the contents with one point per parameter, at offset 0.
    pub(crate) fn set(&self, changes: &[(ParamID, ParamValue)]) {
        let Ok(mut queues) = self.queues.lock() else {
            return;
        };
        queues.clear();
        for &(id, value) in changes {
            let queue = ParamValueQueue {
                id,
                points: Mutex::new(vec![(0, value)]),
            };
            if let Some(q) = ComWrapper::new(queue).to_com_ptr::<IParamValueQueue>() {
                queues.push(q);
            }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queues.lock().map(|q| q.is_empty()).unwrap_or(true)
    }
}

impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.queues.lock().map(|q| q.len() as int32).unwrap_or(0)
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let Ok(queues) = self.queues.lock() else {
            return std::ptr::null_mut();
        };
        usize::try_from(index)
            .ok()
            .and_then(|i| queues.get(i))
            .map_or(std::ptr::null_mut(), |q| q.as_ptr())
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        // SAFETY: the plugin passes a ParamID it owns.
        let Some(&id) = (unsafe { id.as_ref() }) else {
            return std::ptr::null_mut();
        };
        let Ok(mut queues) = self.queues.lock() else {
            return std::ptr::null_mut();
        };
        let queue = ParamValueQueue {
            id,
            points: Mutex::new(Vec::new()),
        };
        let Some(q) = ComWrapper::new(queue).to_com_ptr::<IParamValueQueue>() else {
            return std::ptr::null_mut();
        };
        let ptr = q.as_ptr();
        queues.push(q);
        // SAFETY: optional out-parameter.
        if let Some(i) = unsafe { index.as_mut() } {
            *i = queues.len() as int32 - 1;
        }
        ptr
    }
}
