# ODR agent IPC — a working client

**Evidence class: hardware/host observation.** Everything here was exercised
against NI's running agent (`agent_version 2.0.7 (R15)`) with an attached
S88 MK3, by `crates/tactus-odr` and its `pilot` example. Where something was
inferred rather than observed, it says so.

This supersedes the guesswork in [mk3-odr.md](mk3-odr.md) for the IPC side.
That document's method *vocabulary* remains correct; its assumptions about
the envelope were wrong.

## Transport and framing

A Unix stream socket at
`/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks`.
**No authentication** — any local process may connect.

Each frame is a little-endian `u32` byte count followed by one msgpack value.
The values are standard msgpack-RPC:

| Form | Shape |
|---|---|
| request | `[0, msgid, method, params]` |
| response | `[1, msgid, error, result]` |
| notification | `[2, method, params]` |

`error` is `nil` on success; on failure it is a string and `result` is `nil`.

## Methods are integers

**The single fact that makes or breaks a client.** `method` is not a string —
it is an index into a symbol registry. Map keys are interned the same way, so

```json
[2, 348, ["99FE7169", {"218": 220, "237": false}]]
```

means

```text
device_owned_instance_client_state_changed("99FE7169",
    { page: browser, has_lightguide_focus: false })
```

with `348 → device_owned_instance_client_state_changed`, `218 → page`,
`220 → browser`, `237 → has_lightguide_focus`.

### Getting the registry

**The IPC greeting does not contain it.** It carries `agent_version`,
`ipc_protocol_version`, `available_devices` and `registered_assets` only.

The **device-facing** endpoint does send it. Start the agent with
`NI_ODR_AGENT_ENABLE_TCP=1`, connect to TCP `5454`, and its unsolicited
`handshake` notification carries `symbol_registry` as an ordered array —
**389 entries** on this version. Index is position.

Running the agent with `NI_ODR_DISABLE_SYMBOL_REGISTRY=1` makes it send an
empty registry and use plain strings on the device endpoint. That is useful
for reading traffic, and is why `Registry::is_empty` exists.

Since the table is version-specific, `tactus-odr` takes it as input rather
than embedding a copy.

## Two session quirks

1. **The greeting is an unsolicited response.** The agent opens with
   `[1, 0, nil, {…}]` — a reply to a request nobody made.

2. **The first request is consumed.** The first request on a connection
   establishes the session and is never answered; from the second onward
   replies arrive normally. This cost most of a session to find, because a
   fresh connection per call means *every* call looks silent.
   `Client::connect` spends one automatically.

## The registered surface is six methods

Probing all 389 indices with empty arguments and classifying the replies —
*"Method not registered"* versus *"Arguments cannot be parsed"* — gives the
exact surface of this endpoint:

| Index | Method | Arguments |
|---|---|---|
| 382 | `connect_device` | `(serial, client_name)` |
| 383 | `disconnect_device` | not determined |
| 384 | `get_device_properties` | `(serial, client_name)` |
| 385 | `get_device_settings` | `(serial, client_name)` |
| 386 | `get_devices` | `()` |
| 388 | `instance_sync` | not determined |

**The other 383 are not registered here.** In particular every `client_*`
display method — `client_page_set`, `client_browser_set_sounds`,
`client_mixer_set_trackinfo` — is absent. This socket is a **device-management
API, not the display API.**

## What the agent returns

`get_devices()`:

```json
[{"manufacturer": "Native Instruments", "product": "KONTROL S88 MK3",
  "serialnumber": "99FE7169", "vendorID": 6092, "productID": 8480,
  "versionNumber": 516}]
```

`get_device_settings(serial, client_name)`:

```json
{"led_brightness": 1, "display_brightness": 1, "lightguide_enabled": true,
 "global_led_color": "Blue", "usb_midi_2_enabled": false,
 "usage_data_tracking_enabled": true, "midi_output_port": "usb_priority",
 "velocity_curve": "linear", "aftertouch_mode": "poly_key_pressure",
 "aftertouch_configuration": {"curve": 0.67, "sensitivity": 0.5,
                              "threshold": 0, "delay": 0},
 "active_midi_template": "", "midi_templates": [],
 "speak_browser_preset": true, "experimental_features_enabled": false}
```

Two things worth noting. `aftertouch_mode: poly_key_pressure` independently
corroborates the polyphonic aftertouch seen on the MIDI port. And
`midi_templates: []` with `active_midi_template: ""` means **no MIDI template
is loaded on this unit** — a plausible reason some controls were silent in
earlier MIDI-mode captures, and a reminder that those captures measured a
particular configuration rather than the hardware's capability.

`get_device_properties(serial, client_name)` returns `{}`. Not an error —
there is simply nothing there without a device session.

## The open question: where the display API lives

`connect_device(serial, client_name)` returns **`false`**, with Komplete
Kontrol running *and* with it quit. Until that returns true there is no device
session, and the `client_*` methods are not reachable from here.

Candidates, none yet tested:

- `connect_device` needs a different second argument, or a client registered
  some other way first.
- A successful connect opens a **second** channel — a per-client socket or
  port — where the `client_*` methods are registered.
- The display methods belong to the **device-facing** protocol
  (`host_protocol_version 2.4.0`, distinct from `ipc_protocol_version 2.1.0`)
  and the agent relays them, so a host drives the screen only by being the
  thing the agent fronts for.

The third is the most likely: the two version numbers differ, and the
`client_*` names appear in the device-side handler table rather than this one.

## How to reproduce

```sh
# once: capture the registry from the device-facing endpoint
NI_ODR_AGENT_ENABLE_TCP=1 \
  "/Library/Application Support/Native Instruments/Hardware/Hardware Connection Service/\
NIHardwareConnectionService.app/Contents/MacOS/NIHardwareConnectionService" &
# connect to 127.0.0.1:5454, read the handshake, save symbol_registry as JSON

TACTUS_ODR_REGISTRY=/path/to/symbol_registry.json \
  cargo run -p tactus-odr --example pilot
```

The pilot is read-only: it enumerates, reads properties and settings, and
listens. It writes nothing.
