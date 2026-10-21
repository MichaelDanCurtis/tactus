# Kontrol S-series MK3 — USB interface topology

**Evidence class: hardware observation.** Read from `ioreg` on an attached
S88 MK3 (`idProduct` 8480 = `0x2120`, serial `99FE7169`), corroborated by
symbols in NI's own `NIHardwareConnectionService`.

This document exists because a claim in [mk3-hid.md](mk3-hid.md) — that the HID
interface carries the control surface — turned out to be **wrong**, and the
topology is what shows why.

## The five interfaces

The device supplies a string descriptor for each, so these names are the
*device's own*, not ours:

| # | `iInterface` string | Class | SubClass | Endpoints |
|---|---|---|---|---|
| 0 | `KONTROL S88 MK3` | 1 (audio) | 1 (audio control) | 0 |
| 1 | `KONTROL S88 MK3 MIDI` | 1 (audio) | 3 (MIDI streaming) | 2 |
| 2 | `KONTROL S88 MK3 HID` | 3 (HID) | 0 | 2 |
| 3 | `KONTROL S88 MK3 ODR` | 255 (vendor) | **13 (`0x0D`)** | **3** |
| 4 | `KONTROL S88 MK3 DFU` | 254 (app. specific) | 1 (DFU) | 0 |

Also observed on the device node: `bcdDevice = 516` = `0x0204`, i.e. **device
revision 2.04**, consistent with the `protocol version 2.4.0` read from feature
report `0xD8`. `USBSpeed = 3` (high speed).

## The headline: the control surface is on ODR, not HID

Three observations, in the order they were made:

1. **HID input reports never arrive.** With the vendor's software running, a
   capture tool sharing the device received zero input reports while controls
   were operated. That is explainable by macOS's exclusive-open semantics, so it
   proved nothing on its own.
2. **It is not a seize artifact.** `NIHardwareAgent`,
   `NIHostIntegrationAgent`, `NIHardwareConnectionService` and `NTKDaemon` were
   stopped — verified absent from the process table, and confirmed still absent
   after the test — and the keyboard *still* produced no HID input reports while
   buttons, knobs and the display buttons were operated. Komplete Kontrol itself
   was not running at any point during these tests.
3. **NI never uses the HID API.** `NIHardwareConnectionService` imports **zero**
   `IOHID*` symbols. It matches and opens devices through
   `IOServiceMatching` + `IOCreatePlugInInterfaceForService`, and the ODR
   interface node advertises `IOUSBLib.bundle` as its `IOCFPlugInTypes`. The
   vendor drives this keyboard over **raw USB**.

The ODR transport is explicit in their symbols:

```text
ni::odr::transport::usb::usb_device
ni::odr::transport::socket::socket_device<boost::asio::ip::tcp>
ni::odr::transport::self_healing_stream_parser
ni::odr::agent::kks::device_factory          (built from either transport)
ni::odr::agent::kks::device_manager::hotplug_device_added(
    device_serial_tag, abstract_device_communicator)
ni::odr::agent::kks::abstract_device_communicator
ni::msgpack_support::symbol_registry_const
```

So the device's interactive surface — input *and* display — runs over interface
3 as msgpack, and the same protocol runs over TCP for the simulator noted in
[mk3-display.md](mk3-display.md). One transport, two carriers.

### What this does and does not say about HID

It does **not** say the HID interface is fake. It is a real HID interface with
two endpoints and a descriptor the firmware honours, and **output works**:
writing light-guide report `0x82` is accepted by the hardware and lights the
keybed. Feature reads `0xD0`/`0xD8`/`0xD9` work too.

What is unsupported is the *inbound* direction. The declared input reports
(`0x01`, `0x02`, `0x03`, `0x40`, `0xAA`) have never been observed to arrive.
Two explanations remain open and this evidence does not separate them:

- The firmware only emits them after an enable the vendor never sends, because
  the vendor does not use HID input.
- They are vestigial — declared for a mode, a sibling device, or a compatibility
  path that this firmware does not exercise.

Deciding between those means either finding an enable command in firmware, or
exercising the undocumented output reports (`0xA0`–`0xA4`, `0xAF`, `0xF4`) to
see whether one turns input on. **The second is not worth doing by guesswork**:
six report ids with unknown payloads, written to a configuration surface on a
working instrument, to chase a path the vendor itself does not use.

## Correction to an earlier claim

[mk3-display.md](mk3-display.md) records the vendor's display-interface match as
class `0xFF`, subclass **`0xBD`**, recovered from a vendor binary. This hardware's
vendor interface is class `0xFF`, subclass **`0x0D`**.

Those are different values, so the `0xBD` rule does **not** select this device's
ODR interface. The most likely reading is that `0xBD` belongs to the MK2-era
bulk-display path — consistent with the already-established finding that the MK3
is not on that path at all — but this has not been confirmed against an MK2, so
treat `0xBD` as "not applicable to MK3" rather than "wrong everywhere".

This is the fourth time a vendor artifact has disagreed with this hardware. The
standing rule holds: **confirm against the live device.**

## Consequences for the SDK

- `tactus-io`'s HID input path (`poll_input`, `poll_raw`) is correct code with
  nothing to read on this generation. It stays, because it is how MK2 and
  Maschine devices report, and because an input report arriving would be real
  evidence worth capturing.
- **Semantic control naming is blocked on an ODR implementation**, not on a
  capture session. Earlier notes called it "entirely mechanical, needs only
  hardware and a person"; that was wrong, and it was wrong because it assumed
  the transport without testing it.
- Keys, velocity and **polyphonic aftertouch** do arrive over MIDI on `Main`,
  verified in the same session — see [mk3-daw-port.md](mk3-daw-port.md).
- ~~Nothing about the control surface arrives over MIDI.~~ **False — retracted.**
  The knobs and buttons below the display send control change on **channel 1**
  per the factory MIDI template, and a relative encoder reports on the `DAW`
  port. The original test pressed *transport* buttons, which the template does
  not map, and watched for channel 16. See
  [mk3-midi-mode.md](mk3-midi-mode.md).

## Negative-search bounds

Stated so the negatives above are read as bounded searches rather than proof:

> ### The negatives above were recorded in one device mode only
>
> **This was an uncontrolled variable and it should have been stated from the
> start.** The keyboard was in **MIDI mode** throughout every test on this
> page. The S-series MK3 is a mode-switching device, and "this interface
> carries no input" may be a property of the mode rather than of the
> interface.
>
> What this does **not** weaken: the three positive findings. NI imports zero
> `IOHID*` symbols, the device names interface 3 `ODR`, and the vendor's
> symbols carry a matching USB transport. Those are mode-independent facts
> about code and descriptors.
>
> What it **does** weaken: "the MK3 emits no HID input reports" and "no
> control change arrives on any port". Both should be read as **"not in MIDI
> mode"** until repeated in host/NKS mode. The conclusion that the control
> surface lives on ODR is still the best explanation of the positives, but the
> HID silence is no longer independent evidence for it.

| Checked | Not checked |
|---|---|
| `ioreg` interface and endpoint-count enumeration on one S88 MK3 | An S49 or S61 MK3; a second S88 |
| HID input with vendor agents running, and with all four stopped — **in MIDI mode only** | **Any other device mode**; HID input after sending any of the undocumented output reports |
| MIDI in on all three ports, agents running and stopped — **in MIDI mode only** | **Any other device mode** |
| DAW-port handshake `BF 01 04` sent and answered (see below) | Whether the surface reports after a handshake in a non-MIDI mode |
| `IOHID*` and `IOUSB*` imports of `NIHardwareConnectionService` | `NIHardwareAgent`, `NIHostIntegrationAgent`, `NTKDaemon`, Komplete Kontrol itself |
| Device revision and speed from the device node | The ODR endpoint addresses, types and packet sizes — `ioreg` did not expose pipe children for an unopened interface |
| Whether a MIDI template was assigned to the device | — the vendor ships `ni::odr::agent::midi_template_transfer_mediator`, so templates are pushed over ODR, and an unassigned template is a plausible reason for silent controls in MIDI mode |
