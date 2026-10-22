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
parameter list and MIDI mapping**, forwarded. Automating a parameter in the
DAW automates the instrument's parameter.

## The window

The instrument's own editor, with a 30-point strip above it (`src/view.rs`).
The strip shows the preset and plugin ("Wob Wolfer — Massive X") and has one
button, **Choose preset…**, which loads any `.nksf` preset and so the VST3
instrument it names. Nothing else lives in the frame's window.

- The instrument's view is a child of the frame's. Its resize requests reach
  the DAW through the frame's own `IPlugFrame`, with the strip added.
- When the instrument is replaced with the window open (the button, or the
  DAW restoring a project), the old view is detached **before** the old
  instrument shuts down, and the new instrument's view is attached.
- The keyboard is the first one the agent reports; there is no selector yet,
  as only one is connected.

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
| Strip in a real window | "Wob Wolfer — Massive X" and **Choose preset…** above Massive X's editor; window 1249×753 = editor + 30 |
| Swap with the window open (`SWAP_TO=…`) | strip reads "Agonic Drone — Massive X", the editor is the new instance's; no crash |
| Editor in a real window (`examples/window.rs`) | attached to an `NSWindow`'s content view as a DAW would: Massive X's full editor, 1249×723, showing the preset loaded through the shim. The screenshot is not in the repo: it is NI's artwork |
| Two shims, play one then the other | the keyboard moves to each in turn; a background republish does not steal it |
| Agent log | no complaints after value updates were limited to on-screen ids |

**Not yet done in a DAW:** everything above ran without one. Ableton (goal
items 2–4) needs the owner; no DAW is installed on the development machine.
The window test stands in for item 5 until then.

## Open

- **Keyboard selector** in the strip: one keyboard is assumed.
- **Choose preset…** was exercised through the same swap path (state restore
  with the window open), not by clicking. A click test needs a person or a
  DAW.
- Hosts that cannot see track selection still work by the played-track rule.
- The keyboard's transport buttons do nothing in a DAW (see above).
- During a DAW's plug-in scan the shim is instantiated, loads its instrument
  and briefly takes the keyboard.
