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
