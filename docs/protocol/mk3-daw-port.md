# Kontrol S MK3 DAW-port protocol — recovered from NI's shipped Cubase script

**Date:** 2026-10-20
**Evidence class:** vendor source, not hardware capture
**Status:** NOT hardware-verified. See "Evidence standing" below.

## Source

`/Library/Application Support/Native Instruments/Hardware/Hardware Connection Service/NativeInstruments_Kontrol_S_Mk3.js`

1066 lines of unobfuscated ASCII JavaScript, shipped by Native Instruments. It is their **Cubase MIDI Remote script** (it requires `midiremote_api_v1`, Steinberg's API), not their internal driver. It therefore describes the **DAW-port MIDI/SysEx protocol** only — it contains nothing about the pixel framebuffer or USB bulk transport.

Komplete Kontrol itself is **not installed** on this machine. What is installed: `Controller Editor`, `NIHardwareAgent`, `NIHostIntegrationAgent`, and this script.

## Evidence standing

This is **documentary evidence authored by the vendor**, which is considerably stronger than inference from captures — but it is still not a hardware observation. Under the project rule that software completion and hardware verification are separate gates, this does **not** close M4's "DAW-port labels and values" gate on its own. It does mean the first hardware capture becomes a *confirmation* exercise against a known expected protocol, rather than blind discovery.

One concrete caveat: this script targets Cubase 13.0.55+ and declares `protocolVersion = 0x04`. A different firmware may negotiate a different protocol version with different semantics. The version byte is the thing to check first.

## Transport

- **Port name** (exact match required by NI's own detection): `KONTROL S88 MK3 DAW` — a port distinct from the main MIDI port. Also accepted: `MIDIOUT2 (KONTROL S88 MK3)` / `MIDIIN2 (...)` on Windows-style naming, and **`Kontrol MK3 Simulator DAW`**.
- **`Kontrol MK3 Simulator DAW` is worth chasing separately** — NI appears to ship or support a software MK3 simulator. If it can be run, it may permit protocol work with no keyboard attached.
- All channel messages use **MIDI channel 16** (`midiChannel = 0x0f`, status byte `0xBF`).

## Handshake

| Step | Bytes | Meaning |
|---|---|---|
| Hello | `BF 01 04` | `protocolVersion = 0x04` |
| Enable 14-bit SysEx param updates | `BF 06 01` | opt-in to higher-resolution parameter updates |
| Identity | SysEx `identity` carrying host app name + major/minor | announces the host |
| Surface config | SysEx `surface_configuration`, value 1, `"track_orientation"` | enables vertical navigation |
| Goodbye | `BF 02 00` | on deactivate |

NI's script queues all messages generated before hello and flushes them afterwards — so the device likely ignores or mishandles traffic sent pre-handshake.

## SysEx framing

```
F0 00 21 09 00 00 44 43 01 00 <status> <value> <track> [info bytes...] F7
```

- `00 21 09` is Native Instruments' MIDI manufacturer ID.
- `00 00 44 43 01 00` is a fixed sub-header (observed constant; its internal field split is **unknown**).
- `<status>` is one of the command IDs below; `<value>` and `<track>` are single bytes.
- `info` is optional payload. For text commands it is raw `charCodeAt` output — i.e. **one byte per character, no length prefix, not UTF-8-aware**. Strings are terminated by the SysEx `F7`. NI's debug logging escapes `\0`, which suggests embedded NULs are used as separators in at least some payloads.

### Command IDs (`sysexIds`, verbatim)

| ID | Name |
|---|---|
| `0x03` | `surface_configuration` |
| `0x07` | `identity` |
| `0x19` | `set_tempo` |
| `0x40` | `track_enabled` |
| `0x41` | `focus_follow` |
| `0x42` | `track_selected` |
| `0x43` | `mute_button` |
| `0x44` | `solo_button` |
| `0x45` | `track_armed` |
| `0x46` | `volume` |
| `0x47` | `pan` |
| `0x48` | `track_name` |
| `0x49` | `vu_meter` |
| `0x4b` | `track_color` |
| `0x70` | `selected_track_select_plugin` |
| `0x71` | `selected_track_plugin_chain_info` |
| `0x72` | `selected_track_param_name` |
| `0x73` | `selected_track_param_display_value` |
| `0x74` | `selected_track_param_page_num` |
| `0x7F` | `adjust_parameter_value` |

`0x4a` is absent from the table — a gap, not necessarily unused by the device.

Device→host SysEx observed being handled: `selected_track_select_plugin` (0x70), `set_tempo` (0x19), `adjust_parameter_value` (0x7F). So the protocol is bidirectional over the same framing.

## Control change map (all channel 16)

### Buttons — device → host
| CC | Control |
|---|---|
| `0x04` | shift |
| `0x10` | play |
| `0x12` | record |
| `0x13` | count-in (toggle) |
| `0x14` | stop |
| `0x16` | loop |
| `0x17` | metronome |
| `0x18` | tap tempo |
| `0x20` | undo |
| `0x21` | redo |
| `0x22` | quantize |
| `0x23` | auto |
| `0x05` | bank mapping select (range 0–1: `mixer_tracks`=0, `plugin_parameters`=1) |

Play, record, stop, loop and metronome bind both input and output ports — they take LED feedback on the same CC.

### Encoders — relative, two's complement
| CC | Control |
|---|---|
| `0x30` | track index |
| `0x31` | bank index |
| `0x32` | clip index |
| `0x34` | move transport |
| `0x35` | move loop |
| `0x64` | selected-track volume |
| `0x65` | selected-track pan |

### Per-channel, 8 channels
| CC | Direction | Control |
|---|---|---|
| `0x42` | in | select (raw, bitfield across channels) |
| `0x43` | in | mute (raw) |
| `0x44` | in | solo (raw) |
| `0x50 + i` | out | volume knob display, i = 0..7 |
| `0x58 + i` | out | pan knob display, i = 0..7 |
| `0x70 + i` | out | track parameter value, i = 0..7 |

Note `0x42`–`0x44` collide numerically with SysEx IDs of the same number; they are different namespaces (CC vs SysEx status) and must not be conflated.

### LED / capability enables — host → device
| Bytes | Meaning |
|---|---|
| `BF 30 03` | enable track navigation (prev+next, bitmask) |
| `BF 31 03` | enable bank navigation |
| `BF 32 03` | enable clip navigation |
| `BF 20 01` | canUndo |
| `BF 21 01` | canRedo |
| `BF 22 01` | canQuantize |
| `BF 50+i 127` | per-channel enable, i = 0..7 |

The `03` value for navigation reads as a two-bit mask (prev | next) — **inferred**, not stated.

## What this source does NOT contain

- Pixel/framebuffer graphics of any kind
- USB bulk transport, endpoints, or interface numbers
- Light-guide / per-key RGB LED control
- Display resolution or pixel format

Those live in `NIHardwareAgent` and are being analysed separately.

## Suggested verification order (when hardware is attached)

1. Open the `KONTROL S88 MK3 DAW` port, capture input only, confirm the port name matches exactly.
2. Send nothing. Baseline capture to see what the device emits unprompted.
3. Send hello `BF 01 04` and capture the response. Confirm the device accepts protocol version 4.
4. Send one `track_name` SysEx with a short ASCII string and observe the screen.
5. Only then try `vu_meter` and `track_color`.

Each step is a separate capture fixture. Record firmware version in the metadata — the protocol version byte makes firmware a load-bearing variable here.

---

# Addendum: what has since been confirmed, and one latent bug

Added while implementing `tactus-midi`.

## Port naming is now hardware-confirmed — with a trap

Verified against a connected S88 MK3 through CoreMIDI:

```
device   "KONTROL S88 MK3"   manufacturer "Native Instruments"
  entity "Main"   -> "KONTROL S88 MK3 Main"
  entity "DAW"    -> "KONTROL S88 MK3 DAW"
  entity "Ext"    -> "KONTROL S88 MK3 Ext"
```

The literal string NI's script matches, `KONTROL S88 MK3 DAW`, exists **only as
device name + entity name**. An endpoint's own name property returns the bare
word `DAW`, which is ambiguous across vendors.

**Second trap:** CoreMIDI retains offline devices. With one keyboard attached,
enumeration listed **two** devices named `KONTROL S88 MK3` among fifteen, only
one with live endpoints. Matching on name alone can select a device that is not
physically present.

`tactus_midi::port` therefore matches on manufacturer + device name + role over
**online endpoints only**, and reports multiple matches as an error rather than
choosing — with two identical keyboards attached, guessing would send output to
the wrong instrument.

## A latent bug in NI's own text encoding

Text payloads are built with JavaScript `charCodeAt`: one byte per UTF-16 code
unit, **no length prefix**, running to the terminating `F7`.

That is safe only for **7-bit ASCII**. Any character above `U+007F` produces a
code unit `>= 0x80`, which inside a SysEx frame is read as a **status byte** and
corrupts the remainder of the message. A track named `Café` would break it, and
NI's script contains no guard.

`tactus_midi::daw::encode_text` **rejects** non-ASCII rather than truncating or
mangling it, and says so in the error. A host that quietly corrupts a name is
worse than one that reports it cannot send it; callers wanting best-effort
display should transliterate first.

## Namespace collision worth knowing

CC numbers and SysEx command ids overlap numerically and are unrelated:

| Number | As a CC | As a SysEx command |
|---|---|---|
| `0x42` | per-channel select (raw bitfield) | `track_selected` |
| `0x43` | per-channel mute | `mute_button` |
| `0x44` | per-channel solo | `solo_button` |

Conflating them is an easy and unpleasant bug; `tactus-midi` has a test pinning
the distinction.

## Relative encoder decoding

NI marks the navigation and selected-parameter encoders
`setTypeRelativeTwosComplement`. Values `0x01..=0x3F` are positive deltas;
`0x41..=0x7F` are negative, read as 7-bit two's complement. Decoding this wrong
makes a one-step decrement appear as `+127`, so it has its own function and a
round-trip test across the full representable range `-64..=63`.

## Still not hardware-verified

Everything in the main body above remains **vendor source**. No session has been
observed on the wire. The first confirmation step is cheap and worth doing
before relying on any of it:

1. Open `KONTROL S88 MK3 DAW`, capture input only, send nothing. Baseline.
2. Send `BF 01 04` and observe whether the device accepts protocol version 4.
3. Send one `track_name` SysEx with a short ASCII string; watch the screen.

Record the firmware version alongside: the protocol version byte makes firmware
a load-bearing variable.

---

# Addendum 2: step 1 of the verification order, done

**Evidence class: hardware observation.** An attached S88 MK3, firmware
revision `bcdDevice = 0x0204`. Input captured on all three ports
simultaneously; **nothing was transmitted**, so this is the clean baseline step
1 asks for.

## What the keybed sends

Playing one key, verbatim from the capture:

```text
Main | note on   ch1  note 43  vel 36   | 90 2B 24
Main | poly at   ch1  note 43  val 89   | A0 2B 59
Main | poly at   ch1  note 43  val 94   | A0 2B 5E
...                                       (≈60 updates over 2.2 s)
Main | poly at   ch1  note 43  val 127  | A0 2B 7F
Main | poly at   ch1  note 43  val 0    | A0 2B 00
Main | note off  ch1  note 43  vel 50   | 80 2B 32
```

Two things worth recording:

- **Polyphonic aftertouch**, `0xA0`, addressed per note — not channel
  aftertouch (`0xD0`). The keybed tracks pressure per key.
- **Note-off carries a release velocity** (`0x80 2B 32`), rather than the
  note-on-with-zero-velocity form.

## What the control surface sends: nothing

Buttons, knobs and the buttons under the display were operated throughout. On
`Main`, `DAW` and `Ext`, with NI's four background agents running *and* with
all of them stopped:

**Zero control-change messages. Zero SysEx.**

So the channel-16 CC map in the main body is not merely unverified — it is
**not passively observable**. It cannot be confirmed by listening, which is
what step 1 was hoping for.

The reading that fits both this and the ODR findings: the CC map is a mode the
device enters when a host claims the DAW port, and the handshake in step 2 is
what claims it. That is consistent with the map containing *host → device*
"LED / capability enables", which only make sense once a conversation has
started.

It also means the control surface has **two** possible routes — ODR (what the
vendor uses, see [mk3-usb-interfaces.md](mk3-usb-interfaces.md)) and the DAW
port after a handshake (what a third-party DAW uses). For this SDK the second
is far more attractive: it is ordinary MIDI, it needs no vendor USB transport,
and the semantics arrive already named.

## Status of the remaining steps

| Step | Status |
|---|---|
| 1. Baseline capture, send nothing | **done** — above |
| 2. Send `BF 01 04`, observe the response | not done |
| 3. Send one `track_name` SysEx, watch the screen | not done |

Step 2 is now the highest-value experiment in this project: it is three bytes
of ordinary MIDI control change from the vendor's own script, and if it does
switch the surface on, the entire named CC map becomes verifiable and semantic
control naming is solved without implementing ODR at all.
