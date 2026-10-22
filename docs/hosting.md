# Hosting plugins

`tactus-plugin-host` loads VST3 instruments in-process. The bindings are the
`vst3` crate (MIT OR Apache-2.0). Since 3.8.0 (October 2025) the VST 3 SDK
itself is MIT-licensed, so hosting needs no agreement with anyone. The plugin
runs its own licence check; the host does not touch it.

## Confirmed: Massive X loads an NKS preset

**Evidence class: run on the development machine** (`examples/probe.rs`,
Massive X VST3, Apple Silicon, no Komplete Kontrol running).

| Step | Result |
|---|---|
| Load `Massive X.vst3` (`bundleEntry`, `GetPluginFactory`) | 0.2–0.4 s |
| Find the preset's class | `class_id(PLID VST3.uid)` matches the factory's one `Audio Module Class` |
| Create and initialise | 0.3 s; one object implements both processor and controller |
| Parameters | 2,102 |
| NKS knob ids | **all 20** of Wob Wolfer's knob ids are VST3 `ParamID`s (`01 Macro 1` …) |
| `setState(PCHK body)` | returns `kResultOk` |

### The preset's state is the plugin's own state

The `PCHK` body (after its version word) has the same packaging as what the
plugin's `getState` writes: the same leading header layout, a length field
matching the blob's size, then compressed data. It goes to `setState` as is.
Nothing inside it is decoded. The comparison looked at the first 32 bytes of
each, enough to see the packaging and nothing more.

### State is applied in the background, and needs time

Right after `setState`, nothing changed: 0 of 2,102 parameters, and `getState`
still returned the default state (26,048 bytes). Running the run loop alone for
two seconds did not help, and neither did 50 blocks of audio processing.

What works is **real time**: start the engine, then process a block every
~10.7 ms (512 frames at 48 kHz) while servicing the main run loop. Between one
and two seconds in, the preset applies. `getState` grows to 31,702 bytes (the
preset is 31,729), and the macros take their values: Macro 1 `70.9 %`,
Macro 2 `20.3 %`, Macro 4 `36.2 %`, Macro 5 `4.69 %`, Macro 7 `34.4 %`.

A host should therefore start audio **before** loading a preset, keep
processing, and treat the preset as loaded only once the plugin reflects it.

## Not loadable here

- **Super 8** ships its VST3 as x86_64 only. A native arm64 host cannot load
  it; it would need an x86_64 build of the host running under Rosetta.
  Super 8 R2 presets also name only a VST2 id (`VST.magic`), no VST3 class.
- VST2 is not supported. Its SDK is no longer licensed.

## CONFIRMED: a playable plugin, no Komplete Kontrol

**Evidence class: hardware observation, confirmed by the device's owner**
(`examples/kontrol.rs`, S88 MK3, Massive X, Wob Wolfer, KK closed).

The keys play Massive X through the Mac's default output. The screen shows the
preset's pages with the plugin's own values and text. Knob turns reach both
the controller (for the text) and the processor (for the sound). Preset up and
down load the next preset into the plugin. The owner reported no audible lag.

How it is wired:

- The output device's callback (`cpal`, 256-frame buffer, about 5 ms at
  48 kHz) drives `Audio::render`. Notes go in as a VST3 event list, knob moves
  as parameter changes, both at the start of the block.
- Keys come from the keyboard's main MIDI port on a dedicated thread, which
  forwards each note straight to the audio callback's queue.
- The first version routed keys through the loop that talks to the agent.
  `Client::listen` could block for its socket's full 250 ms read timeout,
  whatever window it was asked for, so notes lagged by up to a quarter of a
  second. Both are fixed.

## Known gaps

Recorded at the milestone, before the owner's own list:

- **Only note on/off reaches the plugin.** Mod wheel, pitch bend, sustain,
  aftertouch and other controllers are dropped. VST3 maps MIDI controllers to
  parameters through the controller's `IMidiMapping`, which is not wired yet.
- **No light guide.** The keys' LEDs do not follow the plugin or the notes.
- **The plugin's own GUI is never opened.** Edits the plugin makes itself
  (`performEdit`) are recorded but not shown on the keyboard.
- **Fixed time context:** 120 BPM, 4/4, transport stopped. Tempo-synced
  patches cannot follow a host clock.
- **Preset changes after the first report "loaded" in ~10 ms.** With the
  device driving the engine the plugin may genuinely be that fast, or it may
  signal before its values settle. Check that the screen shows the new
  preset's values.
- **One plugin, VST3 only, Apple Silicon only.** No AU, no VST2. Intel-only
  bundles (Super 8) do not load. Only presets that name a VST3 class load.
- **Default output device only, f32 only,** and no device choice.
- **Serial hard-coded** in the examples.
