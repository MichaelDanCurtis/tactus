# Kontrol S-series MK3 — firmware access

**Status: no firmware image obtained.** This records where it is not, and what
the access route would be.

## What the device exposes

The MK3 presents a standard USB **DFU** interface — interface 4,
`bInterfaceClass = 254` (0xFE, application specific),
`bInterfaceSubClass = 1` (DFU), `bNumEndpoints = 0`. *(hardware observation,
see [mk3-usb-interfaces.md](mk3-usb-interfaces.md))*

Zero endpoints is normal for DFU: the protocol runs entirely over control
transfers on endpoint 0.

Version information is readable today without any of this:

| Source | Value | Class |
|---|---|---|
| `bcdDevice` on the device node | `0x0204` → revision **2.04** | hardware |
| Feature report `0xD8` | protocol version **2.4.0** | hardware |

## Where the firmware is not

Searched and empty:

- `/Library/Application Support/Native Instruments` — no `*firmware*`, `*.dfu`
  or `*.fw` files
- `/Users/Shared/Native Instruments`, including `installed_products`
- `~/Library/Application Support/Native Instruments`, `~/Library/Caches`

`NIHardwareAgent` and `NIHardwareConnectionService` mention firmware only as
display formatting — `Firmware v%d`, `FIRMWARE: v%1%` — i.e. they *report* a
version, they do not carry an image.

**Conclusion: firmware images are not shipped with the local install.** They
are fetched on demand, so obtaining one means capturing a download rather than
finding a file.

## The most likely local source

`/Applications/Native Instruments/Controller Editor` — its binary carries 47
firmware/DFU string matches, by far the most of anything installed, which makes
it the probable controller-firmware updater. Not yet examined.

## Two routes, both unexercised

1. **Capture an update.** Trigger a firmware update through NI's tooling and
   capture the downloaded image. Needs an update to actually be available for
   this unit, which it may not be.

2. **DFU upload from the device.** USB DFU defines an `UPLOAD` request
   (device → host) alongside `DNLOAD`. Whether a device permits it is declared
   by `bitCanUpload` in its **DFU functional descriptor**, which is readable by
   claiming interface 4 and reading the descriptor — a read-only operation that
   answers the question definitively before anything else is attempted.

   Most commercial devices clear `bitCanUpload` precisely to prevent firmware
   extraction, so the expected answer is no. It is still worth one read,
   because it is cheap and it is a fact rather than an assumption.

## What this project will not do

Writing firmware. `DNLOAD` puts a new image on the device, and a failed or
wrong image on a DFU-class device is how a keyboard becomes unbootable. Reading
descriptors and, if permitted, uploading an image out is interoperability
research; writing one is not part of this program.
