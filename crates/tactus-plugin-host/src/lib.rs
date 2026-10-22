//! Load VST3 instrument plugins in-process.
//!
//! The first job is the one an NKS host needs before any audio: load a plugin,
//! hand it a preset's saved state **as opaque bytes**, and read back its real
//! parameters, so the keyboard can show the plugin's own values and value
//! text. Audio processing comes next.
//!
//! The plugin handles its own licensing. Nothing here inspects, alters or
//! routes around it.
//!
//! # Lifetimes
//!
//! A [`Plugin`] borrows the [`Module`] it came from, so the library cannot be
//! unloaded while an instance is alive.

#![deny(missing_docs)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod com;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vst3::Steinberg::Vst::BusDirections_::{kInput, kOutput};
use vst3::Steinberg::Vst::Event_::EventFlags_::kIsLive;
use vst3::Steinberg::Vst::Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent, kPolyPressureEvent};
use vst3::Steinberg::Vst::MediaTypes_::{kAudio, kEvent};
use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::kTempoValid;
use vst3::Steinberg::Vst::ProcessModes_::kRealtime;
use vst3::Steinberg::Vst::SymbolicSampleSizes_::kSample32;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper, Interface};

/// What went wrong talking to a plugin.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The bundle or its binary could not be loaded.
    #[error("{path}: {reason}")]
    Load {
        /// The bundle.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// The binary exports no usable factory.
    #[error("the plugin exports no factory")]
    NoFactory,
    /// The factory has no class with this id.
    #[error("no audio class {0} in this plugin")]
    NoClass(String),
    /// A plugin call returned an error code.
    #[error("{call} returned {code}")]
    Call {
        /// The VST3 method.
        call: &'static str,
        /// Its `tresult`.
        code: tresult,
    },
    /// The plugin has no edit controller to read parameters from.
    #[error("the plugin has no edit controller")]
    NoController,
}

fn check(call: &'static str, code: tresult) -> Result<(), HostError> {
    if code == kResultOk {
        Ok(())
    } else {
        Err(HostError::Call { call, code })
    }
}

/// A class the plugin's factory can make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassInfo {
    /// Class id.
    pub cid: [u8; 16],
    /// Category, e.g. `Audio Module Class`.
    pub category: String,
    /// Display name.
    pub name: String,
}

/// The category of a plugin's audio processor class.
pub const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

/// A VST3 class id from the four 32-bit words NKS presets store (`VST3.uid`).
pub fn class_id(words: [u32; 4]) -> [u8; 16] {
    vst3::uid(words[0], words[1], words[2], words[3]).map(|b| b as u8)
}

/// A loaded plugin binary.
pub struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    #[cfg(target_os = "macos")]
    exit: Option<libloading::os::unix::Symbol<unsafe extern "C" fn() -> bool>>,
    // Dropped after the factory and bundleExit, in declaration order.
    _lib: libloading::Library,
    #[cfg(target_os = "macos")]
    _bundle: core_foundation::bundle::CFBundle,
}

impl Module {
    /// Load a `.vst3` bundle.
    #[cfg(target_os = "macos")]
    pub fn load(bundle: &Path) -> Result<Self, HostError> {
        use core_foundation::base::TCFType;
        use core_foundation::bundle::CFBundle;
        use core_foundation::url::CFURL;

        let fail = |reason: &str| HostError::Load {
            path: bundle.to_owned(),
            reason: reason.to_owned(),
        };
        let url = CFURL::from_path(bundle, true).ok_or_else(|| fail("not a path"))?;
        let cf = CFBundle::new(url).ok_or_else(|| fail("not a bundle"))?;
        let exe = cf
            .executable_url()
            .and_then(|u| u.to_path())
            .ok_or_else(|| fail("bundle has no executable"))?;
        // SAFETY: loading a plugin runs its initialisers. That is the point
        // of hosting it; the user chose to run this plugin.
        let lib = unsafe { libloading::Library::new(&exe) }.map_err(|e| fail(&e.to_string()))?;

        type Entry = unsafe extern "C" fn(*mut std::ffi::c_void) -> bool;
        type Exit = unsafe extern "C" fn() -> bool;
        type Factory = unsafe extern "system" fn() -> *mut IPluginFactory;
        // SAFETY: symbol types are the VST3 module entry points.
        let (entry, exit, get_factory) = unsafe {
            (
                lib.get::<Entry>(b"bundleEntry").ok().map(|s| s.into_raw()),
                lib.get::<Exit>(b"bundleExit").ok().map(|s| s.into_raw()),
                lib.get::<Factory>(b"GetPluginFactory")
                    .map_err(|e| fail(&e.to_string()))?
                    .into_raw(),
            )
        };
        if let Some(entry) = &entry {
            // SAFETY: bundleEntry takes the plugin's own CFBundleRef.
            let ok = unsafe { entry(cf.as_concrete_TypeRef() as *mut std::ffi::c_void) };
            if !ok {
                return Err(fail("bundleEntry refused"));
            }
        }
        // SAFETY: GetPluginFactory returns an owned reference, or null.
        let factory = unsafe { ComPtr::from_raw(get_factory()) }.ok_or(HostError::NoFactory)?;
        Ok(Self {
            factory: Some(factory),
            exit,
            _lib: lib,
            _bundle: cf,
        })
    }

    fn factory(&self) -> Result<&ComPtr<IPluginFactory>, HostError> {
        self.factory.as_ref().ok_or(HostError::NoFactory)
    }

    /// Every class the factory can make.
    pub fn classes(&self) -> Result<Vec<ClassInfo>, HostError> {
        let f = self.factory()?;
        // SAFETY: plain factory queries into a zeroed POD struct.
        let n = unsafe { f.countClasses() };
        Ok((0..n)
            .filter_map(|i| {
                let mut info: PClassInfo = unsafe { std::mem::zeroed() };
                (unsafe { f.getClassInfo(i, &mut info) } == kResultOk).then(|| ClassInfo {
                    cid: info.cid.map(|b| b as u8),
                    category: com::read_c(&info.category),
                    name: com::read_c(&info.name),
                })
            })
            .collect())
    }

    fn create<I: Interface>(&self, cid: &[u8; 16]) -> Result<ComPtr<I>, HostError> {
        let f = self.factory()?;
        let mut obj = std::ptr::null_mut();
        // SAFETY: cid and iid are 16-byte ids; obj receives an owned pointer.
        let code = unsafe {
            f.createInstance(
                cid.as_ptr() as FIDString,
                I::IID.as_ptr() as FIDString,
                &mut obj,
            )
        };
        check("createInstance", code)?;
        // SAFETY: on success obj is an owned pointer of the requested type.
        unsafe { ComPtr::from_raw(obj.cast::<I>()) }.ok_or(HostError::Call {
            call: "createInstance",
            code,
        })
    }

    /// Make and initialise one instance of an audio class.
    pub fn instantiate(&self, cid: &[u8; 16]) -> Result<Plugin<'_>, HostError> {
        let host = ComWrapper::new(com::Host {
            name: "tactus".into(),
        });
        let host_ptr = host
            .to_com_ptr::<IHostApplication>()
            .ok_or(HostError::NoFactory)?;
        let context = host_ptr.as_ptr().cast::<FUnknown>();

        let component = self.create::<IComponent>(cid)?;
        // SAFETY: the host context outlives the plugin; it is held by Plugin.
        check("IComponent::initialize", unsafe {
            component.initialize(context)
        })?;

        // A single-object plugin implements both interfaces on one object.
        let (controller, separate) = match component.cast::<IEditController>() {
            Some(c) => (c, false),
            None => {
                let mut tuid: TUID = [0; 16];
                // SAFETY: out-parameter TUID.
                check("getControllerClassId", unsafe {
                    component.getControllerClassId(&mut tuid)
                })?;
                let ctrl = self
                    .create::<IEditController>(&tuid.map(|b| b as u8))
                    .map_err(|_| HostError::NoController)?;
                check("IEditController::initialize", unsafe {
                    ctrl.initialize(context)
                })?;
                (ctrl, true)
            }
        };

        let handler = ComWrapper::new(com::ComponentHandler::default());
        if let Some(h) = handler.to_com_ptr::<IComponentHandler>() {
            // SAFETY: the handler outlives the controller; Plugin holds both.
            unsafe { controller.setComponentHandler(h.as_ptr()) };
        }

        // The processor and controller talk through connection points.
        let points = if separate {
            match (
                component.cast::<IConnectionPoint>(),
                controller.cast::<IConnectionPoint>(),
            ) {
                (Some(a), Some(b)) => {
                    // SAFETY: both points are alive for the plugin's lifetime.
                    unsafe {
                        a.connect(b.as_ptr());
                        b.connect(a.as_ptr());
                    }
                    Some((a, b))
                }
                _ => None,
            }
        } else {
            None
        };

        Ok(Plugin {
            _module: self,
            component,
            controller,
            separate,
            points,
            _host: host,
            handler,
            engine: None,
        })
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        self.factory = None;
        #[cfg(target_os = "macos")]
        if let Some(exit) = self.exit.take() {
            // SAFETY: the module's own exit point, called once after every
            // object it made has been released.
            unsafe { exit() };
        }
    }
}

/// One parameter, as the plugin describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// Parameter id. NKS knob slots refer to parameters by this.
    pub id: u32,
    /// Full name.
    pub title: String,
    /// Short name, for small displays.
    pub short_title: String,
    /// Unit label, e.g. `dB`.
    pub units: String,
    /// `0` for continuous, else the number of steps.
    pub step_count: i32,
    /// Default normalised value.
    pub default: f64,
    /// VST3 parameter flags.
    pub flags: i32,
}

/// A running plugin instance.
pub struct Plugin<'m> {
    _module: &'m Module,
    component: ComPtr<IComponent>,
    controller: ComPtr<IEditController>,
    separate: bool,
    points: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    _host: ComWrapper<com::Host>,
    handler: ComWrapper<com::ComponentHandler>,
    engine: Option<Arc<Mutex<Engine>>>,
}

/// `restartComponent` flag: parameter values changed, re-read them.
pub const PARAM_VALUES_CHANGED: i32 = 1 << 2;

/// The audio side, once started: the processor and buffers for every bus.
struct Engine {
    processor: ComPtr<IAudioProcessor>,
    sample_rate: f64,
    max_block: usize,
    active: bool,
    /// One buffer set per input bus, then per output bus: channels of samples.
    inputs: Vec<Vec<Vec<f32>>>,
    outputs: Vec<Vec<Vec<f32>>>,
    events: ComWrapper<com::EventList>,
    events_ptr: ComPtr<IEventList>,
    changes: ComWrapper<com::ParameterChanges>,
    changes_ptr: ComPtr<IParameterChanges>,
}

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

impl Engine {
    fn note(kind: u32, pitch: u8, velocity: u8) -> Event {
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

    fn pressure(pitch: u8, pressure: u8) -> Event {
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

    /// Fill `out` (interleaved, `channels` wide) from the main output bus,
    /// applying `inputs` at the start. Returns the peak level.
    fn render(
        &mut self,
        out: &mut [f32],
        channels: usize,
        inputs: &[Input],
    ) -> Result<f32, HostError> {
        out.fill(0.0);
        if !self.active {
            return Ok(0.0);
        }
        let channels = channels.max(1);
        let total = out.len() / channels;
        let mut done = 0;
        let mut peak = 0.0f32;
        let mut pending = inputs;
        while done < total {
            let frames = (total - done).min(self.max_block);
            // Inputs land at the start of the first chunk.
            let notes: Vec<Event> = pending
                .iter()
                .filter_map(|i| match *i {
                    Input::NoteOn { pitch, velocity } => {
                        Some(Self::note(kNoteOnEvent, pitch, velocity))
                    }
                    Input::NoteOff { pitch, velocity } => {
                        Some(Self::note(kNoteOffEvent, pitch, velocity))
                    }
                    Input::PolyPressure { pitch, pressure } => {
                        Some(Self::pressure(pitch, pressure))
                    }
                    Input::Param { .. } => None,
                })
                .collect();
            let params: Vec<(ParamID, ParamValue)> = pending
                .iter()
                .filter_map(|i| match *i {
                    Input::Param { id, value } => Some((id, value)),
                    _ => None,
                })
                .collect();
            pending = &[];
            if let Ok(mut e) = self.events.events.lock() {
                e.clear();
                e.extend(notes);
            }
            self.changes.set(&params);

            let mut in_ptrs: Vec<Vec<*mut f32>> = self
                .inputs
                .iter_mut()
                .map(|bus| {
                    bus.iter_mut()
                        .map(|ch| {
                            ch.fill(0.0);
                            ch.as_mut_ptr()
                        })
                        .collect()
                })
                .collect();
            let mut out_ptrs: Vec<Vec<*mut f32>> = self
                .outputs
                .iter_mut()
                .map(|bus| {
                    bus.iter_mut()
                        .map(|ch| {
                            ch.fill(0.0);
                            ch.as_mut_ptr()
                        })
                        .collect()
                })
                .collect();
            let mut bus = |ptrs: &mut Vec<*mut f32>| AudioBusBuffers {
                numChannels: ptrs.len() as int32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: ptrs.as_mut_ptr(),
                },
            };
            let mut ins: Vec<AudioBusBuffers> = in_ptrs.iter_mut().map(&mut bus).collect();
            let mut outs: Vec<AudioBusBuffers> = out_ptrs.iter_mut().map(&mut bus).collect();
            // SAFETY: ProcessContext is plain data.
            let mut context: ProcessContext = unsafe { std::mem::zeroed() };
            context.state = kTempoValid;
            context.sampleRate = self.sample_rate;
            context.tempo = 120.0;
            context.timeSigNumerator = 4;
            context.timeSigDenominator = 4;
            let mut data = ProcessData {
                processMode: kRealtime as int32,
                symbolicSampleSize: kSample32 as int32,
                numSamples: frames as int32,
                numInputs: ins.len() as int32,
                numOutputs: outs.len() as int32,
                inputs: if ins.is_empty() {
                    std::ptr::null_mut()
                } else {
                    ins.as_mut_ptr()
                },
                outputs: if outs.is_empty() {
                    std::ptr::null_mut()
                } else {
                    outs.as_mut_ptr()
                },
                inputParameterChanges: if self.changes.is_empty() {
                    std::ptr::null_mut()
                } else {
                    self.changes_ptr.as_ptr()
                },
                outputParameterChanges: std::ptr::null_mut(),
                inputEvents: self.events_ptr.as_ptr(),
                outputEvents: std::ptr::null_mut(),
                processContext: &mut context,
            };
            // SAFETY: every buffer pointer refers to a live Vec of max_block
            // samples, and the event and change lists outlive the call.
            check("process", unsafe { self.processor.process(&mut data) })?;

            if let Some(main) = self.outputs.first().filter(|b| !b.is_empty()) {
                for f in 0..frames {
                    for c in 0..channels {
                        let sample = main
                            .get(c % main.len())
                            .and_then(|ch| ch.get(f))
                            .copied()
                            .unwrap_or(0.0);
                        if let Some(slot) = out.get_mut((done + f) * channels + c) {
                            *slot = sample;
                        }
                        peak = peak.max(sample.abs());
                    }
                }
            }
            done += frames;
        }
        Ok(peak)
    }
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

/// A handle to a running plugin's audio engine, for an audio thread.
///
/// Stop the audio stream that uses this before dropping the [`Plugin`] or
/// calling [`Plugin::stop_audio`]; after either, `render` writes silence.
#[derive(Clone)]
pub struct Audio(Arc<Mutex<Engine>>);

impl Audio {
    /// Render `out`, interleaved and `channels` wide, applying `inputs` first.
    /// Returns the peak level.
    pub fn render(
        &self,
        out: &mut [f32],
        channels: usize,
        inputs: &[Input],
    ) -> Result<f32, HostError> {
        match self.0.lock() {
            Ok(mut e) => e.render(out, channels, inputs),
            Err(_) => {
                out.fill(0.0);
                Ok(0.0)
            }
        }
    }

    /// The sample rate the engine was set up for.
    pub fn sample_rate(&self) -> f64 {
        self.0.lock().map(|e| e.sample_rate).unwrap_or(0.0)
    }
}

impl Plugin<'_> {
    /// Hand the plugin its saved state, as opaque bytes, then let the
    /// controller sync to it.
    ///
    /// For an NKS preset this is the `PCHK` chunk's body. The bytes are
    /// passed through untouched; only the plugin reads them.
    pub fn load_state(&self, state: &[u8]) -> Result<(), HostError> {
        let stream = ComWrapper::new(com::MemoryStream::new(state.to_vec()));
        let s = stream
            .to_com_ptr::<IBStream>()
            .ok_or(HostError::NoFactory)?;
        // SAFETY: the stream lives until both calls return.
        check("IComponent::setState", unsafe {
            self.component.setState(s.as_ptr())
        })?;
        stream.rewind();
        check("IEditController::setComponentState", unsafe {
            self.controller.setComponentState(s.as_ptr())
        })
    }

    /// The plugin's saved state, as it would write it to a preset.
    pub fn save_state(&self) -> Result<Vec<u8>, HostError> {
        let stream = ComWrapper::new(com::MemoryStream::default());
        let s = stream
            .to_com_ptr::<IBStream>()
            .ok_or(HostError::NoFactory)?;
        check("IComponent::getState", unsafe {
            self.component.getState(s.as_ptr())
        })?;
        Ok(stream.bytes())
    }

    /// Start the audio engine: set up processing, activate the main output
    /// and event input buses, and switch the processor on.
    ///
    /// Some plugins apply a loaded state only once their engine runs.
    pub fn start_audio(&mut self, sample_rate: f64, max_block: usize) -> Result<(), HostError> {
        if self.engine.is_some() {
            return Ok(());
        }
        let processor = self
            .component
            .cast::<IAudioProcessor>()
            .ok_or(HostError::Call {
                call: "IAudioProcessor",
                code: kNoInterface,
            })?;
        let mut setup = ProcessSetup {
            processMode: kRealtime as int32,
            symbolicSampleSize: kSample32 as int32,
            maxSamplesPerBlock: max_block as int32,
            sampleRate: sample_rate,
        };
        // SAFETY: plain calls with valid in-parameters.
        check("setupProcessing", unsafe {
            processor.setupProcessing(&mut setup)
        })?;

        let buses = |dir| -> Vec<Vec<Vec<f32>>> {
            // SAFETY: bus queries into a zeroed POD struct.
            let n = unsafe { self.component.getBusCount(kAudio as MediaType, dir) };
            (0..n)
                .map(|i| {
                    let mut info: BusInfo = unsafe { std::mem::zeroed() };
                    unsafe {
                        self.component
                            .getBusInfo(kAudio as MediaType, dir, i, &mut info)
                    };
                    let channels = usize::try_from(info.channelCount).unwrap_or(0);
                    vec![vec![0.0f32; max_block]; channels]
                })
                .collect()
        };
        let inputs = buses(kInput as BusDirection);
        let outputs = buses(kOutput as BusDirection);
        // SAFETY: activating buses that exist, then the component.
        unsafe {
            if !outputs.is_empty() {
                self.component
                    .activateBus(kAudio as MediaType, kOutput as BusDirection, 0, 1);
            }
            if self
                .component
                .getBusCount(kEvent as MediaType, kInput as BusDirection)
                > 0
            {
                self.component
                    .activateBus(kEvent as MediaType, kInput as BusDirection, 0, 1);
            }
            check("setActive", self.component.setActive(1))?;
            // Some plugins return kNotImplemented here; that is allowed.
            processor.setProcessing(1);
        }
        let events = ComWrapper::new(com::EventList::default());
        let changes = ComWrapper::new(com::ParameterChanges::default());
        let (Some(events_ptr), Some(changes_ptr)) = (
            events.to_com_ptr::<IEventList>(),
            changes.to_com_ptr::<IParameterChanges>(),
        ) else {
            return Err(HostError::Call {
                call: "event lists",
                code: kNoInterface,
            });
        };
        self.engine = Some(Arc::new(Mutex::new(Engine {
            processor,
            sample_rate,
            max_block,
            active: true,
            inputs,
            outputs,
            events,
            events_ptr,
            changes,
            changes_ptr,
        })));
        Ok(())
    }

    /// A handle an audio thread can render through. `None` until
    /// [`start_audio`](Self::start_audio).
    pub fn audio(&self) -> Option<Audio> {
        self.engine.clone().map(Audio)
    }

    /// Stop the audio engine, if it is running.
    pub fn stop_audio(&mut self) {
        if let Some(engine) = self.engine.take()
            && let Ok(mut e) = engine.lock()
        {
            e.active = false;
            // SAFETY: the reverse of start_audio.
            unsafe {
                e.processor.setProcessing(0);
                self.component.setActive(0);
            }
        }
    }

    /// Run one block of `frames` with no notes, on the calling thread.
    /// Returns the output's peak level. For hosts without an audio thread;
    /// with one, render through [`audio`](Self::audio) instead.
    pub fn process_block(&mut self, frames: usize) -> Result<f32, HostError> {
        let audio = self.audio().ok_or(HostError::Call {
            call: "process (engine not started)",
            code: kNotInitialized,
        })?;
        let mut scratch = vec![0.0f32; frames * 2];
        audio.render(&mut scratch, 2, &[])
    }

    /// The `restartComponent` flags the plugin has raised since the last
    /// call, OR-ed together. [`PARAM_VALUES_CHANGED`] after a preset applies
    /// means the host should re-read every value.
    pub fn take_restart_flags(&self) -> i32 {
        self.handler
            .restarts
            .lock()
            .map(|mut r| std::mem::take(&mut *r))
            .unwrap_or(0)
    }

    /// Edits the plugin made itself (`performEdit`) since the last call.
    pub fn take_edits(&self) -> Vec<(u32, f64)> {
        self.handler
            .edits
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }

    /// Ask the plugin which parameter each MIDI controller drives (bus 0,
    /// channel 0). Empty when the plugin has no MIDI mapping.
    pub fn midi_map(&self) -> MidiMap {
        let Some(mapping) = self.controller.cast::<IMidiMapping>() else {
            return MidiMap::new();
        };
        (0..=PITCH_BEND)
            .filter_map(|ctrl| {
                let mut id: ParamID = 0;
                // SAFETY: out-parameter ParamID.
                let code = unsafe {
                    mapping.getMidiControllerAssignment(0, 0, ctrl as CtrlNumber, &mut id)
                };
                (code == kResultOk).then_some((ctrl, id))
            })
            .collect()
    }

    /// Whether the controller is a separate object from the processor, joined
    /// by connection points.
    pub fn separate_controller(&self) -> bool {
        self.separate
    }

    /// Every parameter the controller exposes.
    pub fn parameters(&self) -> Vec<Param> {
        // SAFETY: plain controller queries into a zeroed POD struct.
        let n = unsafe { self.controller.getParameterCount() };
        (0..n)
            .filter_map(|i| {
                let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
                (unsafe { self.controller.getParameterInfo(i, &mut info) } == kResultOk).then(
                    || Param {
                        id: info.id,
                        title: com::read_utf16(&info.title),
                        short_title: com::read_utf16(&info.shortTitle),
                        units: com::read_utf16(&info.units),
                        step_count: info.stepCount,
                        default: info.defaultNormalizedValue,
                        flags: info.flags,
                    },
                )
            })
            .collect()
    }

    /// A parameter's current normalised value.
    pub fn value(&self, id: u32) -> f64 {
        // SAFETY: plain query.
        unsafe { self.controller.getParamNormalized(id) }
    }

    /// The plugin's own text for a value, e.g. `"440 Hz"`.
    pub fn display(&self, id: u32, value: f64) -> Option<String> {
        let mut buf: String128 = [0; 128];
        // SAFETY: out-parameter String128.
        let code = unsafe { self.controller.getParamStringByValue(id, value, &mut buf) };
        (code == kResultOk).then(|| com::read_utf16(&buf))
    }

    /// Set a parameter on the controller. The audio processor hears about it
    /// once audio processing exists.
    pub fn set_value(&self, id: u32, value: f64) -> Result<(), HostError> {
        check("setParamNormalized", unsafe {
            self.controller
                .setParamNormalized(id, value.clamp(0.0, 1.0))
        })
    }
}

impl Drop for Plugin<'_> {
    fn drop(&mut self) {
        self.stop_audio();
        // SAFETY: teardown in the order the VST3 docs give: disconnect, then
        // terminate the controller, then the component.
        unsafe {
            if let Some((a, b)) = self.points.take() {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            self.controller.setComponentHandler(std::ptr::null_mut());
            if self.separate {
                self.controller.terminate();
            }
            self.component.terminate();
        }
    }
}

/// Service the main thread's run loop for a while.
///
/// Some plugins schedule work on the main run loop (timers, deferred
/// initialisation). A host without a GUI event loop calls this regularly.
#[cfg(target_os = "macos")]
pub fn pump_main_run_loop(for_at_most: Duration) {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
    // SAFETY: reading an immutable CF constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    let _ = CFRunLoop::run_in_mode(mode, for_at_most, false);
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
        // Centre bend is 0x2000 of 0x3FFF.
        let Some(Input::Param { id: 103, value }) = midi_to_input(&[0xE0, 0x00, 0x40], &map) else {
            panic!("pitch bend")
        };
        assert!((value - 8192.0 / 16383.0).abs() < 1e-9);
        assert_eq!(midi_to_input(&[0xF8], &map), None, "clock is ignored");
    }

    #[test]
    fn the_audio_handle_can_go_to_an_audio_thread() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<Audio>();
    }

    #[test]
    fn an_nks_uid_becomes_a_big_endian_class_id_on_macos() {
        // Massive X, from a preset's PLID chunk.
        let cid = class_id([1_448_301_646, 1_763_985_517, 1_634_956_137, 1_986_338_936]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(&cid[..4], &1_448_301_646u32.to_be_bytes());
        assert_eq!(cid.len(), 16);
    }
}
