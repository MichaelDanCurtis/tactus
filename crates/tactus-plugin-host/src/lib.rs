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
//! A [`Plugin`] holds the [`Module`] it came from (`Arc`), so the library
//! cannot be unloaded while an instance is alive.

#![deny(missing_docs)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod com;
mod engine;

pub use engine::{
    AFTERTOUCH, Audio, Input, MAX_EVENTS, MAX_PARAM_CHANGES, MidiMap, PITCH_BEND, midi_to_input,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use vst3::Steinberg::Vst::BusDirections_::{kInput, kOutput};
use vst3::Steinberg::Vst::MediaTypes_::{kAudio, kEvent};
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

pub(crate) fn check(call: &'static str, code: tresult) -> Result<(), HostError> {
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

// SAFETY: after `load` a Module is never mutated until it drops. Its factory
// is a VST3 IPluginFactory, which may be called from any thread; the library
// handle and the CFBundle are immutable references to loaded images.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Module {
    /// Load a `.vst3` bundle.
    #[cfg(target_os = "macos")]
    pub fn load(bundle: &Path) -> Result<Arc<Self>, HostError> {
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
        Ok(Arc::new(Self {
            factory: Some(factory),
            exit,
            _lib: lib,
            _bundle: cf,
        }))
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
    pub fn instantiate(self: &Arc<Self>, cid: &[u8; 16]) -> Result<Plugin, HostError> {
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
            component,
            controller,
            separate,
            points,
            _host: host,
            handler,
            engine: None,
            running: None,
            _module: Arc::clone(self),
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
pub struct Plugin {
    component: ComPtr<IComponent>,
    controller: ComPtr<IEditController>,
    separate: bool,
    points: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    _host: ComWrapper<com::Host>,
    handler: ComWrapper<com::ComponentHandler>,
    engine: Option<Box<engine::Engine>>,
    running: Option<(ComPtr<IAudioProcessor>, Arc<std::sync::atomic::AtomicU8>)>,
    // Last: released after every object the module made.
    _module: Arc<Module>,
}

/// `restartComponent` flag: parameter values changed, re-read them.
pub const PARAM_VALUES_CHANGED: i32 = 1 << 2;

impl Plugin {
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
        if self.running.is_some() {
            return Ok(());
        }
        let processor = self
            .component
            .cast::<IAudioProcessor>()
            .ok_or(HostError::Call {
                call: "IAudioProcessor",
                code: kNoInterface,
            })?;
        let engine =
            engine::Engine::new(&self.component, processor.clone(), sample_rate, max_block)?;
        // SAFETY: activating buses that exist, then the component.
        unsafe {
            if self
                .component
                .getBusCount(kAudio as MediaType, kOutput as BusDirection)
                > 0
            {
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
        self.running = Some((processor, Arc::clone(&engine.state)));
        self.engine = Some(Box::new(engine));
        Ok(())
    }

    /// Hand the audio engine to an audio thread. `None` before
    /// [`start_audio`](Self::start_audio) or once taken.
    pub fn take_audio(&mut self) -> Option<Audio> {
        self.engine.take().map(Audio)
    }

    /// Give a taken engine back, so [`process_block`](Self::process_block)
    /// works again.
    pub fn put_audio(&mut self, audio: Audio) {
        self.engine = Some(audio.0);
    }

    /// Stop the audio engine, if it is running. Waits for a block in flight
    /// on the audio thread to finish; after this the engine only renders
    /// silence and never calls the plugin again.
    pub fn stop_audio(&mut self) {
        if let Some((processor, state)) = self.running.take() {
            engine::stop(&state);
            // SAFETY: the reverse of start_audio, with no block in flight.
            unsafe {
                processor.setProcessing(0);
                self.component.setActive(0);
            }
        }
        self.engine = None;
    }

    /// Run one block of `frames` with no notes, on the calling thread, while
    /// the engine is not handed to an audio thread. Returns the peak level.
    pub fn process_block(&mut self, frames: usize) -> Result<f32, HostError> {
        let e = self.engine.as_mut().ok_or(HostError::Call {
            call: "process (engine not started, or taken)",
            code: kNotInitialized,
        })?;
        let mut scratch = std::mem::take(&mut e.scratch);
        let n = (frames * 2).min(scratch.len());
        let peak = e.render(scratch.get_mut(..n).unwrap_or_default(), 2, &[]);
        e.scratch = scratch;
        Ok(peak)
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

impl Drop for Plugin {
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
    fn an_nks_uid_becomes_a_big_endian_class_id_on_macos() {
        // Massive X, from a preset's PLID chunk.
        let cid = class_id([1_448_301_646, 1_763_985_517, 1_634_956_137, 1_986_338_936]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(&cid[..4], &1_448_301_646u32.to_be_bytes());
        assert_eq!(cid.len(), 16);
    }
}
