# The shim

A thin VST3 plugin ("the frame") that a DAW loads. It hosts one VST3
instrument, passes audio and MIDI straight through, and puts that instrument
on a Kontrol S88 MK3 keyboard with no Komplete Kontrol running. Crate:
`crates/tactus-shim`. Build and install:

```text
tools/bundle-shim.sh release --install
```

which writes `~/Library/Audio/Plug-Ins/VST3/Tactus Shim.vst3`.

## What the DAW sees

One class, **Tactus Shim** (`Audio Module Class`, subcategory `Instrument`),
with one MIDI input bus, one stereo output, and **the hosted instrument's own
parameter list, MIDI mapping and editor**, forwarded. Automating a parameter
in the DAW automates the instrument's parameter. The frame's window is the
instrument's own window.

## Threads

| Thread | Does | Never |
|---|---|---|
| DAW audio (`process`) | renders the instrument; reads the DAW's notes, automation and time | allocates, waits, or takes a lock it could wait on: it only `try_lock`s, and renders silence if a swap is in progress |
| DAW main | controller calls, state; a 10 ms run-loop timer for knobs, presets, lights and the instrument's own edits | talks to the agent |
| keyboard | owns the connection to the agent; every network wait | touches the plugin |

Notes leave the audio thread for the light guide through a bounded channel
with `try_send`. Knob moves reach it the same way.

## Saved state

`SavedState` (`src/state.rs`): `TACTSHIM`, a version, the preset path, the
instrument's bundle path and class id, then **the instrument's state as opaque
bytes**. On reopen the frame loads the bundle, creates the class and hands the
bytes back to it.

## The keyboard

- **Screen:** the preset's NKS pages, with the instrument's own values and
  text, its product banner and arc colour (`tactus-nks::artwork`).
- **Knobs** set the instrument's parameter, the DAW is told (so automation
  can record it), and the screen shows the instrument's text.
- **Page buttons** work on the device. **Preset up/down** step through the
  preset's folder (same plugin).
- **Light guide:** held keys green.
- **Transport lights** follow the DAW's transport (playing, tempo) from its
  process context. The transport *buttons* cannot drive the DAW: VST3 gives a
  plugin no way to control the host's transport.
- **Several shims share the keyboard.** A newly inserted shim takes it. After
  that, the shim whose track is being **played** takes it: Ableton sends MIDI
  to the armed track, which follows selection by default, so the keyboard
  follows the selected track. A background screen update never steals it.
  When the shim holding it is removed, the agent passes it to another.

Needs, on the user's machine: the agent's symbol registry at
`~/Library/Application Support/tactus/symbol_registry.json`, generated from
the user's own agent; nothing of NI's is shipped. Until the settings strip
exists, the default preset comes from
`~/Library/Application Support/tactus/shim.conf` (`preset=/path.nksf`), or
`TACTUS_SHIM_PRESET`.

## Evidence

`crates/tactus-shim/examples/selftest.rs`, with tactus's own host loading the
shim and the shim loading Massive X (Wob Wolfer):

| Check | Result |
|---|---|
| The shim's factory | one class, `Tactus Shim (Audio Module Class)` |
| Parameters through the shim | 2,102, the instrument's |
| Middle C through both layers | peak 0.35–0.43 held, ~0.3 in the release |
| Saved state | ~31.9 KB, `TACTSHIM` header |
| Reopen: a fresh, empty shim given that state | 0 parameters before, 2,102 after; plays (0.36) |
| Editor | the instrument's own view; supports `NSView` |
| Two shims, play one then the other | the keyboard moves to each in turn; a background republish does not steal it |
| Agent log | no complaints after value updates were limited to on-screen ids |

**Not yet done on hardware:** everything above was run without a DAW. Ableton
(goal items 2–4), and opening the window in a real host (item 5), need the
owner. No DAW is installed on the development machine.

## Open

- The **settings strip** (which plugin, which preset folder, which keyboard)
  is not built. The default preset comes from `shim.conf`.
- Hosts that cannot see track selection still work by the played-track rule.
- The keyboard's transport buttons do nothing in a DAW (see above).
- During a DAW's plug-in scan the shim is instantiated, loads its instrument
  and briefly takes the keyboard.
