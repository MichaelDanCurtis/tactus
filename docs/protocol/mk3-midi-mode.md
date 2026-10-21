# Kontrol S-series MK3 — MIDI mode control map

**This is the answer to semantic control naming**, and most of it was published
by the vendor all along. See [Where this should have started](#where-this-should-have-started).

## Evidence classes used here

| Class | Meaning |
|---|---|
| **vendor docs** | NI's published online manual. Authoritative for the factory template. |
| **hardware** | Observed on an attached S88 MK3 in MIDI mode, firmware `bcdDevice = 0x0204`. |
| **hardware, undocumented** | Observed on hardware and **absent** from the vendor's table. |

## Modes matter

The S-MK3 switches modes, and a control's behaviour depends on which mode it is
in. Every observation on this page is **MIDI mode**.

Per the vendor manual, the keyboard enters MIDI mode on `SHIFT` + `SETTINGS
(MIDI)`, and *automatically* when the standalone Kontakt or Komplete Kontrol
application closes, when a DAW track without one of those plug-ins is selected,
or **when there is no USB data connection**.

That last clause matters for this project's earlier results: stopping NI's
background agents is itself likely to have put the device into MIDI mode. Tests
run "with the agents stopped" were therefore not testing a neutral device —
they were testing MIDI mode.

The manual also notes that NKS content can override button and knob
configuration when connected to an NKS-compatible product, so the factory
template is not what a Komplete Kontrol session sees.

## Factory template — knobs below the display

All on **MIDI channel 1**, absolute, range 0–127. *(vendor docs)*

| Control | Page 1 CC | Page 2 CC |
|---|---|---|
| Knob 1 | **14** | 22 |
| Knob 2 | **15** | 23 |
| Knob 3 | 16 | 24 |
| Knob 4 | 17 | 25 |
| Knob 5 | 18 | 26 |
| Knob 6 | 19 | 27 |
| Knob 7 | 20 | 28 |
| Knob 8 | 21 | 29 |

The Page buttons (left/right arrows) switch the knob bank; a dimly lit Page
button means another page exists, and the current page is shown at the far right
of the display. **Paging applies to the knobs only**, not the buttons.

> The manual's "Control Function" column labels CC 14, 15, 20–29 as *Undefined*
> and CC 16–19 as *General Purpose Controller 1–4*. Those are the **MIDI
> specification's** names for those controller numbers, not a statement that the
> knob is unassigned. The CC number column is the assignment.

## Factory template — buttons below the display

All on **MIDI channel 1**, toggle, action on down, off = 0 / on = 127.
*(vendor docs)*

| Control | CC |
|---|---|
| Button 1 | 112 |
| Button 2 | **113** |
| Button 3 | 114 |
| Button 4 | **115** |
| Button 5 | 116 |
| Button 6 | 117 |
| Button 7 | 118 |
| Button 8 | 119 |

## Keybed — sends in every mode

*(vendor docs, confirmed on hardware)*

Keys send Note and **Polyphonic** Key Pressure on MIDI channel 1. Observed:

```text
Main | note on   ch1  note 43  vel 36   | 90 2B 24
Main | poly at   ch1  note 43  val 89   | A0 2B 59     ← ~60 updates over 2.2 s
Main | poly at   ch1  note 43  val 0    | A0 2B 00
Main | note off  ch1  note 43  vel 50   | 80 2B 32     ← carries release velocity
```

Per-key pressure, not channel aftertouch. Note-off carries a release velocity
rather than using note-on-with-zero-velocity.

The pitch wheel, modulation wheel and touch strip can send control change,
pitchbend or program change, selected in Keyboard Settings. **The manual gives
no default CC numbers for them**, so they are per-unit configuration rather
than a protocol fact.

## Hardware confirmations

Each of these was read off the wire and agrees with the vendor table:

| Control | Observed bytes | Decodes to | Vendor table |
|---|---|---|---|
| Knob 1 | `B0 0E 27` | ch1 CC 14 = 39 | Knob 1 = 14 ✓ |
| Knob 2 | `B0 0F 07` … `B0 0F 2E` | ch1 CC 15, smooth 7→46 | Knob 2 = 15, absolute ✓ |
| Button 2 | `B0 71 7F` | ch1 CC 113 = 127 | Button 2 = 113 ✓ |
| Button 4 | `B0 73 7F` | ch1 CC 115 = 127 | Button 4 = 115 ✓ |

Two out of eight knobs and two out of eight buttons were exercised. The rest of
the table is vendor docs, not hardware, and is labelled as such in the code.

## Undocumented: a relative encoder on the DAW port

**Evidence class: hardware, undocumented.** Not in the vendor's MIDI-mode
tables, which list no assignment for the transport buttons, the 4-D encoder or
the pedals.

Observed on the **`DAW` port** — not `Main` — on **channel 1**:

```text
DAW | CC ch1 cc 60 val 1    | B0 3C 01
DAW | CC ch1 cc 60 val 65   | B0 3C 41
```

Only two values appear, alternating in bursts of a few per second across about
a second of continuous operation. `0x01` and `0x41` are NI's relative-encoder
encoding — bit 6 as the direction flag, so `+1` and `-1` — the same scheme
`tactus_midi::decode_relative` already implements for the channel-16 map.

**What is not established:** which physical control this is. A 4-D encoder was
among the controls being operated, which makes it the leading candidate, but
several controls were in use and nothing isolates this one. It is recorded as
`DawPortEncoder` rather than named, on the same principle as the HID indices:
an unnamed observation beats a plausible guess.

Note the namespace: CC 60 on **channel 1** is distinct from the channel-16 map
in [mk3-daw-port.md](mk3-daw-port.md). The DAW port carries both.

## The handshake is confirmed, and changes nothing here

**Evidence class: hardware.** Sending `handshake::HELLO` (`BF 01 04`) to the
DAW port draws an immediate, reproducible two-message reply:

```text
DAW | CC ch16 0x05 = 0   BANK_MAPPING                 | BF 05 00
DAW | CC ch16 0x01 = 4   PROTOCOL_VERSION (handshake) | BF 01 04
```

Reproduced across two separate runs, ~1.5 s after send both times. This is the
**first hardware confirmation of any part of the DAW-port protocol beyond port
naming** — everything else there remains vendor source.

It did **not** switch on the channel-16 control map. With the handshake
established, then `ENABLE_14BIT_PARAMS` (`BF 06 01`), then the navigation
enables (`BF 30/31/32 03`) added in stages, no channel-16 control traffic
arrived in any stage. `GOODBYE` (`BF 02 00`) was sent at the end of every run.

The honest reading is that the channel-16 map belongs to a host-integration
mode the device was not in, and that reaching it needs more than these four
messages. The staged result bounds what was tried; it does not prove the map
is unreachable.

## Where this should have started

The knob and button assignments on this page come from NI's published manual.
They were a web search away, and this project instead reached them after an
HID capture session, a process-kill experiment, a USB topology enumeration and
two decompilation passes.

The reverse engineering was not wasted — the ODR transport, the absent `IOHID*`
imports and the handshake confirmation are all real, and none of them are
published. But the *order* was wrong, and it produced a false negative that
survived into a commit: "no MIDI control change on any port", recorded while
the controls were in fact sending on channel 1 the whole time. Two mistakes
compounded — testing with transport buttons, which the factory template does
not map, and watching for channel 16 when the manual says channel 1.

**Rule for the rest of this program: read the vendor's documentation first, and
treat it as a hypothesis to confirm against hardware rather than as something
to be discovered independently.** It is a vendor artifact like any other — the
existing evidence hierarchy already says hardware outranks it — but it is a
free hypothesis, and this project paid for several it could have had for
nothing.

## Sources

- [MIDI mode — Kontrol S MK3 manual](https://docs.native-instruments.com/online-guides/kontrol-s-mk3-manual/en/midi-mode)
