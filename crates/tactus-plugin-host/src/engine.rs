//! The real-time audio engine.
//!
//! Rules on the audio thread, and how they are kept:
//!
//! - **No allocation.** Bus buffers, the pointer arrays the plugin reads them
//!   through, the note list and the parameter queues are all built in
//!   [`Engine::new`]. `render` only overwrites them.
//! - **No locks.** The engine is owned by one thread at a time ([`Audio`] is
//!   moved into the audio callback). The note list and parameter queues the
//!   plugin reads are touched only by that thread, during `process`.
//! - **No waiting.** Stopping is a handshake on one atomic. The audio side
//!   never spins or sleeps; only the thread that stops it does.
//!
//! When more notes or parameter changes arrive in one block than were
//! preallocated, the excess is dropped rather than allocated for.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use vst3::Steinberg::Vst::BusDirections_::{kInput, kOutput};
use vst3::Steinberg::Vst::Event_::EventFlags_::kIsLive;
use vst3::Steinberg::Vst::Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent, kPolyPressureEvent};
use vst3::Steinberg::Vst::MediaTypes_::kAudio;
use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::kTempoValid;
use vst3::Steinberg::Vst::ProcessModes_::kRealtime;
use vst3::Steinberg::Vst::SymbolicSampleSizes_::kSample32;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};

use crate::{HostError, check};

/// Most notes (and pressure events) delivered in one block.
pub const MAX_EVENTS: usize = 512;
/// Most distinct parameters changed in one block.
pub const MAX_PARAM_CHANGES: usize = 64;
/// Most points per parameter in one block.
const MAX_POINTS: usize = 16;

/// Engine states, shared between the plugin and the audio thread.
pub(crate) const STOPPED: u8 = 0;
pub(crate) const IDLE: u8 = 1;
pub(crate) const RENDERING: u8 = 2;

/// Something for the audio engine to act on in the next block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    /// A key went down. Velocity 1-127.
    NoteOn {
        /// MIDI note number.
        pitch: u8,
        /// MIDI velocity.
        velocity: u8,
    },
    /// A key came up.
    NoteOff {
        /// MIDI note number.
        pitch: u8,
        /// MIDI release velocity.
        velocity: u8,
    },
    /// Pressure on one held key.
    PolyPressure {
        /// MIDI note number.
        pitch: u8,
        /// Pressure, 0-127.
        pressure: u8,
    },
    /// A parameter moved, for example a knob on the keyboard.
    Param {
        /// Parameter id.
        id: u32,
        /// Normalised value.
        value: f64,
    },
}

/// VST3's controller number for channel pressure (aftertouch).
pub const AFTERTOUCH: u16 = 128;
/// VST3's controller number for pitch bend.
pub const PITCH_BEND: u16 = 129;

/// Which parameter each MIDI controller drives, as the plugin reports it.
/// Keys are CC numbers 0-127, [`AFTERTOUCH`] and [`PITCH_BEND`].
pub type MidiMap = HashMap<u16, u32>;

/// Translate one MIDI message into a plugin input.
///
/// Notes and poly pressure become events. Controllers, channel pressure and
/// pitch bend become parameter changes through `map`; a controller the plugin
/// does not map is dropped. Every channel is treated alike.
pub fn midi_to_input(bytes: &[u8], map: &MidiMap) -> Option<Input> {
    let param = |ctrl: u16, value: f64| {
        map.get(&ctrl).map(|&id| Input::Param {
            id,
            value: value.clamp(0.0, 1.0),
        })
    };
    match *bytes {
        [s, pitch, velocity] if s & 0xF0 == 0x90 && velocity > 0 => {
            Some(Input::NoteOn { pitch, velocity })
        }
        [s, pitch, velocity] if s & 0xF0 == 0x90 || s & 0xF0 == 0x80 => {
            Some(Input::NoteOff { pitch, velocity })
        }
        [s, pitch, pressure] if s & 0xF0 == 0xA0 => Some(Input::PolyPressure { pitch, pressure }),
        [s, cc, value] if s & 0xF0 == 0xB0 && cc < 128 => {
            param(u16::from(cc), f64::from(value) / 127.0)
        }
        [s, value, ..] if s & 0xF0 == 0xD0 => param(AFTERTOUCH, f64::from(value) / 127.0),
        [s, lsb, msb] if s & 0xF0 == 0xE0 => {
            let bend = (u16::from(msb & 0x7F) << 7) | u16::from(lsb & 0x7F);
            param(PITCH_BEND, f64::from(bend) / 16383.0)
        }
        _ => None,
    }
}

fn note_event(kind: u32, pitch: u8, velocity: u8) -> Event {
    // SAFETY: Event is plain data; every field used is set below.
    let mut ev: Event = unsafe { std::mem::zeroed() };
    ev.flags = kIsLive as uint16;
    ev.r#type = kind as uint16;
    let v = f32::from(velocity) / 127.0;
    if kind == kNoteOnEvent {
        ev.__field0.noteOn = NoteOnEvent {
            channel: 0,
            pitch: i16::from(pitch),
            tuning: 0.0,
            velocity: v,
            length: 0,
            noteId: -1,
        };
    } else {
        ev.__field0.noteOff = NoteOffEvent {
            channel: 0,
            pitch: i16::from(pitch),
            velocity: v,
            noteId: -1,
            tuning: 0.0,
        };
    }
    ev
}

fn pressure_event(pitch: u8, pressure: u8) -> Event {
    // SAFETY: Event is plain data; every field used is set below.
    let mut ev: Event = unsafe { std::mem::zeroed() };
    ev.flags = kIsLive as uint16;
    ev.r#type = kPolyPressureEvent as uint16;
    ev.__field0.polyPressure = PolyPressureEvent {
        channel: 0,
        pitch: i16::from(pitch),
        pressure: f32::from(pressure) / 127.0,
        noteId: -1,
    };
    ev
}

/// A fixed-capacity note list.
///
/// SAFETY of the `Sync` impl: touched only by the thread that owns the
/// engine, and by the plugin during `process`, which runs on that thread.
pub(crate) struct RtEventList {
    events: UnsafeCell<Vec<Event>>,
}

unsafe impl Send for RtEventList {}
unsafe impl Sync for RtEventList {}

impl RtEventList {
    fn new() -> Self {
        Self {
            events: UnsafeCell::new(Vec::with_capacity(MAX_EVENTS)),
        }
    }

    #[allow(clippy::mut_from_ref)]
    fn list(&self) -> &mut Vec<Event> {
        // SAFETY: single-thread access, see the type's docs.
        unsafe { &mut *self.events.get() }
    }

    fn clear(&self) {
        self.list().clear();
    }

    /// Add an event; drop it if the list is full rather than grow.
    fn push(&self, ev: Event) {
        let list = self.list();
        if list.len() < list.capacity() {
            list.push(ev);
        }
    }
}

impl Class for RtEventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for RtEventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.list().len() as int32
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        let Some(out) = (unsafe { e.as_mut() }) else {
            return kInvalidArgument;
        };
        match usize::try_from(index).ok().and_then(|i| self.list().get(i)) {
            Some(ev) => {
                *out = *ev;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        match unsafe { e.as_ref() } {
            Some(ev) => {
                self.push(*ev);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
}

/// A queue's contents: parameter id, points used, the points.
type QueueState = (ParamID, usize, [(int32, ParamValue); MAX_POINTS]);

/// One parameter's points in a block, fixed capacity.
pub(crate) struct RtQueue {
    state: UnsafeCell<QueueState>,
}

unsafe impl Send for RtQueue {}
unsafe impl Sync for RtQueue {}

impl RtQueue {
    fn new() -> Self {
        Self {
            state: UnsafeCell::new((0, 0, [(0, 0.0); MAX_POINTS])),
        }
    }

    #[allow(clippy::mut_from_ref)]
    fn get(&self) -> &mut QueueState {
        // SAFETY: single-thread access, as for RtEventList.
        unsafe { &mut *self.state.get() }
    }

    fn reset(&self, id: ParamID) {
        let s = self.get();
        s.0 = id;
        s.1 = 0;
    }

    fn add(&self, offset: int32, value: ParamValue) -> Option<usize> {
        let s = self.get();
        let i = s.1;
        let slot = s.2.get_mut(i)?;
        *slot = (offset, value);
        s.1 += 1;
        Some(i)
    }
}

impl Class for RtQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for RtQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.get().0
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.get().1 as int32
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        let s = self.get();
        let Some(&(o, v)) = usize::try_from(index)
            .ok()
            .filter(|&i| i < s.1)
            .and_then(|i| s.2.get(i))
        else {
            return kInvalidArgument;
        };
        // SAFETY: optional out-parameters.
        unsafe {
            if let Some(p) = offset.as_mut() {
                *p = o;
            }
            if let Some(p) = value.as_mut() {
                *p = v;
            }
        }
        kResultOk
    }

    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        match self.add(offset, value) {
            Some(i) => {
                // SAFETY: optional out-parameter.
                if let Some(p) = unsafe { index.as_mut() } {
                    *p = i as int32;
                }
                kResultOk
            }
            None => kResultFalse,
        }
    }
}

/// A block's parameter changes: a fixed pool of queues, `used` of them live.
pub(crate) struct RtChanges {
    queues: Vec<ComWrapper<RtQueue>>,
    ptrs: Vec<ComPtr<IParamValueQueue>>,
    used: UnsafeCell<usize>,
}

unsafe impl Send for RtChanges {}
unsafe impl Sync for RtChanges {}

impl RtChanges {
    fn new() -> Option<Self> {
        let queues: Vec<_> = (0..MAX_PARAM_CHANGES)
            .map(|_| ComWrapper::new(RtQueue::new()))
            .collect();
        let ptrs = queues
            .iter()
            .map(|q| q.to_com_ptr::<IParamValueQueue>())
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            queues,
            ptrs,
            used: UnsafeCell::new(0),
        })
    }

    #[allow(clippy::mut_from_ref)]
    fn used(&self) -> &mut usize {
        // SAFETY: single-thread access, as for RtEventList.
        unsafe { &mut *self.used.get() }
    }

    fn clear(&self) {
        *self.used() = 0;
    }

    fn is_empty(&self) -> bool {
        *self.used() == 0
    }

    /// The live queue for `id`, opening one from the pool if needed.
    fn queue(&self, id: ParamID) -> Option<usize> {
        let used = *self.used();
        if let Some(i) = (0..used).find(|&i| self.queues.get(i).is_some_and(|q| q.get().0 == id)) {
            return Some(i);
        }
        let q = self.queues.get(used)?;
        q.reset(id);
        *self.used() = used + 1;
        Some(used)
    }

    fn set(&self, id: ParamID, value: ParamValue) {
        if let Some(q) = self.queue(id).and_then(|i| self.queues.get(i)) {
            q.add(0, value);
        }
    }
}

impl Class for RtChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for RtChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        *self.used() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        usize::try_from(index)
            .ok()
            .filter(|&i| i < *self.used())
            .and_then(|i| self.ptrs.get(i))
            .map_or(std::ptr::null_mut(), |p| p.as_ptr())
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        let Some(&id) = (unsafe { id.as_ref() }) else {
            return std::ptr::null_mut();
        };
        match self.queue(id) {
            Some(i) => {
                // SAFETY: optional out-parameter.
                if let Some(p) = unsafe { index.as_mut() } {
                    *p = i as int32;
                }
                self.ptrs
                    .get(i)
                    .map_or(std::ptr::null_mut(), |p| p.as_ptr())
            }
            None => std::ptr::null_mut(),
        }
    }
}

/// The audio side of a running plugin. Everything `render` touches lives here
/// and is allocated once.
pub(crate) struct Engine {
    processor: ManuallyDrop<ComPtr<IAudioProcessor>>,
    pub(crate) state: Arc<AtomicU8>,
    pub(crate) sample_rate: f64,
    max_block: usize,
    // Sample storage. The heap buffers never move: nothing resizes them.
    _inputs: Vec<Vec<Vec<f32>>>,
    outputs: Vec<Vec<Vec<f32>>>,
    // The plugin reads the storage through these.
    _in_ptrs: Vec<Vec<*mut f32>>,
    _out_ptrs: Vec<Vec<*mut f32>>,
    in_bufs: Vec<AudioBusBuffers>,
    out_bufs: Vec<AudioBusBuffers>,
    events: ComWrapper<RtEventList>,
    events_ptr: ManuallyDrop<ComPtr<IEventList>>,
    changes: ComWrapper<RtChanges>,
    changes_ptr: ManuallyDrop<ComPtr<IParameterChanges>>,
    context: ProcessContext,
    /// Interleaved stereo scratch for `Plugin::process_block`.
    pub(crate) scratch: Vec<f32>,
}

// SAFETY: the raw pointers point into buffers this struct owns, and the COM
// objects it holds are only used by whichever single thread owns it.
unsafe impl Send for Engine {}

impl Engine {
    /// Set up processing and build every buffer. The caller activates the
    /// component and switches processing on.
    pub(crate) fn new(
        component: &ComPtr<IComponent>,
        processor: ComPtr<IAudioProcessor>,
        sample_rate: f64,
        max_block: usize,
    ) -> Result<Self, HostError> {
        let mut setup = ProcessSetup {
            processMode: kRealtime as int32,
            symbolicSampleSize: kSample32 as int32,
            maxSamplesPerBlock: max_block as int32,
            sampleRate: sample_rate,
        };
        // SAFETY: plain call with a valid in-parameter.
        check("setupProcessing", unsafe {
            processor.setupProcessing(&mut setup)
        })?;

        let buses = |dir| -> Vec<Vec<Vec<f32>>> {
            // SAFETY: bus queries into a zeroed POD struct.
            let n = unsafe { component.getBusCount(kAudio as MediaType, dir) };
            (0..n)
                .map(|i| {
                    let mut info: BusInfo = unsafe { std::mem::zeroed() };
                    unsafe { component.getBusInfo(kAudio as MediaType, dir, i, &mut info) };
                    let channels = usize::try_from(info.channelCount).unwrap_or(0);
                    vec![vec![0.0f32; max_block]; channels]
                })
                .collect()
        };
        let mut inputs = buses(kInput as BusDirection);
        let mut outputs = buses(kOutput as BusDirection);
        let ptrs = |storage: &mut Vec<Vec<Vec<f32>>>| -> Vec<Vec<*mut f32>> {
            storage
                .iter_mut()
                .map(|bus| bus.iter_mut().map(|ch| ch.as_mut_ptr()).collect())
                .collect()
        };
        let mut in_ptrs = ptrs(&mut inputs);
        let mut out_ptrs = ptrs(&mut outputs);
        let bufs = |p: &mut Vec<Vec<*mut f32>>| -> Vec<AudioBusBuffers> {
            p.iter_mut()
                .map(|bus| AudioBusBuffers {
                    numChannels: bus.len() as int32,
                    silenceFlags: 0,
                    __field0: AudioBusBuffers__type0 {
                        channelBuffers32: bus.as_mut_ptr(),
                    },
                })
                .collect()
        };
        let in_bufs = bufs(&mut in_ptrs);
        let out_bufs = bufs(&mut out_ptrs);

        let no_lists = || HostError::Call {
            call: "event lists",
            code: kNoInterface,
        };
        let events = ComWrapper::new(RtEventList::new());
        let changes = ComWrapper::new(RtChanges::new().ok_or_else(no_lists)?);
        let events_ptr = events.to_com_ptr::<IEventList>().ok_or_else(no_lists)?;
        let changes_ptr = changes
            .to_com_ptr::<IParameterChanges>()
            .ok_or_else(no_lists)?;

        // SAFETY: ProcessContext is plain data.
        let mut context: ProcessContext = unsafe { std::mem::zeroed() };
        context.state = kTempoValid;
        context.sampleRate = sample_rate;
        context.tempo = 120.0;
        context.timeSigNumerator = 4;
        context.timeSigDenominator = 4;

        Ok(Self {
            processor: ManuallyDrop::new(processor),
            state: Arc::new(AtomicU8::new(IDLE)),
            sample_rate,
            max_block,
            _inputs: inputs,
            outputs,
            _in_ptrs: in_ptrs,
            _out_ptrs: out_ptrs,
            in_bufs,
            out_bufs,
            events,
            events_ptr: ManuallyDrop::new(events_ptr),
            changes,
            changes_ptr: ManuallyDrop::new(changes_ptr),
            context,
            scratch: vec![0.0; max_block * 2],
        })
    }

    /// Fill `out` (interleaved, `channels` wide) from the main output bus,
    /// applying `inputs` at the start. Returns the peak level.
    ///
    /// Real-time safe: no allocation, no locks. Writes silence once the
    /// plugin has been stopped.
    pub(crate) fn render(&mut self, out: &mut [f32], channels: usize, inputs: &[Input]) -> f32 {
        out.fill(0.0);
        let channels = channels.max(1);
        self.guarded(Out::Interleaved(out, channels), inputs)
    }

    /// As [`render`](Self::render), into separate channel buffers of equal
    /// length, as a DAW hands them to a plugin.
    pub(crate) fn render_planar(&mut self, outs: &mut [&mut [f32]], inputs: &[Input]) -> f32 {
        for ch in outs.iter_mut() {
            ch.fill(0.0);
        }
        self.guarded(Out::Planar(outs), inputs)
    }

    fn guarded(&mut self, out: Out<'_, '_>, inputs: &[Input]) -> f32 {
        if self
            .state
            .compare_exchange(IDLE, RENDERING, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return 0.0;
        }
        let peak = self.render_inner(out, inputs);
        self.state.store(IDLE, Ordering::Release);
        peak
    }

    fn render_inner(&mut self, mut out: Out<'_, '_>, inputs: &[Input]) -> f32 {
        let total = out.frames();
        let mut done = 0;
        let mut peak = 0.0f32;
        let mut pending = inputs;
        while done < total {
            let frames = (total - done).min(self.max_block);
            self.events.clear();
            self.changes.clear();
            // Inputs land at the start of the first chunk.
            for input in pending {
                match *input {
                    Input::NoteOn { pitch, velocity } => {
                        self.events.push(note_event(kNoteOnEvent, pitch, velocity));
                    }
                    Input::NoteOff { pitch, velocity } => {
                        self.events.push(note_event(kNoteOffEvent, pitch, velocity));
                    }
                    Input::PolyPressure { pitch, pressure } => {
                        self.events.push(pressure_event(pitch, pressure));
                    }
                    Input::Param { id, value } => self.changes.set(id, value),
                }
            }
            pending = &[];
            for bus in &mut self.outputs {
                for ch in bus.iter_mut() {
                    if let Some(s) = ch.get_mut(..frames) {
                        s.fill(0.0);
                    }
                }
            }
            let mut data = ProcessData {
                processMode: kRealtime as int32,
                symbolicSampleSize: kSample32 as int32,
                numSamples: frames as int32,
                numInputs: self.in_bufs.len() as int32,
                numOutputs: self.out_bufs.len() as int32,
                inputs: if self.in_bufs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    self.in_bufs.as_mut_ptr()
                },
                outputs: if self.out_bufs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    self.out_bufs.as_mut_ptr()
                },
                inputParameterChanges: if self.changes.is_empty() {
                    std::ptr::null_mut()
                } else {
                    self.changes_ptr.as_ptr()
                },
                outputParameterChanges: std::ptr::null_mut(),
                inputEvents: self.events_ptr.as_ptr(),
                outputEvents: std::ptr::null_mut(),
                processContext: &mut self.context,
            };
            // SAFETY: every pointer refers to storage this engine owns, sized
            // for max_block, and frames <= max_block.
            let _ = unsafe { self.processor.process(&mut data) };

            if let Some(main) = self.outputs.first().filter(|b| !b.is_empty()) {
                let channels = out.channels();
                for f in 0..frames {
                    for c in 0..channels {
                        let sample = main
                            .get(c % main.len())
                            .and_then(|ch| ch.get(f))
                            .copied()
                            .unwrap_or(0.0);
                        out.put(done + f, c, sample);
                        peak = peak.max(sample.abs());
                    }
                }
            }
            done += frames;
        }
        peak
    }
}

/// Where rendered samples go.
enum Out<'a, 'b> {
    /// One buffer, `channels` samples per frame.
    Interleaved(&'a mut [f32], usize),
    /// One buffer per channel.
    Planar(&'a mut [&'b mut [f32]]),
}

impl Out<'_, '_> {
    fn channels(&self) -> usize {
        match self {
            Self::Interleaved(_, c) => *c,
            Self::Planar(chs) => chs.len(),
        }
    }

    fn frames(&self) -> usize {
        match self {
            Self::Interleaved(b, c) => b.len() / (*c).max(1),
            Self::Planar(chs) => chs.iter().map(|c| c.len()).min().unwrap_or(0),
        }
    }

    fn put(&mut self, frame: usize, channel: usize, sample: f32) {
        let slot = match self {
            Self::Interleaved(b, c) => b.get_mut(frame * *c + channel),
            Self::Planar(chs) => chs.get_mut(channel).and_then(|ch| ch.get_mut(frame)),
        };
        if let Some(slot) = slot {
            *slot = sample;
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Once the plugin has been torn down, its code may already be
        // unloaded: releasing these references would call into it. Leak them
        // instead. While it is alive, release normally.
        if self.state.load(Ordering::Acquire) != STOPPED {
            // SAFETY: each is dropped exactly once, here.
            unsafe {
                ManuallyDrop::drop(&mut self.processor);
                ManuallyDrop::drop(&mut self.events_ptr);
                ManuallyDrop::drop(&mut self.changes_ptr);
            }
        }
    }
}

/// A running plugin's audio engine, owned by an audio thread.
///
/// Take it with [`Plugin::take_audio`](crate::Plugin::take_audio) and move it
/// into the audio callback. Dropping the [`Plugin`](crate::Plugin) first is
/// safe: the engine goes silent and never calls into the plugin again.
pub struct Audio(pub(crate) Box<Engine>);

impl Audio {
    /// Render `out`, interleaved and `channels` wide, applying `inputs` first.
    /// Returns the peak level. Real-time safe.
    pub fn render(&mut self, out: &mut [f32], channels: usize, inputs: &[Input]) -> f32 {
        self.0.render(out, channels, inputs)
    }

    /// Render into separate channel buffers (as a DAW provides them),
    /// applying `inputs` first. Returns the peak level. Real-time safe.
    pub fn render_planar(&mut self, outs: &mut [&mut [f32]], inputs: &[Input]) -> f32 {
        self.0.render_planar(outs, inputs)
    }

    /// The sample rate the engine was set up for.
    pub fn sample_rate(&self) -> f64 {
        self.0.sample_rate
    }
}

/// Stop an engine from another thread: wait for any block in flight, then
/// mark it stopped so it never processes again. Only this side waits.
pub(crate) fn stop(state: &AtomicU8) {
    loop {
        match state.compare_exchange(IDLE, STOPPED, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) | Err(STOPPED) => return,
            Err(_) => std::thread::yield_now(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn midi_becomes_notes_pressure_and_mapped_parameters() {
        let map: MidiMap = [(1, 100), (64, 101), (AFTERTOUCH, 102), (PITCH_BEND, 103)].into();
        assert_eq!(
            midi_to_input(&[0x90, 60, 100], &map),
            Some(Input::NoteOn {
                pitch: 60,
                velocity: 100
            })
        );
        assert_eq!(
            midi_to_input(&[0x90, 60, 0], &map),
            Some(Input::NoteOff {
                pitch: 60,
                velocity: 0
            })
        );
        assert_eq!(
            midi_to_input(&[0x83, 60, 40], &map),
            Some(Input::NoteOff {
                pitch: 60,
                velocity: 40
            })
        );
        assert_eq!(
            midi_to_input(&[0xA0, 60, 90], &map),
            Some(Input::PolyPressure {
                pitch: 60,
                pressure: 90
            })
        );
        assert_eq!(
            midi_to_input(&[0xB0, 1, 127], &map),
            Some(Input::Param {
                id: 100,
                value: 1.0
            })
        );
        assert_eq!(
            midi_to_input(&[0xB0, 64, 0], &map),
            Some(Input::Param {
                id: 101,
                value: 0.0
            })
        );
        assert_eq!(
            midi_to_input(&[0xB0, 7, 64], &map),
            None,
            "unmapped controller"
        );
        assert_eq!(
            midi_to_input(&[0xD0, 127], &map),
            Some(Input::Param {
                id: 102,
                value: 1.0
            })
        );
        let Some(Input::Param { id: 103, value }) = midi_to_input(&[0xE0, 0x00, 0x40], &map) else {
            panic!("pitch bend")
        };
        assert!((value - 8192.0 / 16383.0).abs() < 1e-9);
        assert_eq!(midi_to_input(&[0xF8], &map), None, "clock is ignored");
    }

    #[test]
    fn the_note_list_drops_rather_than_grows() {
        let list = RtEventList::new();
        let cap = list.list().capacity();
        for _ in 0..cap + 10 {
            list.push(note_event(kNoteOnEvent, 60, 100));
        }
        assert_eq!(list.list().len(), cap);
        assert_eq!(list.list().capacity(), cap, "no reallocation");
    }

    #[test]
    fn parameter_changes_reuse_queues_and_cap_out() {
        let c = RtChanges::new().unwrap();
        c.set(7, 0.1);
        c.set(7, 0.2);
        c.set(9, 0.5);
        assert_eq!(
            unsafe { c.getParameterCount() },
            2,
            "same id shares a queue"
        );
        let q = &c.queues[0];
        assert_eq!(unsafe { q.getPointCount() }, 2);
        c.clear();
        assert!(c.is_empty());
        for id in 0..(MAX_PARAM_CHANGES as u32 + 5) {
            c.set(id, 0.0);
        }
        assert_eq!(unsafe { c.getParameterCount() } as usize, MAX_PARAM_CHANGES);
    }

    #[test]
    fn stopping_waits_for_a_block_in_flight_and_is_final() {
        let state = Arc::new(AtomicU8::new(RENDERING));
        let s2 = Arc::clone(&state);
        let t = std::thread::spawn(move || stop(&s2));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!t.is_finished(), "stop must wait while a block renders");
        state.store(IDLE, Ordering::Release);
        t.join().unwrap();
        assert_eq!(state.load(Ordering::Acquire), STOPPED);
        assert!(
            state
                .compare_exchange(IDLE, RENDERING, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        );
    }

    #[test]
    fn the_audio_handle_can_go_to_an_audio_thread() {
        fn send<T: Send>() {}
        send::<Audio>();
    }
}
