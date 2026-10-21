# Kontrol S-series MK3 — HID report map

**Evidence class: hardware observation.** Parsed from the device's own HID
report descriptor, read live off an S88 MK3 — 1001 bytes, 476 HID items.

Raw evidence in this directory:
- [`mk3-hid-descriptor.hex`](mk3-hid-descriptor.hex) — the descriptor verbatim
- [`mk3-hid-report-map.txt`](mk3-hid-report-map.txt) — full parse output
- [`../../tools/hidparse.py`](../../tools/hidparse.py) — the parser, re-runnable

## Why this supersedes the vendor's table

Native Instruments ships a per-device report-map table in
`NIHardwareConnectionService`. It is **wrong about this device in two places**:

- it declares a feature report `F-F8` that **this firmware does not implement**
- it **omits input report `0x40`**, which the device does declare

It agrees on the other eighteen report ids, so it is a useful hint — but the
live descriptor is the authority. Any host should read it rather than trusting a
built-in table, including this SDK's own.

## Parse validation

Three independent cross-checks, all passing — which is why the layouts below
can be relied on:

| Parsed | Independently observed |
|---|---|
| `0xAA` = 62 byte payload + 1 id = 63 | `MaxInputReportSize` = 63 |
| `0xA1` = 203 byte payload + 1 id = 204 | `MaxOutputReportSize` = 204 |
| `0x82` = 88 bytes | 88 keys on an S88 |

## Structure is known; semantics are not

The descriptor says **how many** controls of **what width** each report
carries. It does **not** say which physical control each index is: the vendor
uses a single opaque vendor-defined usage page (`0xFF01`) with generic usage
ids. There is no "this is the Play button" in the data.

So this document and the `tactus-hid` crate describe fields **by index with
their declared range**, and invent no names. Mapping index → control requires
pressing each control and observing which field changes. Guessing would be
worse than leaving it unnamed, because callers would build on the guess.

**Keys and velocity do not appear here.** Note data — and polyphonic
aftertouch — arrives over MIDI on the `Main` port.

> ### Correction (hardware observation, supersedes the rest of this section)
>
> This document previously ended the paragraph above with "HID carries the
> control surface." **That is false for this generation.** No HID input report
> has ever been observed to arrive from an S88 MK3, with the vendor's agents
> running *or* with all four of them stopped. The control surface runs over the
> device's vendor interface, which the device itself names **ODR** — see
> [mk3-usb-interfaces.md](mk3-usb-interfaces.md).
>
> The input-report *structure* documented below is still accurate: it is read
> from the live descriptor and the firmware declares every one of these
> reports. What is wrong is the assumption that declaring them means sending
> them. **Output is unaffected** — light-guide report `0x82` is accepted by the
> hardware — and so are feature reads.
>
> The error is worth naming precisely, because the rule it broke is this
> document's own: a declared capability is not an observed one. The descriptor
> is a vendor artifact like any other.

## Input reports

| Id | Payload | Declared fields |
|---|---|---|
| `0x01` | 6 B | 40 × 1 bit (0..1); 2 × 4 bit (0..15) |
| `0x02` | 27 B | 8 × 1 bit; **8 × 16 bit (0..999)**; 4 × 16 bit (0..4095); 16 × 1 bit |
| `0x03` | 12 B | 8 × 1 bit; 3 × 16 bit (0..65535); 2 × 16 bit (0..2047); 1 × 8 bit |
| `0x40` | 13 B | 13 × 8 bit — purpose unknown; **absent from the vendor's table** |
| `0xAA` | 62 B | 31 × 16 bit (0..65535) — purpose unknown; largest input report |

Reasonable inferences, flagged as such:

- The **eight 0..999 fields in `0x02`** are almost certainly the eight
  continuous encoders — the count matches the hardware and 1000 steps is a
  plausible resolution. **Inferred, not confirmed.**
- The **two 4-bit fields in `0x01`** are plausibly relative encoders, from
  their width and range. **Inferred.**
- The **0..2047 and 0..4095 fields** are plausibly touch strips, wheels or
  pedals. **Unassigned.**

`0x40` and `0xAA` are genuinely uncharacterised. The SDK surfaces them as
`InputReport::Unmodelled` rather than dropping them, so device behaviour is
visible to callers instead of silently discarded.

### Bit packing

HID packs bit fields **least-significant-bit first within each byte**. The
first declared control is bit 0 of byte 0; the ninth is bit 0 of byte 1. This
is the single most likely thing to get wrong, and `tactus-hid` has a dedicated
test pinning it.

## Output reports

| Id | Payload | Declared fields |
|---|---|---|
| `0x80` | 42 B | 42 × 8 bit (0..127) — LED brightness |
| `0x81` | 30 B | 30 × 8 bit (0..127) — LED brightness |
| **`0x82`** | **88 B** | **88 × 8 bit (0..127) — per-key light guide** |
| `0xA0` | 14 B | 14 × 8 bit (0..255) |
| `0xA1` | 203 B | 96 + 96 × 8 bit; 8 × 8 bit (0..31); 1 × 16 bit |
| `0xA2` | 44 B | 12 + 12 + 20 × 8 bit |
| `0xA3` | 144 B | 12 × (12 × 8 bit) |
| `0xA4` | 128 B | 128 × 8 bit |
| `0xAF` | 2 B | — |
| `0xF3` | 1 B | 1 × 8 bit (0..127) — brightness |
| `0xF4` | 32 B | 1 × 8 bit; 31 × 8 bit |

### The light guide is 7-bit, not 8

Report `0x82` carries one byte per key, sized exactly to the keybed
(`0x31`/`0x3D`/`0x58` = 49/61/88). Its declared logical range is **0..=127**,
not 0..=255.

That matters: it is consistent with a **palette index**, not an 8-bit
intensity or a packed RGB value. `tactus-hid` validates against 127 and
**rejects** larger values rather than masking them, because masking would send
a wrong colour while hiding the caller's bug.

This also refines the earlier correction to third-party documentation: the
light guide is neither 128 note-indexed entries nor free RGB — it is one 7-bit
value per physical key.

## Feature reports

Only three exist. `0xF8` — which the vendor's own table claims, and which its
pixel-display stack uses to fetch panel geometry — is **not implemented by this
firmware**.

| Id | Payload | Flags | Declared fields |
|---|---|---|---|
| `0xD0` | 32 B | `[Data]` — settable | **2 × 16 bit (0..65535)**; 1 × 8 bit; 1 × 8 bit (**0..100**); 1 × 8 bit (0..127); 25 × 8 bit |
| `0xD8` | 32 B | `[Const]` — read-only | 2 + 2 + 4 × 16 bit; 1 + 15 × 8 bit — identity/version |
| `0xD9` | 32 B | read-only | ASCII serial number |

Observed values (read with Komplete Kontrol live, so not an uninitialised
device):

```
D0  all zeros
D8  D8 05 01 00 13 69 71 FE 99 5D 6A CF 65 00 00 00 00 FF×12 73 74 33 32
D9  ASCII serial
```

### A structural note on `0xD0`, explicitly not a conclusion

`0xD0` opens with **two 16-bit fields** — the shape of a width/height pair —
followed by a byte, a **0..100 field** (the shape of a percentage, plausibly
brightness), and a 0..127 field sharing the usage id used for LED brightness
elsewhere.

It is `[Data]`, i.e. settable, and it reads all zeros even with the vendor's
software running. That is consistent with "never written on this device".

**This is not evidence of panel geometry.** The MK3 renders its own UI and the
host never composes a screen — see [`mk3-display.md`](mk3-display.md) — so
there is no reason for the host to know a resolution. The most likely reading
is a shared descriptor template carrying a field this model does not use. It is
recorded because it is a real structural observation, and because somebody will
otherwise rediscover it and misread it as the geometry we spent a long time
looking for.

## What would complete this

1. **Semantic mapping.** ~~Press each button, move each encoder, and record
   which field index changes. Entirely mechanical, needs only hardware and a
   person.~~ **Retracted.** This assumed the control surface reports over HID.
   It does not; there is nothing to capture here. Mapping index to control is
   blocked on an ODR implementation, and may turn out to be meaningless for
   MK3 if these reports are vestigial. See
   [mk3-usb-interfaces.md](mk3-usb-interfaces.md).
2. **Characterise `0x40` and `0xAA`.** 13 bytes and 31 × 16 bit respectively,
   both currently unexplained.
3. **Identify the output reports beyond the light guide.** `0xA1`–`0xA4` are
   large and unexplained; `0xA3`'s 12 × 12-byte structure suggests twelve
   somethings.
