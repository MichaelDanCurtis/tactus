# Tactus — program plan

**Status:** draft for review. Nothing here is committed engineering yet.
**Working name:** `tactus` is provisional and device-neutral. See §8 on naming.

## 1. What we are building

Two deliverables, open-sourced.

**D1 — An SDK and complete protocol documentation** that lets anyone write a host for
Native Instruments Kontrol S-series MK3 keyboards, then MK2, then Maschine MK2, and
later Akai MPC hardware. Rust core, stable C ABI, so C/C++/Python/JS/Swift/Max can all
bind to it.

**D2 — Our own host:** clean, lightweight, fast, doing what Komplete Kontrol does
without the bloat. For scale, the thing we are replacing currently runs six background
processes, ~260 MB resident, a cloud RPC client, analytics telemetry, and aborts the
entire application on an uncaught exception when one directory is missing.

## 2. Why this is a program, not a project

"Everything Komplete Kontrol does" contains two problems that have almost nothing in
common:

- **Driving the hardware.** USB/HID/MIDI, light guide, display, input. Mostly already
  reverse-engineered — see §4. Weeks of work.
- **NKS.** Preset browsing with auto-mapped plugin parameters. This requires a
  **VST3/AU plugin host** plus a parameter-mapping database. It is the largest single
  piece of the program, larger than the keyboard protocol, and it has its own
  independent failure modes.

Binding those together would mean the keyboard work cannot ship until the plugin host
works. They are therefore separate projects with a clean interface between them.

## 3. Project map

```
A. tactus-core ────────────────┐
   device SDK + protocol docs  │
   (D1)                        │
                               ├──► C. tactus-host  (D2)
B. nks ────────────────────────┘        the KK replacement
   preset format + plugin host
   (the long pole)

D. Maschine MK2   — reuses A's architecture, later
E. Akai MPC       — reuses A's architecture, later
```

### A. `tactus-core` — the SDK

| Sub | Scope | Gating unknowns |
|---|---|---|
| A1 | Device layer: enumeration, identity, hotplug, HID/MIDI/USB transports | none |
| A2 | Input: keys, aftertouch, pedals, wheels, touch strips, knobs, buttons, encoders | none |
| A3 | Light guide: per-key RGB | semantics of `OUT 0x82` payload |
| A4 | Display: pixel arm (MK2/Maschine) **done**; page arm (MK3) partial | ODR payload schemas, method-index registry |
| A5 | C ABI + generated header | none |
| A6 | Protocol documentation (this is a D1 deliverable, not a byproduct) | tracks A1–A4 |

### B. `nks` — split further, because these fail independently

| Sub | Scope | Difficulty |
|---|---|---|
| B1 | NKS preset/metadata file format reader | moderate; partly community-documented |
| B2 | Preset database and browser model | moderate |
| B3 | **VST3/AU plugin host** | hard; the long pole of the whole program |
| B4 | Parameter auto-mapping: NKS pages → knobs and display | moderate, depends on B1+B3 |

B3 is the risk. Rust's plugin-*hosting* ecosystem is far less mature than its
plugin-*authoring* ecosystem, so expect to evaluate wrapping an existing C++ host core
against writing one.

### C. `tactus-host`

Consumes A, and later B. Shippable and genuinely useful **before** B exists: a fast,
quiet host that drives the keyboard and talks to a DAW beats the incumbent for a lot of
real use even with no plugin hosting at all.

## 4. What is already established

Carried over from the reverse-engineering work in
`../KontrolS88MK3/docs/protocol/` and `../KontrolS88MK3/docs/s88-protocol-notebook.md`.

**Evidence classes** (this discipline carries into this repo): hardware observation >
vendor artifact > community inference. A claim is promoted to class 1 only by an
observation, never by two lower-class sources agreeing.

| Item | Value | Class |
|---|---|---|
| USB identity | VID `0x17CC`; S49 `0x2100`, S61 `0x2110`, **S88 `0x2120`** | hardware |
| Display interface match | class `0xFF`, subclass `0xBD`, first bulk-OUT; endpoint not hardcoded | vendor binary |
| HID reports (real descriptor) | IN `01/02/03/40/AA`; OUT `80/81/82/A0–A4/AF/F3/F4`; FEATURE `D0/D8/D9` only | **hardware** |
| Light guide | `OUT 0x82`, one byte per key — `0x31`/`0x3D`/`0x58` = 49/61/88 keys | report hardware-confirmed |
| Device identity | serial via `0xD9`; `protocol version 2.4.0` | **hardware** |
| MIDI topology | entities `Main` / `DAW` / `Ext`; full name `KONTROL S88 MK3 DAW` | **hardware** |
| DAW-port protocol | SysEx `F0 00 21 09 00 00 44 43 01 00 …`, 20-command table, handshake `BF 01 04`, full channel-16 CC map | vendor source |
| NHL2 display stream | 1-byte opcode + 24-bit BE word count; literal/repeat/skip/end; RGB565 `0x60`, BGR565 `0x61`, big-endian in word; frame `84 00 \| idx \| fmt \| 0000 0000 \| x y w h` | vendor binary |

**Corrections already recorded** (do not re-import): no 4-byte length header; light guide
is keybed-sized not 128 note-indexed; no evidence for an `A0 00 00` "legacy LED mode";
NI's own report-map table is **wrong** in two places against the real device, so always
confirm against the live HID descriptor.

## 5. The architectural question — ANSWERED

**The MK3 keyboard renders its own UI. The host does not send pixels.**

Settled by decompiling the vendor's page-selection path and its symbol-to-enum
lookup table. Full write-up and evidence in
[protocol/mk3-display.md](protocol/mk3-display.md). Headlines:

- The host selects one of four pages — `browser=0, mixer=1, parameter=2,
  smartplay=3`, one byte — and supplies each a typed view-model.
- The entire display surface is semantic (`set_trackinfo`, `set_meters`,
  `parameter_page_set_model`, `browser_set_sounds`, `leds_set_lightguide`, …).
  There is no `set_pixels`, `set_image`, `blit` or `draw`.
- `renderer_version` is a property the **host queries from the device**.
- Wire format is **msgpack-RPC**, size-prefixed, over a Unix socket — which
  vindicates a community claim this project had earlier marked *questionable*.
- A **software simulator** exists (`kks_mk3_simulator::prepareTCP`): the same
  protocol over TCP, with no keyboard attached.

**Consequences:**

1. **There is no MK3 framebuffer to reverse-engineer, and no geometry to find.**
   The earlier "post-v1 framebuffer spike" was chasing something that does not
   exist for this generation. Dropping it is the single largest scope reduction
   available, and it came from evidence rather than fatigue.
2. A4 for MK3 is a documented RPC protocol — a smaller, more stable target than
   a raster format with an unknown panel size.
3. The pixel path remains necessary for **MK2, Maschine and Traktor**, whose
   geometry is known. Hence two display arms, not one.

The reasoning that led here is preserved below, because it is also the evidence
that the negative is sound rather than a failed search:

- The device DB stores all three MK3 keyboards as `0×0`, format `0xFF` — a sentinel,
  verified against known-good siblings (S49 MK2 = `0x60`, 480×272; Traktor S4 MK3 =
  `0x21`, 320×240).
- No `Komplete::KontrolMK3` class exists in any NHL2 binary.
- `eUSBDeviceType 4` (the MK3 bulk-display DB entry) is never requested by any examined code.
- Komplete Kontrol compiles in **no geometry for any device**, validated against a
  working MK2 control (`getDisplayDataFormat()` is literally `MOVZ w0,#0x60; RET`).
- Hardware: `0xF8` is not implemented; `D0`/`D8`/`D9` carry no geometry even with
  Komplete Kontrol live (`D0` reads all zeros).
- The MK3 instead appears on an **ODR** stack:
  `ni::odr::agent::kks::ipc_mediator::handle_instance_display_set_page(...)`, with
  msgpack over a Unix socket at
  `/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks`.

All of which the answer above confirms.

**Status of A4:** the display layer now exists, with both arms. The pixel arm is
complete and byte-exactly tested. The page arm models page selection (fully
recovered) and the capability surface (observed), but not payload schemas — those
need the method-index registry enumerated or a session observed.

## 6. Open-source boundaries

Deliberate design constraints, because this is going public.

- **Do:** document the protocol, write original implementations, test against hardware
  the user owns. Interoperability reverse engineering is explicitly protected under the
  EU Software Directive Art. 6 and the US DMCA §1201(f) interop exception.
- **Do not redistribute NI material:** their MIDI Remote script, artwork, fonts, icons,
  library metadata, or any extracted binary blob. Document formats; ship no assets.
- **Do not defeat licensing.** Reading an NKS file the user owns to recover preset
  metadata is interop. Bypassing a plugin's license check is not, and would discredit
  the whole project. Keep B1 (read metadata) strictly separate from B3 (host plugins the
  user is licensed to run).
- **No vendor naming.** No NI or Akai trademarks in the project name, package names, or
  anything implying endorsement. Device *identification* strings are fine and necessary.
- **License:** recommend **Apache-2.0** over MIT. Same permissiveness, plus an explicit
  patent grant — which matters more than usual for hardware-interop code.

## 7. Sequencing, with early shippable value

| Milestone | Contents | Useful on its own? |
|---|---|---|
| M1 | A1 + A2 + A5 + A6 — device enumeration, full input, C ABI, docs | Yes: anyone can write a host against it |
| M2 | A3 — light guide | Yes: learning/visualisation apps become possible |
| M3 | A4 — display, shape per §5 | Yes: completes the surface |
| M4 | C on top of A — the lightweight host, no NKS | **Yes: already better than the incumbent for many users** |
| M5 | B1 + B2 — NKS preset format and browser | Yes: browsing without hosting |
| M6 | B3 + B4 — plugin host and auto-mapping | Completes D2 |
| M7 | D — Maschine MK2 | Second device validates the SDK's abstractions |
| M8 | E — Akai MPC | |

M4 is the point at which this replaces Komplete Kontrol for day-to-day control use.
M6 is the point at which it replaces it entirely.

A second device (M7) is also the real test of whether the SDK abstractions are right.
Expect to revise A1–A5 at that point; design for it rather than being surprised.

## 8. Naming

`tactus` is a placeholder chosen to be device-neutral and trademark-clean. Rename cost
is a directory move plus package names — cheap now, annoying after publication. Decide
before M1 ships.

## 9. Immediate next steps

1. Resolve §5. Everything about A4 depends on it.
2. Confirm this decomposition, then write a full spec for **M1** only — not the program.
3. Scaffold `tactus-core` with the C ABI boundary present from the first commit, so it
   constrains the API rather than being bolted on.
