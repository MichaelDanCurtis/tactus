# C examples

Two programs, both plain C against the generated `tactus.h`. They exist to keep
the ABI honest: every bug found in the C boundary so far was found by compiling
a real C program against it, not by reading the Rust.

| File | Needs hardware | What it covers |
|---|---|---|
| `smoke.c` | no | The pure protocol surface — model identity, light-guide encoding, DAW SysEx framing, display frame encoding, and every documented refusal. |
| `device.c` | a keyboard, to do anything interesting | Enumeration, opening, identity, feature reports, input polling, light-guide writes, MIDI port resolution. Exits 0 with nothing attached. |

## Building

```sh
cargo build -p tactus-ffi

cc -Wall -Wextra -I crates/tactus-ffi/include -o /tmp/tactus_device \
   examples/c/device.c target/debug/libtactus.a \
   -framework CoreFoundation -framework IOKit -framework CoreMIDI \
   -framework CoreAudio -framework AppKit -lobjc -liconv

/tmp/tactus_device
```

`libtactus.a` is one static library carrying the whole SDK, so linking it pulls
in the platform I/O layer whether or not your program calls that half. That is
why `smoke.c` needs the same frameworks as `device.c` despite touching no
hardware.

Do not copy the flag list above onto another platform or a later version of this
crate. Ask the toolchain instead — it is generated from the actual dependency
graph and cannot go stale:

```sh
cargo rustc -p tactus-ffi --crate-type staticlib -- --print native-static-libs
```

## What `device.c` will not do

It opens MIDI ports and then stops, rather than sending anything.

Opening a port is harmless. Transmitting is not: the DAW-port command set is
recovered from the vendor's own MIDI Remote script — a good source, but one that
has never been confirmed against hardware — and the keyboard may well be in the
middle of a session driven by somebody else's software. An example program is
the wrong place to find out what an unverified command does to a running rig.

The one write it does perform is the light guide (output report `0x82`): the
report is device-declared, its `0..=127` range is device-declared, and every
value is validated before transmission.

It also never writes a feature report. `tactus_device_write_feature` exists only
to refuse, with a reason — `0xD8` and `0xD9` are read-only, and `0xD0` is
settable with not one of its 32 bytes understood.
