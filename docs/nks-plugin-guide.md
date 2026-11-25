# Making a plugin work on a Kontrol keyboard

What tactus has learned about putting a plugin on a Kontrol S-series MK3
keyboard the NKS way: what the plugin must do, what its preset files contain,
what artwork and colour it can ship, and what the keyboard does with all of
it. It is written for **plugin authors** first and **host authors** second.

This is a clean-room document. It was learned from hardware the project owns,
files installed on the development machine, and the observable behaviour of
the vendor's software. It is **not** Native Instruments' NKS SDK, which is
under NDA and was not used. "NKS" is NI's trademark and is used here only to
describe compatibility.

## How sure is each claim

Every section says where its facts come from:

| Label | Meaning |
|---|---|
| **Hardware** | Seen on an S88 MK3 and confirmed by its owner |
| **Run** | Exercised by tactus code against real plugins or the vendor's agent |
| **Survey** | True of all 625 `.nksf` presets on the development machine (Super 8 R2 ×561, Massive X ×64) |
| **Decompiled** | Read from the vendor's binaries; not yet exercised |
| **Inferred** | Our best reading, not demonstrated |

---

## 1. The short version

An NKS plugin is an **ordinary VST3 plugin**. Nothing NKS-specific runs
inside it. It becomes "NKS" through what ships beside it and through a few
habits in how it is built:

1. **Stable parameter ids.** The keyboard's knob pages refer to your VST3
   `ParamID`s. Never renumber them between versions.
2. **Good value text.** The keyboard shows what `getParamStringByValue`
   returns for every knob. Make it short and readable ("70.9 %", "440 Hz").
3. **Complete state.** A preset's plugin data is exactly what your
   `IComponent::getState` writes. `setState` must restore everything from it.
4. **Say when a preset has landed.** If you apply state in the background,
   call `restartComponent(kParamValuesChanged)` when the values are in place.
5. **Map your controllers.** Implement `IMidiMapping` so mod wheel, pitch bend,
   sustain and aftertouch reach your parameters.
6. **Ship `.nksf` presets** with eight-knob pages, section names and browser
   tags, as described in section 3.
7. **Ship a banner and a colour,** optional but they make the screen yours
   (section 4).
8. **Ship arm64.** An Intel-only bundle cannot load into a native host on
   Apple Silicon.

tactus can read and write the preset format (`crates/tactus-nks`), host your
plugin (`crates/tactus-plugin-host`), and drive the keyboard
(`crates/tactus-odr`), so you can test all of this without Komplete Kontrol.

---

## 2. Your plugin

### Format and identity

- **VST3.** Every behaviour below was run against VST3 builds. VST2 is not
  supported by tactus (its SDK is no longer licensed). AU and CLAP are not yet
  hosted.
- **Class id.** Presets name your plugin by its VST3 audio class id, stored as
  four 32-bit words (section 3, `PLID`). On macOS the 16-byte TUID is those
  four words **big-endian** in order; tactus's `class_id([u32; 4])` builds it,
  and it matched Massive X's factory class exactly. **Run.** (On Windows, VST3's
  COM byte order differs; the same four words become a different byte layout.
  **Inferred** from the VST3 headers.)
- **Bundle name.** tactus finds a plugin by the preset's `pluginName`:
  `~/Library/Audio/Plug-Ins/VST3/<pluginName>.vst3`, then
  `/Library/Audio/Plug-Ins/VST3/<pluginName>.vst3`. Keep the bundle's name
  equal to the name your presets carry.
- **Architecture.** Ship a universal (x86_64 + arm64) binary. Super 8's VST3
  is x86_64-only and cannot load into an arm64 host. **Run.**

### Single object or split

Massive X implements processor and controller on one object; tactus handles
both that and the split form (controller created from
`getControllerClassId`, the two joined with `IConnectionPoint` and host-made
`IMessage`s). **Run** for single-object; the split path is implemented but has
not met a real split plugin yet.

### Parameters

The keyboard is a view onto your `IEditController`:

| The keyboard needs | It comes from | Standing |
|---|---|---|
| Which parameter a knob is | the `id` in the preset's knob page = your `ParamID` | **Run**: all 20 of Wob Wolfer's knob ids were Massive X parameters |
| The knob's position | `getParamNormalized(id)`, 0–1 | **Hardware** |
| The text under it | `getParamStringByValue(id, value)` | **Hardware**: Massive X's own "70.9 %" |
| A turn | the host adds the device's relative step and calls `setParamNormalized`, and passes the change to your processor as a parameter change | **Hardware** |

Knob turns arrive as small relative steps (about ±0.002–0.02 per detent), so
a parameter with very few steps will feel sticky. The device also reports
touch and release on each knob. **Hardware.**

The knob label on screen comes from the **preset's** page, not from your
`ParameterInfo.title`, so it can be shorter than your full parameter name.

### State and presets

- A preset's plugin data (`PCHK`) is **your `getState` output, unchanged**. In
  Massive X both carry the same header layout and a length field matching the
  blob. **Run.** Hosts hand it to `IComponent::setState`, then to
  `IEditController::setComponentState`.
- **Background loading is fine, but signal it.** Massive X accepts `setState`
  at once and applies the preset over the next second or two, and only while
  its audio engine is running and being processed in real time. When the values
  are in place it calls `restartComponent(0x1c)`: parameter values, latency
  and titles changed. tactus waits for `kParamValuesChanged` before showing the
  new values. A warm engine applied a preset switch in **11 ms**, and no value
  moved after the signal. **Run.**
- If your plugin applies state synchronously, values must be correct as soon
  as `setState` returns.

### MIDI controllers

Implement `IMidiMapping::getMidiControllerAssignment`. tactus asks for every
CC 0–127 plus `kAfterTouch` (128) and `kPitchBend` (129) on bus 0, channel 0,
and routes the keyboard's wheels, pedals and pressure to the parameters you
return. Massive X maps all 130. **Run** (the keyboard side awaits a hardware
check). Poly pressure (the S88 MK3's default aftertouch mode) arrives as VST3
`PolyPressureEvent`s, not through the mapping.

### Your editor

Hosts open your editor through `createView("editor")` and attach it to an
`NSView`. Massive X reports its size (1249×723) only **after** `attached`; a
host must ask again then. Resize requests go through the `IPlugFrame` the
host gives you. **Run** in a real window.

### What not to do

- Don't renumber parameters.
- Don't change the meaning of a saved state between versions without reading
  old ones.
- Don't rely on a particular host: tactus's shim runs your plugin inside
  another VST3 inside a DAW, and its own host runs it standalone.

---

## 3. The preset file (`.nksf`)

**Survey** across all 625 files, and **Run**: `tactus-nks` reads every one and
rewrites every one **byte-for-byte identical**
(`cargo run -p tactus-nks --example roundtrip -- <files>`).

### Container

A RIFF file of form `NIKS`:

```text
"RIFF"  u32 size (= file length - 8)  "NIKS"
  chunk "NISI"   browser metadata
  chunk "NICA"   knob pages
  chunk "PLID"   which plugin
  chunk "PCHK"   the plugin's state
```

Every chunk is `id`, `u32` little-endian length, then a body that starts with a
`u32` **version, always `1`**, followed by one msgpack value (for `PCHK`, the
raw state). Odd-length chunks are padded with one zero byte. All 625 files use
exactly these four chunks in this order.

### `NISI`: browser metadata

A msgpack map with string keys:

| Key | Type | Example | Present in |
|---|---|---|---|
| `author` | string | `"Jamil Samad"` | all |
| `bankchain` | list of strings: product, bank, sub-bank | `["Massive X", "Massive X Player Library", ""]` | all |
| `characters` | list of strings | `["Synthetic", "Monophonic"]` | newer presets |
| `comment` | string | `""` | all |
| `deviceType` | `"INST"` or `"FX"` | `"INST"` | all |
| `modes` | list of strings | `["Synthetic"]` | most |
| `name` | string: the preset's name | `"Wob Wolfer"` | all |
| `types` | list of [type, sub-type] lists | `[["Bass"], ["Bass", "Synth"]]` | almost all |
| `uuid` | string | `"99be1d21-…"` | newer presets |
| `vendor` | string | `"Native Instruments"` | all |
| `__ni_internal` | string, not interpreted | | NI's newer presets |

Details that matter for a byte-faithful writer:

- Keys are written in **byte order** (`__ni_internal` sorts first).
- Files differ on **empty lists**: one leaves `types` out entirely, others
  keep `"characters": []`. `tactus-nks` remembers which keys a file had
  (`Info::keys`) and writes exactly those back.
- **`bankchain[0]` is the product name.** It is how hosts find your banner and
  colour (section 4), so it must match your product's resource folder name.

### `NICA`: knob pages

```text
{ "ni8": [ page, page, … ] }     a page is a list of 8 slots
slot = { "autoname": bool, "id": ParamID, "name": string,
         "section": string (optional), "vflag": bool }
empty slot = { "autoname": bool, "vflag": bool }
```

- **Eight slots per page** is the keyboard's eight knobs. Super 8 R2 ships ten
  pages; Massive X three.
- **`section`** on a slot starts a new section header there; slots without it
  continue the current section. Slots before the first header form an unnamed
  section. The same section name may repeat. **Hardware**: the keyboard drew
  Super 8 R2's own headers ("Filter" over Cutoff, Reso, Env Amt, FM Osc 2;
  "Filter Env" over the next four).
- **`name`** is the label under the knob; it may be empty.
- **`id`** is your `ParamID`.
- **`autoname`, `vflag`:** meaning not established. Empty slots carry them
  too, sometimes `autoname: true`. Write them back as read.
- A preset may have **no pages**: `NICA` is then an empty map `{}` (4 of 625).
  The key `ni8` presumably means NKS1's eight-knob layout. NKS2 layouts were
  not seen in any preset file.

### `PLID`: which plugin

```text
VST2-only:  { "VST.magic": u32 }
VST3:       { "VST.magic": 0, "VST3.uid": [u32, u32, u32, u32],
              "pluginName": string, "pluginVendor": string }
```

`VST.magic` is the VST2 unique id; as a four-character code Super 8's is
`NIS8`. `VST3.uid` is the class id (section 2). `pluginName` must match your
`.vst3` bundle's name for tactus to find it. Seen: 565 VST2-only, 60 VST3.

### `PCHK`: your state

The version word `1`, then your `getState` bytes, untouched. Nothing reads
inside it; only your plugin does.

### Where presets go

NI's own products install them in
`/Library/Application Support/Native Instruments/<Product>/Presets/`. tactus
steps through a preset's folder with the keyboard's preset buttons, in file
name order. Where the vendor's browser looks for third-party presets, and how
it indexes them, has **not** been studied.

### Writing one

```rust
use tactus_nks::{Info, Page, PluginId, Preset, Slot};

let knob = |id, name: &str, section: Option<&str>| Some(Slot {
    id, name: name.into(), section: section.map(Into::into),
    autoname: false, vflag: false,
});
let preset = Preset {
    info: Info {
        name: "Warm Pad".into(),
        vendor: "Your Company".into(),
        author: "You".into(),
        bankchain: vec!["Your Synth".into(), "Factory".into(), "".into()],
        types: vec![vec!["Pad".into()]],
        device_type: "INST".into(),
        ..Default::default()
    },
    plugin: PluginId {
        vst_magic: 0,
        vst3_uid: Some([0x1234_5678, 0x9ABC_DEF0, 0x1122_3344, 0x5566_7788]),
        name: Some("Your Synth".into()),
        vendor: Some("Your Company".into()),
    },
    pages: vec![Page {
        slots: vec![
            knob(10, "Cutoff", Some("Filter")), knob(11, "Reso", None),
            None, None,
            knob(20, "Attack", Some("Amp")), knob(21, "Decay", None),
            knob(22, "Sustain", None), knob(23, "Release", None),
        ],
        ..Default::default()
    }],
    plugin_state: your_plugin_state, // from IComponent::getState
    ..Default::default()
};
preset.write("Warm Pad.nksf")?;
```

The same code, compiled and runnable, is `crates/tactus-nks/examples/write_preset.rs`.
With tactus's host, the state of a loaded plugin is `Plugin::save_state()`.

**Not yet verified:** whether the vendor's own browser accepts presets written
this way. tactus's reader, host and keyboard path do.

---

## 4. Banner and colour

### The banner

The keyboard shows one image: a **1280×212** band across the top of the
parameter page, under which it draws the knobs itself. **Hardware.**

- NI's products ship it as `NKS2_hardware_banner.webp` (WebP lossless,
  1280×212) in `…/PAResources/image/<Product>/`. 1,616 such files were found,
  all that size, plus 317 at **1280×152** (`NKS2_hardware_banner_1280x152.webp`)
  whose use is unknown.
- The host sends it once as an asset (PNG or WebP; its id is the SHA-256 of the
  bytes) and references it from the page. The device decodes it.
- **Nothing can be drawn behind the knobs.** The page model has one image
  field, the header.
- tactus looks for it at
  `/Library/Application Support/Native Instruments/<Product>/PAResources/image/<Product>/`
  and in folders up to two levels below that root, with `<Product>` =
  `bankchain[0]`.

### The arc colour

The keyboard colours each knob's value arc with the page's
`nks_control_color`. With none, arcs are white. **Hardware.**

NI keeps a colour per product in a SQLite table,
`…/PAResources/_DatabaseResources/color/color.db`:

```text
color(Location text, Name text, Value text, seqidx integer)
Location = product name   Name = "NKS2_control_color"   Value = "FC6949"
```

(`Super 8 R2` → `FC6949`, `Massive X` → `FF4D00`, `FM8` → `EBE9EA`.) Sibling
rows `MST_bgcolor` and `VB_bgcolor` are browser tile backgrounds. The page's
other colour, `plugin_color`, changed nothing visible on the parameter page.
**Hardware.**

### Other images

The same folders hold `MST_artwork.png`, `MST_logo.png`, `MST_plugin.png`,
`VB_artwork.png`, `VB_logo.png`, `OSO_logo.png` and
`NKS2_software_tile.webp`. They are presumably for the vendor's browsers
(MST and VB being browser views). tactus does not use them.

---

## 5. What the keyboard does with it

For host authors; plugin authors can skip this. Full detail is in
`docs/protocol/mk3-odr-ipc.md`, `docs/shim.md` and `docs/hosting.md`.

| Feature | How | Standing |
|---|---|---|
| Talking to the keyboard | msgpack-RPC over the vendor agent's local socket; methods and keys are registry indices | **Run** |
| Knob pages | `client_parameter_page_set_data`: every NKS page at once, each a list of sections of `{id, name}` or `nil` | **Hardware** (10 pages, repeated section names, empty slots) |
| Page buttons | the device turns pages itself and reports `plugin_parameter_set_page_index` | **Hardware** |
| Preset buttons | `browser_load_next_sound` / `…_previous_sound`, both with payload `-1`: direction is the action name | **Hardware** |
| Knob turns | `plugin_parameter_value_change [id, delta]`; the host applies it and echoes value and text | **Hardware** |
| Transport lights | host state (`transport`, `recording`, `metro`, `loop`, `tap_tempo`, `count_in`, `can_quantize`) | **Hardware**, 12 of 14 states; Undo/Redo stayed dark; nothing blinks |
| Light guide | per MIDI note, a colour byte (green `0x1C`, red `0x04`, off `0`) | **Hardware** |
| Banner, arc colour | above | **Hardware** |
| Parameter kinds | continuous (knob or value), bipolar, discrete (value, menu, waveform, filter type), toggle (value, power, tempo sync) | **Decompiled**; only continuous with knob style is **Hardware** |

The keyboard does not draw anything a plugin sends as pixels: it renders its
own page from this model. A plugin's own window is a separate matter, shown on
the computer.

---

## 6. Open questions

- Does the vendor's browser accept presets written by `tactus-nks`?
- What do `autoname` and `vflag` mean?
- How are NKS2 layouts stored in presets, and what does the 1280×152 banner
  belong to?
- How do discrete, toggle and bipolar parameters render (menus, waveforms,
  filter shapes, power buttons)?
- Where does the vendor's software look for a third-party product's artwork
  and colour?
- For VST2-only presets, `PCHK` is presumably the VST2 chunk. tactus cannot
  host VST2, so this is untested.

Small open ends across the project are in `docs/LATER.md`.

---

## 7. Using tactus to test your plugin

| Step | Command |
|---|---|
| Read a preset | `cargo run -p tactus-nks --example nksinfo -- <file.nksf>` |
| Check a writer round trip | `cargo run -p tactus-nks --example roundtrip -- <files…>` |
| Load the plugin, apply the preset, list its knobs | `cargo run -p tactus-plugin-host --example probe -- <file.nksf>` |
| Play it from the keyboard, with its pages on screen | `cargo run -p tactus-plugin-host --example kontrol -- <file.nksf>` |
| Run it inside a DAW, on the keyboard | install `tools/bundle-shim.sh release --install`, then load **Tactus Shim** |

The keyboard examples need the vendor's agent running (it ships with NI's
hardware support) and the agent's symbol registry at
`~/Library/Application Support/tactus/symbol_registry.json`.
