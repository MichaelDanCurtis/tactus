//! The frame: the VST3 component a DAW loads.
//!
//! One object is processor and controller together. Towards the DAW it is an
//! instrument with one event input and one stereo output whose parameters,
//! MIDI mapping and editor are the hosted instrument's own, forwarded.
//!
//! Threads:
//!
//! - **Audio** (`process`): renders the instrument. It only ever `try_lock`s
//!   the real-time state; if that is busy (a plugin swap in progress) the
//!   block is silent. Notes go out to the main side through a bounded
//!   channel with `try_send`. Nothing allocates.
//! - **Main** (the DAW's UI thread): controller calls, state, and a 10 ms
//!   run-loop timer that handles the keyboard: knobs, presets, lights.
//! - **Keyboard** (`keyboard.rs`): all waits on the agent.

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use tactus_nks::artwork;
use tactus_odr::Rgb;
use tactus_odr::events::Event as DeviceEvent;
use tactus_plugin_host::vst3::Steinberg::Vst::BusDirections_::{kInput, kOutput};
use tactus_plugin_host::vst3::Steinberg::Vst::BusTypes_::kMain;
use tactus_plugin_host::vst3::Steinberg::Vst::Event_::EventTypes_::{
    kNoteOffEvent, kNoteOnEvent, kPolyPressureEvent,
};
use tactus_plugin_host::vst3::Steinberg::Vst::MediaTypes_::{kAudio, kEvent};
use tactus_plugin_host::vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::{
    kPlaying, kTempoValid,
};
use tactus_plugin_host::vst3::Steinberg::Vst::SymbolicSampleSizes_::kSample32;
use tactus_plugin_host::vst3::Steinberg::Vst::*;
use tactus_plugin_host::vst3::Steinberg::*;
use tactus_plugin_host::vst3::{Class, ComPtr, ComRef};
use tactus_plugin_host::{Audio, Input, MAX_EVENTS, MAX_PARAM_CHANGES, PARAM_VALUES_CHANGED};

use crate::instrument::{self, Instrument};
use crate::keyboard::{Command, Keyboard, KnobData, Screen};
use crate::state::SavedState;

/// `restartComponent` flags the frame raises towards the DAW.
const PARAM_TITLES_CHANGED: i32 = 1 << 4;
/// Light-guide colour for a held key (confirmed: green).
const HELD: u8 = 0x1C;
/// Stereo speaker arrangement (left | right).
const STEREO: SpeakerArrangement = 0x3;

/// State touched only on the DAW's main thread (behind a mutex so the type
/// is shareable; the audio thread never takes it).
struct Main {
    instrument: Option<Instrument>,
    handler: Option<ComPtr<IComponentHandler>>,
    keyboard: Option<Keyboard>,
    params_tx: SyncSender<Input>,
    notes_rx: Receiver<(u8, bool)>,
    rate: f64,
    max_block: usize,
    active: bool,
    held: [bool; 128],
    sent_transport: (bool, u64),
    /// Parameter ids on the keyboard's pages. Value updates for anything
    /// else are not sent: the agent rejects ids its page does not hold.
    on_screen: std::collections::HashSet<u32>,
}

/// State the audio thread renders with.
struct Rt {
    audio: Option<Audio>,
    params_rx: Receiver<Input>,
    notes_tx: SyncSender<(u8, bool)>,
    inputs: Vec<Input>,
}

pub(crate) struct Shared {
    main: Mutex<Main>,
    rt: Mutex<Rt>,
    playing: AtomicBool,
    tempo_bits: AtomicU64,
}

/// The run-loop timer that drives the main side.
struct Timer(core_foundation::runloop::CFRunLoopTimer, *const Shared);

/// The component.
pub struct Frame {
    shared: Arc<Shared>,
    timer: Mutex<Option<Timer>>,
}

// SAFETY: the timer is created, used and invalidated on the main thread
// only; the raw pointer it carries is an Arc reference owned by the timer.
unsafe impl Send for Timer {}

impl Default for Frame {
    fn default() -> Self {
        let (params_tx, params_rx) = sync_channel(1024);
        let (notes_tx, notes_rx) = sync_channel(1024);
        let shared = Shared {
            main: Mutex::new(Main {
                instrument: None,
                handler: None,
                keyboard: None,
                params_tx,
                notes_rx,
                rate: 48_000.0,
                max_block: 4096,
                active: false,
                held: [false; 128],
                sent_transport: (false, 0),
                on_screen: std::collections::HashSet::new(),
            }),
            rt: Mutex::new(Rt {
                audio: None,
                params_rx,
                notes_tx,
                inputs: Vec::with_capacity(MAX_EVENTS + MAX_PARAM_CHANGES),
            }),
            playing: AtomicBool::new(false),
            tempo_bits: AtomicU64::new(120f64.to_bits()),
        };
        Self {
            shared: Arc::new(shared),
            timer: Mutex::new(None),
        }
    }
}

impl Class for Frame {
    type Interfaces = (IComponent, IAudioProcessor, IEditController, IMidiMapping);
}

/// Read every byte from a DAW-provided stream.
fn read_all(stream: *mut IBStream) -> Vec<u8> {
    // SAFETY: the DAW's stream, valid for this call; borrowed, not owned.
    let Some(s) = (unsafe { ComRef::<IBStream>::from_raw(stream) }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut n: int32 = 0;
        // SAFETY: chunk is a writable buffer of the size given.
        let code = unsafe { s.read(chunk.as_mut_ptr().cast(), chunk.len() as int32, &mut n) };
        if code != kResultOk || n <= 0 {
            break;
        }
        out.extend_from_slice(chunk.get(..n as usize).unwrap_or_default());
    }
    out
}

fn write_all(stream: *mut IBStream, bytes: &[u8]) -> tresult {
    // SAFETY: the DAW's stream, valid for this call; borrowed, not owned.
    let Some(s) = (unsafe { ComRef::<IBStream>::from_raw(stream) }) else {
        return kInvalidArgument;
    };
    let mut written: int32 = 0;
    // SAFETY: bytes is a readable buffer of the size given.
    unsafe {
        s.write(
            bytes.as_ptr() as *mut c_void,
            bytes.len() as int32,
            &mut written,
        )
    }
}

impl Main {
    fn send(&self, cmd: Command) {
        if let Command::Value { id, .. } = &cmd
            && !self.on_screen.contains(id)
        {
            return;
        }
        if let Some(k) = &self.keyboard {
            let _ = k.commands.send(cmd);
        }
    }

    /// Build the keyboard's screen from the instrument and its preset.
    fn screen(&self) -> Option<Screen> {
        let inst = self.instrument.as_ref()?;
        let plugin = &inst.plugin;
        let preset = inst.preset.as_ref()?;
        let product = preset.info.bankchain.first().cloned().unwrap_or_default();
        let root = Path::new(artwork::MAC_RESOURCE_ROOT);
        let pages = preset
            .pages
            .iter()
            .filter(|p| !p.is_empty())
            .map(|page| {
                page.sections()
                    .into_iter()
                    .map(|sec| {
                        let knobs = sec
                            .slots
                            .iter()
                            .map(|s| {
                                s.map(|s| {
                                    let value = plugin.value(s.id);
                                    KnobData {
                                        id: s.id,
                                        name: s.name.clone(),
                                        value,
                                        display: plugin.display(s.id, value).unwrap_or_default(),
                                    }
                                })
                            })
                            .collect();
                        (sec.name.to_owned(), knobs)
                    })
                    .collect()
            })
            .collect();
        Some(Screen {
            plugin_name: preset.plugin.name.clone().unwrap_or(product.clone()),
            pages,
            banner: artwork::hardware_banner(root, &product).and_then(|p| std::fs::read(p).ok()),
            arc: artwork::control_color(root, &product).map(|[r, g, b]| Rgb { r, g, b }),
            preset_name: preset.info.name.clone(),
        })
    }

    fn show(&mut self) {
        if let Some(s) = self.screen() {
            self.on_screen = s
                .pages
                .iter()
                .flatten()
                .flat_map(|(_, knobs)| knobs.iter().flatten().map(|k| k.id))
                .collect();
            self.send(Command::Show(Box::new(s)));
        }
    }

    fn restart(&self, flags: i32) {
        if let Some(h) = &self.handler {
            // SAFETY: the DAW's handler, called on the main thread.
            unsafe { h.restartComponent(flags) };
        }
    }

    /// Tell the DAW a parameter moved, as if the user had moved it in the
    /// plugin's window.
    fn edit(&self, id: u32, value: f64) {
        if let Some(h) = &self.handler {
            // SAFETY: the DAW's handler, called on the main thread.
            unsafe {
                h.beginEdit(id);
                h.performEdit(id, value);
                h.endEdit(id);
            }
        }
    }

    /// One parameter changed on our side: plugin controller, audio, DAW,
    /// keyboard.
    fn set_param(&self, id: u32, value: f64, tell_daw: bool) {
        let Some(inst) = &self.instrument else {
            return;
        };
        let _ = inst.plugin.set_value(id, value);
        let value = inst.plugin.value(id);
        let _ = self.params_tx.try_send(Input::Param { id, value });
        if tell_daw {
            self.edit(id, value);
        }
        let display = inst.plugin.display(id, value).unwrap_or_default();
        self.send(Command::Value { id, value, display });
    }

    /// Start or stop the instrument's engine and hand it to (or take it
    /// back from) the audio thread.
    fn set_active(&mut self, rt: &Mutex<Rt>, on: bool) {
        let Some(inst) = self.instrument.as_mut() else {
            self.active = on;
            return;
        };
        if on && !self.active {
            if inst.plugin.start_audio(self.rate, self.max_block).is_ok() {
                if let Some(state) = inst.pending_state.take() {
                    let _ = inst.plugin.load_state(&state);
                }
                let audio = inst.plugin.take_audio();
                if let Ok(mut r) = rt.lock() {
                    r.audio = audio;
                }
            }
        } else if !on && self.active {
            let audio = rt.lock().ok().and_then(|mut r| r.audio.take());
            if let Some(a) = audio {
                inst.plugin.put_audio(a);
            }
            inst.plugin.stop_audio();
        }
        self.active = on;
    }

    /// Replace the instrument, keeping the engine's running state.
    fn replace(&mut self, rt: &Mutex<Rt>, new: Instrument) {
        let was_active = self.active;
        self.set_active(rt, false);
        self.instrument = Some(new);
        if was_active {
            self.set_active(rt, true);
        }
        self.restart(PARAM_TITLES_CHANGED | PARAM_VALUES_CHANGED);
        self.show();
    }

    /// The main side's periodic work.
    fn tick(&mut self, shared: &Shared) {
        // Keyboard events.
        let events: Vec<DeviceEvent> = self
            .keyboard
            .as_ref()
            .map(|k| k.events.try_iter().collect())
            .unwrap_or_default();
        for ev in events {
            match ev {
                DeviceEvent::KnobTurn { id, delta } => {
                    if let Some(v) = self.instrument.as_ref().map(|i| i.plugin.value(id)) {
                        self.set_param(id, (v + delta).clamp(0.0, 1.0), true);
                    }
                }
                DeviceEvent::PresetStep(step) => {
                    let Some(inst) = self.instrument.as_mut() else {
                        continue;
                    };
                    let folder = inst.folder();
                    let here = inst
                        .preset_path
                        .as_ref()
                        .and_then(|p| folder.iter().position(|f| f == p));
                    if let Some(here) = here {
                        let len = folder.len().max(1) as i64;
                        let next = (here as i64 + i64::from(step)).rem_euclid(len) as usize;
                        if let Some(p) = folder.get(next) {
                            let _ = inst.switch_preset(p);
                        }
                    }
                }
                _ => {}
            }
        }

        // Notes from the audio thread: light the keys; a first note takes
        // the keyboard, so it follows whichever track is being played.
        let mut changed = Vec::new();
        let was_idle = !self.held.iter().any(|&h| h);
        while let Ok((pitch, down)) = self.notes_rx.try_recv() {
            if let Some(h) = self.held.get_mut(usize::from(pitch))
                && *h != down
            {
                *h = down;
                changed.push((pitch, if down { HELD } else { 0 }));
            }
        }
        if !changed.is_empty() {
            if was_idle && self.held.iter().any(|&h| h) {
                self.send(Command::TakeFocus);
            }
            self.send(Command::Leds(changed));
        }

        // The instrument's own news: a preset applied, or edits in its window.
        let news = self
            .instrument
            .as_ref()
            .map(|i| (i.plugin.take_restart_flags(), i.plugin.take_edits()));
        if let Some((flags, edits)) = news {
            if flags & PARAM_VALUES_CHANGED != 0 {
                self.restart(PARAM_VALUES_CHANGED);
                self.show();
            }
            for (id, value) in edits {
                let _ = self.params_tx.try_send(Input::Param { id, value });
                self.edit(id, value);
                let display = self
                    .instrument
                    .as_ref()
                    .and_then(|i| i.plugin.display(id, value))
                    .unwrap_or_default();
                self.send(Command::Value { id, value, display });
            }
        }

        // The DAW's transport, for the transport lights.
        let transport = (
            shared.playing.load(Ordering::Relaxed),
            shared.tempo_bits.load(Ordering::Relaxed),
        );
        if transport != self.sent_transport {
            self.sent_transport = transport;
            self.send(Command::Transport {
                playing: transport.0,
                tempo: f64::from_bits(transport.1),
            });
        }
    }
}

extern "C" fn on_timer(_timer: core_foundation::runloop::CFRunLoopTimerRef, info: *mut c_void) {
    // SAFETY: info is the Arc reference the timer owns; it outlives the timer.
    let shared = unsafe { &*(info as *const Shared) };
    if let Ok(mut main) = shared.main.try_lock() {
        main.tick(shared);
    }
}

impl Frame {
    fn start_timer(&self) {
        use core_foundation::date::CFAbsoluteTimeGetCurrent;
        use core_foundation::runloop::{
            CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext, kCFRunLoopCommonModes,
        };
        let info = Arc::into_raw(Arc::clone(&self.shared));
        let mut ctx = CFRunLoopTimerContext {
            version: 0,
            info: info as *mut c_void,
            retain: None,
            release: None,
            copyDescription: None,
        };
        // SAFETY: plain CF calls; the context is copied by CFRunLoopTimerCreate.
        let timer = unsafe {
            CFRunLoopTimer::new(
                CFAbsoluteTimeGetCurrent() + 0.01,
                0.01,
                0,
                0,
                on_timer,
                &mut ctx,
            )
        };
        // SAFETY: reading an immutable CF constant.
        CFRunLoop::get_main().add_timer(&timer, unsafe { kCFRunLoopCommonModes });
        if let Ok(mut t) = self.timer.lock() {
            *t = Some(Timer(timer, info));
        }
    }

    fn stop_timer(&self) {
        use core_foundation::base::TCFType;
        if let Some(Timer(timer, info)) = self.timer.lock().ok().and_then(|mut t| t.take()) {
            // SAFETY: invalidating a timer this frame created.
            unsafe {
                core_foundation::runloop::CFRunLoopTimerInvalidate(timer.as_concrete_TypeRef())
            };
            // SAFETY: the reference start_timer leaked; the timer is gone.
            drop(unsafe { Arc::from_raw(info) });
        }
    }
}

impl IPluginBaseTrait for Frame {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        if let Ok(mut main) = self.shared.main.lock() {
            if let Some(p) = instrument::default_preset() {
                match Instrument::from_preset(&p) {
                    Ok(i) => main.instrument = Some(i),
                    Err(e) => eprintln!("tactus-shim: {}: {e}", p.display()),
                }
            }
            let registry = instrument::support_dir().map(|d| d.join("symbol_registry.json"));
            if let Some(r) = registry.filter(|r| r.is_file()) {
                main.keyboard = Some(Keyboard::start(r, "tactus-shim".into()));
            }
            main.show();
        }
        self.start_timer();
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        self.stop_timer();
        if let Ok(mut main) = self.shared.main.lock() {
            main.set_active(&self.shared.rt, false);
            main.keyboard = None;
            main.handler = None;
            main.instrument = None;
        }
        kResultOk
    }
}

impl IComponentTrait for Frame {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, r#type: MediaType, dir: BusDirection) -> int32 {
        let (audio, event) = (r#type == kAudio as MediaType, r#type == kEvent as MediaType);
        let (input, output) = (
            dir == kInput as BusDirection,
            dir == kOutput as BusDirection,
        );
        i32::from((audio && output) || (event && input))
    }

    unsafe fn getBusInfo(
        &self,
        r#type: MediaType,
        dir: BusDirection,
        index: int32,
        bus: *mut BusInfo,
    ) -> tresult {
        let Some(info) = (unsafe { bus.as_mut() }) else {
            return kInvalidArgument;
        };
        if index != 0 || unsafe { self.getBusCount(r#type, dir) } == 0 {
            return kInvalidArgument;
        }
        info.mediaType = r#type;
        info.direction = dir;
        info.busType = kMain as BusType;
        info.flags = 1; // kDefaultActive
        let name = if r#type == kAudio as MediaType {
            info.channelCount = 2;
            "Output"
        } else {
            info.channelCount = 16;
            "MIDI In"
        };
        for (slot, unit) in info.name.iter_mut().zip(name.encode_utf16().chain([0])) {
            *slot = unit as TChar;
        }
        kResultOk
    }

    unsafe fn getRoutingInfo(
        &self,
        _in_info: *mut RoutingInfo,
        _out_info: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        _type: MediaType,
        _dir: BusDirection,
        _index: int32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        if let Ok(mut main) = self.shared.main.lock() {
            main.set_active(&self.shared.rt, state != 0);
        }
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let bytes = read_all(state);
        let Some(saved) = SavedState::from_bytes(&bytes) else {
            return kResultFalse;
        };
        let new = match Instrument::from_saved(&saved) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("tactus-shim: restoring {}: {e}", saved.bundle.display());
                return kResultFalse;
            }
        };
        match self.shared.main.lock() {
            Ok(mut main) => {
                main.replace(&self.shared.rt, new);
                kResultOk
            }
            Err(_) => kInternalError,
        }
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let bytes = match self.shared.main.lock() {
            Ok(main) => match &main.instrument {
                Some(inst) => inst.saved().to_bytes(),
                None => return kResultFalse,
            },
            Err(_) => return kInternalError,
        };
        write_all(state, &bytes)
    }
}

impl IAudioProcessorTrait for Frame {
    unsafe fn setBusArrangements(
        &self,
        _inputs: *mut SpeakerArrangement,
        _num_ins: int32,
        outputs: *mut SpeakerArrangement,
        num_outs: int32,
    ) -> tresult {
        // Stereo out only.
        let ok = num_outs == 1 && unsafe { outputs.as_ref() }.is_some_and(|&a| a == STEREO);
        if ok { kResultOk } else { kResultFalse }
    }

    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: int32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        match unsafe { arr.as_mut() } {
            Some(a) if dir == kOutput as BusDirection && index == 0 => {
                *a = STEREO;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size == kSample32 as int32 {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getLatencySamples(&self) -> uint32 {
        0
    }

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        let Some(s) = (unsafe { setup.as_ref() }) else {
            return kInvalidArgument;
        };
        if let Ok(mut main) = self.shared.main.lock() {
            main.rate = s.sampleRate;
            main.max_block = usize::try_from(s.maxSamplesPerBlock).unwrap_or(4096).max(1);
        }
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let Some(data) = (unsafe { data.as_mut() }) else {
            return kInvalidArgument;
        };
        let frames = usize::try_from(data.numSamples).unwrap_or(0);
        // Output channels, straight from the DAW's buffers.
        // SAFETY: the DAW provides numOutputs buses, each with numChannels
        // buffers of numSamples samples.
        let (left, right) = unsafe {
            let bus = data.outputs.as_mut().filter(|_| data.numOutputs > 0);
            let ch = bus.map(|b| (b.numChannels, b.__field0.channelBuffers32));
            match ch {
                Some((n, p)) if n >= 1 && !p.is_null() && frames > 0 => {
                    let l = std::slice::from_raw_parts_mut(*p, frames);
                    let r = (n >= 2).then(|| std::slice::from_raw_parts_mut(*p.add(1), frames));
                    (Some(l), r)
                }
                _ => (None, None),
            }
        };

        let Ok(mut rt) = self.shared.rt.try_lock() else {
            // A swap is in progress on the main thread: silence, no waiting.
            for ch in [left, right].into_iter().flatten() {
                ch.fill(0.0);
            }
            return kResultOk;
        };
        let Rt {
            audio,
            params_rx,
            notes_tx,
            inputs,
        } = &mut *rt;
        inputs.clear();
        let push = |i: Input, inputs: &mut Vec<Input>| {
            if inputs.len() < inputs.capacity() {
                inputs.push(i);
            }
        };
        while let Ok(i) = params_rx.try_recv() {
            push(i, inputs);
        }
        // SAFETY: the DAW's event list, valid for this call.
        {
            if let Some(list) = unsafe { ComRef::<IEventList>::from_raw(data.inputEvents) } {
                let n = unsafe { list.getEventCount() };
                for i in 0..n {
                    let mut ev: Event = unsafe { std::mem::zeroed() };
                    if unsafe { list.getEvent(i, &mut ev) } != kResultOk {
                        continue;
                    }
                    let kind = u32::from(ev.r#type);
                    // SAFETY: the union member matches the event type.
                    let input = unsafe {
                        if kind == kNoteOnEvent && ev.__field0.noteOn.velocity > 0.0 {
                            let n = ev.__field0.noteOn;
                            Some(Input::NoteOn {
                                pitch: n.pitch as u8,
                                velocity: (n.velocity * 127.0).round() as u8,
                            })
                        } else if kind == kNoteOnEvent || kind == kNoteOffEvent {
                            let n = ev.__field0.noteOff;
                            Some(Input::NoteOff {
                                pitch: n.pitch as u8,
                                velocity: (n.velocity * 127.0).round() as u8,
                            })
                        } else if kind == kPolyPressureEvent {
                            let p = ev.__field0.polyPressure;
                            Some(Input::PolyPressure {
                                pitch: p.pitch as u8,
                                pressure: (p.pressure * 127.0).round() as u8,
                            })
                        } else {
                            None
                        }
                    };
                    if let Some(input) = input {
                        match input {
                            Input::NoteOn { pitch, .. } => {
                                let _ = notes_tx.try_send((pitch, true));
                            }
                            Input::NoteOff { pitch, .. } => {
                                let _ = notes_tx.try_send((pitch, false));
                            }
                            _ => {}
                        }
                        push(input, inputs);
                    }
                }
            }
        }
        // Automation and MIDI-mapped controllers: last point of each queue.
        // SAFETY: the DAW's change list, valid for this call.
        {
            if let Some(changes) =
                unsafe { ComRef::<IParameterChanges>::from_raw(data.inputParameterChanges) }
            {
                let n = unsafe { changes.getParameterCount() };
                for i in 0..n {
                    let q = unsafe { changes.getParameterData(i) };
                    if let Some(q) = unsafe { ComRef::<IParamValueQueue>::from_raw(q) } {
                        let points = unsafe { q.getPointCount() };
                        let mut offset = 0;
                        let mut value = 0.0;
                        if points > 0
                            && unsafe { q.getPoint(points - 1, &mut offset, &mut value) }
                                == kResultOk
                        {
                            push(
                                Input::Param {
                                    id: unsafe { q.getParameterId() },
                                    value,
                                },
                                inputs,
                            );
                        }
                    }
                }
            }
        }
        // The DAW's time: to the instrument, and to the transport lights.
        if let Some(ctx) = unsafe { data.processContext.as_ref() } {
            self.shared
                .playing
                .store(ctx.state & kPlaying != 0, Ordering::Relaxed);
            if ctx.state & kTempoValid != 0 {
                self.shared
                    .tempo_bits
                    .store(ctx.tempo.to_bits(), Ordering::Relaxed);
            }
            if let Some(a) = audio.as_mut() {
                a.set_process_context(ctx);
            }
        }

        match (audio.as_mut(), left, right) {
            (Some(a), Some(l), Some(r)) => {
                a.render_planar(&mut [l, r], inputs);
            }
            (Some(a), Some(l), None) => {
                a.render_planar(&mut [l], inputs);
            }
            (_, l, r) => {
                for ch in [l, r].into_iter().flatten() {
                    ch.fill(0.0);
                }
            }
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> uint32 {
        0
    }
}

/// Run `f` on the hosted instrument's controller, or return `default`.
fn with_controller<T>(
    frame: &Frame,
    default: T,
    f: impl FnOnce(&ComPtr<IEditController>) -> T,
) -> T {
    match frame.shared.main.lock() {
        Ok(main) => match &main.instrument {
            Some(i) => f(i.plugin.edit_controller()),
            None => default,
        },
        Err(_) => default,
    }
}

impl IEditControllerTrait for Frame {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        // One object: the component's setState already restored everything.
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> int32 {
        with_controller(self, 0, |c| unsafe { c.getParameterCount() })
    }

    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        with_controller(self, kResultFalse, |c| unsafe {
            c.getParameterInfo(index, info)
        })
    }

    unsafe fn getParamStringByValue(
        &self,
        id: ParamID,
        value: ParamValue,
        string: *mut String128,
    ) -> tresult {
        with_controller(self, kResultFalse, |c| unsafe {
            c.getParamStringByValue(id, value, string)
        })
    }

    unsafe fn getParamValueByString(
        &self,
        id: ParamID,
        string: *mut TChar,
        value: *mut ParamValue,
    ) -> tresult {
        with_controller(self, kResultFalse, |c| unsafe {
            c.getParamValueByString(id, string, value)
        })
    }

    unsafe fn normalizedParamToPlain(&self, id: ParamID, value: ParamValue) -> ParamValue {
        with_controller(self, value, |c| unsafe {
            c.normalizedParamToPlain(id, value)
        })
    }

    unsafe fn plainParamToNormalized(&self, id: ParamID, value: ParamValue) -> ParamValue {
        with_controller(self, value, |c| unsafe {
            c.plainParamToNormalized(id, value)
        })
    }

    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        with_controller(self, 0.0, |c| unsafe { c.getParamNormalized(id) })
    }

    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        // The DAW syncing its view. The sound follows through process().
        let r = with_controller(self, kResultFalse, |c| unsafe {
            c.setParamNormalized(id, value)
        });
        if let Ok(main) = self.shared.main.lock()
            && let Some(inst) = &main.instrument
        {
            let display = inst.plugin.display(id, value).unwrap_or_default();
            main.send(Command::Value { id, value, display });
        }
        r
    }

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        if let Ok(mut main) = self.shared.main.lock() {
            // SAFETY: the DAW's handler; keep our own reference.
            main.handler = unsafe { ComRef::from_raw(handler) }.map(|h| h.to_com_ptr());
        }
        kResultOk
    }

    unsafe fn createView(&self, name: FIDString) -> *mut IPlugView {
        // The frame's window is the instrument's own window.
        with_controller(self, std::ptr::null_mut(), |c| unsafe {
            c.createView(name)
        })
    }
}

impl IMidiMappingTrait for Frame {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: int32,
        channel: int16,
        ctrl: CtrlNumber,
        id: *mut ParamID,
    ) -> tresult {
        with_controller(self, kResultFalse, |c| match c.cast::<IMidiMapping>() {
            Some(m) => unsafe { m.getMidiControllerAssignment(bus, channel, ctrl, id) },
            None => kResultFalse,
        })
    }
}
