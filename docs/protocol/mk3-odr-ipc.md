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

## The connection opens with a raw 16-byte UUID

**Evidence class: host observation.** Before any framing, a client writes
**16 raw bytes**: its connection UUID. Only then do length-prefixed msgpack
frames begin.

This was found by accident and is worth recording as a method. Sending frames
without the prelude made the agent log

```text
rpc_server::remove_connection: 38000000-9302-ae69-6e73-74616e63655f
```

Those are *our own bytes* rendered as a UUID — `38 00 00 00` is the 56-byte
length prefix, `93 02` the msgpack `[2, …`, `ae` a 14-character string header,
then `instance_` . The agent had consumed our first 16 bytes as the connection
id. Sending a real UUID first makes it report that UUID back correctly.

**This also explains the "first request is consumed" behaviour** recorded
earlier in this document: nothing was consuming a request. The first 16 bytes
of the first frame were being taken as the connection id, corrupting that one
frame and leaving the rest of the stream aligned. One cause, two symptoms.

## The method surface depends on the connection's role

The six methods listed above are what an **unidentified** connection gets.
Once `instance_hello` succeeds the agent registers the client surface, which
its own log enumerates:

```text
add_asset
client_browser_set_data_model      client_browser_set_filter
client_browser_set_filter_selection client_browser_set_filters
client_browser_set_settings        client_browser_set_sound_item
client_browser_set_sound_selection client_browser_set_sounds
client_lightguide_set_leds         client_metro_flash
client_mixer_set_meters            client_mixer_set_track_data
client_mixer_set_trackinfo
client_parameter_page_set_data     client_parameter_page_set_plugin_data
client_parameter_page_set_parameter_value
client_parameter_page_set_device_owned_viewstate
client_parameter_page_set_host_owned_viewstate
client_plugin_chain_set_data       client_plugin_chain_set_index
client_request_focus               client_release_focus
client_set_host_owned_instance_client_state
client_set_page
client_smartplay_set_data          client_smartplay_set_device_owned_viewstate
instance_goodbye                   run_self_test
send_client_notification           send_global_notification
set_device_settings
```

**That is the display API, and it is reachable from this socket** — the
earlier conclusion that it was not reachable here was wrong, and is retracted.
`client_set_page` plus one `client_*_set_*` call is the path to putting
content on screen; `send_global_notification` is the smallest possible
"hello world".

## The agent's log is the feedback channel

`~/Library/Logs/Native Instruments/NIHardwareConnectionService.log`, at trace
level, is what makes this protocol tractable at all. It reports:

| Log line | Meaning |
|---|---|
| `rpc_server::start_async_accept: connection established` | connection accepted |
| `rpc_server::remove_connection: <uuid>` | the connection id it parsed from the prelude |
| `IPC: instance_hello - <uuid>, client version 2.2.0, server version 2.1.0` | handshake accepted |
| `msgpack_rpc::handle_notify - method not registered (X)` | X is not valid for this connection's role |
| `msgpack_rpc::handle_notify - available methods …` | the full valid set, free of charge |
| `match_dictionary_elements: unexpected key found: "K"` | K is not a known key of that argument |
| `Arguments cannot be parsed: […]` | arity or types wrong, echoed back verbatim |

The distinction between the last two is what makes argument discovery
possible: *unexpected key* means the dictionary was reached, so arity is
right; *cannot be parsed* without any key warning means it failed earlier.

## `instance_hello` — solved

**Evidence class: host observation**, confirmed by decompiling the handler.

```text
request  [0, msgid, "instance_hello", [ <instance-uuid-string>, {
             "client_info": { "name": "<client>", "version": "2.2.0" },
             "ipc_protocol_version": "2.2.0"
         } ]]

reply    [1, msgid, nil, { "agent_version": "...", "ipc_protocol_version": "2.1.0",
                           "available_devices": [...], "registered_assets": [...] }]
```

The reply payload is the same thing this document earlier called the
"unsolicited greeting". It was never unsolicited — it is the handshake's
result, and it only looked spontaneous because the 16-byte prelude problem
was eating the request that caused it.

### How the keys were found, after guessing failed

Name-guessing cost three rounds across **45 candidates** and found only
`ipc_protocol_version` — which turned out to be a red herring: it is a key of
the *reply*, and `match_dictionary_elements` recognises it globally, so "not
flagged" never meant "belongs to this call".

Decompiling settled it in three steps:

1. The success-path log string `"IPC: instance_hello - {}, client version {},
   server version {}"` is at `0x10060a292`.
2. Its only xref, `0x100233888`, lies inside
   `ipc_server::register_rpc_instance_handling()::$_30`'s dispatch handler at
   **`0x10023336c`**.
3. Of that function's 259 distinct reference targets, exactly two resolve to
   key-shaped strings — and they are adjacent in the string table:

   ```text
   0x100582780  client_info
   0x10058278c  ipc_protocol_version
   ```

`client_info` was never among the 45 guesses. The lesson is the one already
written into `mk3-midi-mode.md`: read the source of truth first. Static
analysis gave a definite answer in three queries after name-guessing had
burned three rounds on a wrong lead.

## CONFIRMED: a known-good sequence drives the light guide

**Evidence class: hardware observation.** The public LED test script from the
prior-art thread ([skylerwshaw's gist](https://gist.github.com/skylerwshaw/be4b39e12b65286aafed25e2a595e547),
reviewed before running: Unix socket only, no network, no subprocesses) was
run by the device's owner against agent `2.0.7 (R15)`. **The light guide lit
green.** Its self-test also passed, including the legacy-id fallback this
agent needs.

The sequence, on one connection:

| # | Kind | Method | Arguments |
|---|---|---|---|
| 0 | raw | — | 16-byte UUID prelude |
| 1 | request | `instance_hello` (string name) | `[uuid, {client_info: {name, version, type: "standalone"}, ipc_protocol_version: "2.1.0"}]` |
| 2 | request | `connect_device` (382) | `[uuid, serial]` — reply checked |
| 3 | notify | `client_request_focus` (373) | `[uuid, serial]` |
| 4 | notify | `client_lightguide_set_leds` (360) | `[uuid, serial, {239: [[midi_note, colour], …]}]` |

Key `239` is `client_midi_addressing`. The array is indexed by **MIDI note**,
128 entries, not by physical key. Colour bytes: green `0x1C`, red `0x04`.

Compared with this project's failing attempts, four differences:

1. **`connect_device` after the handshake** — never sent on an identified
   connection here. This is the step that attaches a device to the instance,
   and its absence is the most likely cause of every `no such device`.
2. **`(uuid, serial)` order**, as already corrected above.
3. **`ipc_protocol_version: "2.1.0"`**, matching the agent, where this project
   sent `"2.2.0"`.
4. **Integer method ids and integer payload keys** from the registry, rather
   than strings.

**Komplete Kontrol was closed** — confirmed by the owner — and the result
**reproduced on a second run**. Only NI's background agent was in the path.
This is the result the program rests on: the hardware responds to a client
that is not the vendor's application.

The agent logged nothing for the run. That is consistent with success, since
successful calls log only at trace level and this agent was at default level.
It is not evidence by itself.

## Correction: argument order is (instance_uuid, serial, payload)

Earlier sections of this document state display calls take
`(serial, instance_uuid, payload)`. **That is backwards.**

Two independent lines of evidence:

- **Prior art.** A community thread documenting this same agent shows the
  display call as `[uuid, serial, {...}]`, instance first. Its method ids also
  match this project's registry exactly — 382 attach (`connect_device`), 373
  focus (`client_request_focus`), 360 set LEDs (`client_lightguide_set_leds`) —
  which independently corroborates the registry.
- **This project's own log, re-aligned.** Anchoring on the one parse-error line
  in the schema-probe run, the push that rendered on screen was the
  `(instance, serial)` call, and it is the call with no `no such device` line.
  The earlier attribution was off by one call because agent log lines lag the
  sends.

`no such device <our-instance-uuid>` is consistent with this: with the order
reversed, the agent reads our instance uuid as the device.

**Consequence:** every negative result in this document obtained with
`(serial, instance)` order — including the Komplete Kontrol A/B — tested the
wrong call and settles nothing.

## What `instance_hello` actually parses (decompiled)

The handler calls

```text
match_dictionary_elements<dictionary_unpack_error_strategy(2),
    required_matcher<ni::odr::instance_info>,
    required_matcher<semver::version>>
```

So the options dictionary has exactly two required fields: `client_info`,
parsed as an `instance_info` struct, and `ipc_protocol_version`, parsed as a
semver. The error strategy warns on unexpected keys rather than failing, which
is why `is_host` is reported as unexpected yet tolerated.

`type` is a field of `instance_info`. A missing `type` is a **warning only** —
the push that rendered went out without it. Supplying it as a plain string
(`"plugin"`, `"host"`, `"standalone"`, `"instance"`) makes the whole handshake
fail to parse, so its encoding is something else; not yet determined, and not
blocking.

## Prior art

- [tillt/KompleteSynthesia discussion #29](https://github.com/tillt/KompleteSynthesia/discussions/29)
  — community notes on this agent's protocol: the 16-byte UUID prelude,
  little-endian length framing, the call order `instance_hello` →
  `connect_device` → `client_request_focus` → content, and that **focus lapses
  after a short idle period** and needs re-asserting — a better explanation for
  pushes stopping after ~17 s than the heartbeat theory, which tested negative.
  Also documents images on the display via `add_asset` (32-byte SHA-256 handle
  plus PNG/WebP bytes) and a parameter-page background, and that a malformed
  handle aborts the agent — matching the crash seen here.
- [terminar/rebellion](https://github.com/terminar/rebellion) — earlier work on
  NI's IPC for Maschine MK3 and Komplete Kontrol MK2.

Lesson repeated from `mk3-midi-mode.md`: this thread existed before most of
this document was written.

## CONFIRMED: host content renders on the MK3 display

**Evidence class: hardware observation, visually confirmed by the device's
owner.** The keyboard's screen showed `TACTUS` / `hello from tactus`.

The call that did it:

```text
[2, "send_global_notification", [
     "99FE7169",                       # device serial
     "<our-instance-uuid>",            # the uuid sent as the connection prelude
     { "notification": {
         "level":          "info",
         "title":          "TACTUS",
         "description":    "hello from tactus",
         "url_message":    "",
         "url":            "",
         "blocking_state": "non_blocking" } } ]]
```

preceded on the same connection by the 16-byte UUID prelude,
`instance_hello`, and `client_request_focus(serial, instance_uuid)`.

**Whether Komplete Kontrol was running at the moment of the write is
UNKNOWN.** The owner had closed it and *subsequently* noticed the text on
screen, which does not establish the state at write time — a notification
drawn while KK was still up could simply have persisted. Deciding this needs a
run with KK confirmed closed beforehand, and it matters: "the display is
drivable without the vendor's application" is the claim this whole program
rests on, and it is **not yet evidenced**.

### A retraction of the retraction

An earlier revision of this document said "nothing has been displayed on the
device". That was wrong, and the way it was wrong is worth keeping.

Two measurement faults were real and are still worth fixing — a reply check
that ignored the msgid, and a log window too short to catch the agent's
warnings. But having found bad instrumentation, the conclusion drawn from it
was "it failed", when the only supportable conclusion was **"this cannot
currently be measured"**. The pushes had in fact landed.

Distrusting a measurement is not the same as establishing a negative. The
first is caution; the second needs its own evidence.

The `[trace]`-level detail compounded it: a *successful* `instance_hello` logs
at trace, and the agent had been restarted without `SPDLOG_LEVEL=trace`, so
successes were invisible while failures still printed at warning. An absence
of log lines was read as an absence of events.

### Still unexplained

With strict msgid matching, `instance_hello` draws a genuine reply — but
`instance_goodbye` then reports `client not found`, and in a 30-push run the
first ~14 landed and the rest failed with `no such device <our-instance>`.
Fourteen pushes at ~1.2 s is ~17 s, which suggests the session lapses. The
agent's own handshake advertises `disable_heartbeat_validation: false`, so
heartbeats are validated and an idle client is plausibly dropped. Under test.

## Superseded: `instance_hello` is **not** solved

The section above claimed it was. That was wrong, and two measurement errors
of mine produced it. Both are worth recording, because they are the kind that
manufacture a positive result.

**Error 1 — matching a reply without checking its id.** The acceptance test was

```python
any(o[0] == 1 and not o[2] for o in replies)   # WRONG
```

The agent sends an unsolicited greeting shaped `[1, 0, nil, {...}]` on
connect. That satisfies the predicate. So "handshake OK" was reading the
*greeting*, not a response to the request — for any payload whatsoever. The
check must compare the msgid against the one sent.

**Error 2 — reading the log before the agent wrote to it.** Errors were
sampled ~0.4 s after each send, and the agent's warnings often land later. A
run reported "14 clean, 16 errors" where all 30 almost certainly failed; an
earlier single probe reported "CLEAN" on the same basis. **Any "clean" result
in this project obtained by a short log window should be distrusted.**

**Consequently retracted:** that `is_host: false` makes registration succeed.
That conclusion came from an oracle — absence of `client not found` in a short
window — which also reported success for a payload whose handshake had
explicitly failed to parse. A self-contradicting oracle proves nothing.

### What the log actually shows

Across the whole session, `IPC: instance_hello` appears **three** times:

```text
18:45:05 [trace] IPC: instance_hello - 01375f44-…, client version 2.2.0, server version 2.1.0
18:55:40 [error] IPC: instance_hello received, but instance already present (0f8f323c-…)
18:55:43 [error] IPC: instance_hello received, but instance already present (0f8f323c-…)
```

The first is the vendor's own client. The other two are ours, reaching the
handler and being refused because instance `0f8f323c` was already registered —
which means **one** earlier attempt did register, and the agent kept that
registration after its socket closed.

Every later attempt produced **no handler line at all**, while still drawing a
reply. So the reply is not coming from this handler, and the registration is
not happening. Downstream, that is exactly what the failures say:

```text
send_global_notification failed: no such device <our-instance-uuid>
instance_manager::get_instance_client cannot find identity 99FE7169
instance_manager::handle_instance_goodbye: client not found <our-instance-uuid>
```

`no such device` naming *our instance uuid* is the giveaway: the agent is
resolving that argument through `instance_manager` and finding nothing,
because we were never entered into it.

### Next step, and what not to repeat

1. Match replies by msgid, strictly, and re-establish which payloads the
   handler actually answers. Every "accepted" claim in this project predates
   that fix and needs re-testing.
2. Read the log after a settled delay, or tail it continuously, rather than
   sampling.
3. Then find why the handler is bypassed: decompile the dispatch path that
   answers a request without logging, around
   `register_rpc_instance_handling()::$_30` at `0x10023336c`.

## What *is* solid

Independently repeatable, and not dependent on either faulty oracle:

- The **16-byte UUID prelude**. Confirmed by the agent echoing the exact UUID
  back in `remove_connection`, and by frames being parsed at all once sent.
- **Framing and RPC forms** — length-prefixed msgpack, `[0,id,m,p]` /
  `[1,id,err,res]` / `[2,m,p]`.
- **`get_devices` and `get_device_settings`**, which return real values from
  the hardware, repeatably, with the reply id matching the request.
- The **symbol registry**, 389 entries, as sent by the agent itself.
- **Method names and arities**, from the `args_to_tuple size mismatch` oracle,
  which states the expected count outright rather than being inferred.
- The **notification payload schema**, which the agent enumerated by name:

  ```text
  notification: { level, title, description, url_message, url, blocking_state }
  ```

  Obtained from `expected key missing: …` warnings, one per field — an oracle
  that names what it wants instead of requiring a guess.

**Nothing has been displayed on the device.** No run has been visually
confirmed, and the agent's own logs show the writes failing.

## Call signatures

**Evidence class: host observation.** Arity came from a third log oracle,
`dispatcher::args_to_tuple size mismatch: expected N arguments`, which names
the expected count outright.

| Method | Arity | Signature |
|---|---|---|
| `instance_hello` | 2 | `(instance_uuid, {client_info, ipc_protocol_version})` |
| `client_set_page` | 3 | `(serial, instance_uuid, page_symbol_index)` |
| `send_global_notification` | 3 | `(serial, instance_uuid, {text})` |
| `send_client_notification` | 3 | `(serial, instance_uuid, {text})` — assumed by symmetry |
| `client_request_focus` | 2 | `(serial, instance_uuid)` |
| `client_release_focus` | 2 | `(serial, instance_uuid)` |
| `client_metro_flash` | 2 | `(serial, instance_uuid)` |
| `client_mixer_set_trackinfo` | 3 | `(serial, instance_uuid, payload)` — payload shape unknown |
| `client_mixer_set_meters` | 3 | as above |
| `client_lightguide_set_leds` | 3 | as above |
| `client_browser_set_sounds` | 3 | as above |
| `client_parameter_page_set_data` | 3 | as above |

The pattern is consistent: **every display call carries the device serial and
the caller's instance uuid**, then its payload.

`client_set_page`, `send_global_notification` and `client_request_focus` have
been sent and **accepted without error**. Accepted means the arguments parsed
— not that the screen changed. That distinction is kept deliberately: nothing
here has been confirmed visually.

### A crash worth knowing about

Sending `add_asset` with a two-element array ending in a map **aborts the
agent** (`SIGABRT`, exit 134). Reproduced once, not investigated further. It
is a robustness bug in the vendor's agent, and a reason to avoid fuzzing this
interface on a machine doing real work.

## Superseded: the earlier guess at where the display API lives

Established by probing against the log:

- **Form:** a **request**, not a notification. As a notification it is "not
  registered".
- **Method name is a string**, `"instance_hello"` — the IPC side has no
  registry exchange, unlike the device side.
- **Arity 2:** `(uuid, options_map)`. Both a 36-character string and 16 raw
  bytes get far enough for the map to be matched, so the map is argument two.
- **`ipc_protocol_version` is a real key.** Of 45 candidate key names probed,
  it is the *only* one the agent did not flag as unexpected.
- Unexpected keys do **not** block success: the real client sends `is_host`,
  which the agent flags, and its handshake still succeeds.

**Not yet solved:** at least one more required key, whose name was not among
the 45 tried, and whose absence yields `Arguments cannot be parsed` with no
key warning. Six value encodings for `ipc_protocol_version` were also tried —
`"2.2.0"`, `"2.1.0"`, `[2,2,0]`, `[2,1,0]`, `{major,minor,patch}`, `2` — and
all failed, so the gap is a missing key rather than a wrong value.

**Stop guessing here.** Name-guessing cost three rounds for one key. The
reliable route is to read the handler: find the `match_dictionary_elements`
call site for `instance_hello` and the key list it is given. That is static
analysis with a definite answer, against an unbounded search over names.

## Superseded: where the display API lives

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

## Input: what the keyboard sends to the focused instance

**Evidence class: hardware observation.** Captured with `examples/buttons.rs`
holding focus on an S88 MK3, agent `2.0.7 (R15)`, Komplete Kontrol closed.

Control actions arrive through **one** method:

```text
plugin_action(serial, { <action_symbol>: <value> }, <bool>)
```

The action is the map's key, by registry id. Observed:

| Control | Action | Value |
|---|---|---|
| Browser page encoders | `browser_navigate_sound`, `browser_navigate_bank`, `browser_navigate_brand` | `±1` step |
| Load next / previous sound | `browser_load_next_sound`, `browser_load_previous_sound` | `-1` in both cases; meaning not yet established |
| Browser filter buttons | `browser_toggle_filter_user_presets`, `browser_toggle_filter_favorite_presets` | `nil` |

The third argument was `true` in every case; meaning not established.

State changes arrive as their own notifications:

| Event | Payload |
|---|---|
| Page change (the **Browser** button) | `device_owned_instance_client_state_changed(serial, {page: <symbol>, has_lightguide_focus: bool})` |
| A built-in mode taking the surface | `focus_changed(serial, "play_assist")`, `focus_changed(serial, "standalone_midi")`; focus came back to the instance on exit |
| Scale root change | `device_properties(serial, {root_key: <note number>})` |
| Becoming the active instance | `active_instance_client_changed(serial, <instance uuid>)` |

**Values are not always symbols.** `page: 220` is the symbol `browser`, but
encoder steps and `root_key` are plain integers. An early version of this
crate's decoder converted every integer through the registry and reported an
encoder step of `1` as `pedal_expression_connected` — symbol #1. The decoder
now resolves keys only.

**Not yet observed:** transport buttons and knob turns. The knob hypothesis is
that `plugin_parameter_value_change` only reports for parameters that exist,
and the parameter page currently shows "no instrument loaded"; publishing a
parameter page first should make knob turns report.

## Display: the parameter-page schema

**Evidence class: host observation**, recovered from the agent's own
`expected key missing` / `unexpected key found` warnings and fatal parse
errors, one field at a time (`examples/page_probe.rs`).

```text
client_parameter_page_set_data(instance_uuid, serial, {
  plugin_data: {
    name:              string
    background:        nil accepted          (an image handle; see add_asset)
    parameters:        map                   (entry format: not yet known)
    plugin_color:      {r, g, b}             (nil also accepted)
    nks_control_color: colour                (by analogy with plugin_color; untested)
  },
  device_owned_viewstate: { view_address, force_host_viewstate },
  host_owned_viewstate:   { nks1_layout, nks2_layout },
})
```

How each was established:

| Field | Evidence |
|---|---|
| `parameters` is a map | `{}` accepted; `[]` and `[{}]` fatal |
| colour is `{r,g,b}` | `{r,g,b}` accepted with no unexpected keys; `0`, `0.5`, `"blue"`, `[0,0,255]` all fatal |
| viewstates sit **beside** `plugin_data` | inside it they are reported both missing and unexpected; beside it they are accepted, and the agent names their own fields |

**`nks1_layout` / `nks2_layout`** are part of the host-owned viewstate. The NKS
knob-page layout is therefore a first-class field of the parameter page, and
showing an NKS plugin on the keyboard comes down to populating these.

**Still open:** the parameter entry. `{0: {}}` and `{"p0": {}}` are both fatal,
so either the key type or the entry's own shape is wrong, and an empty entry
gives no warnings to learn from.

The same nesting rule likely explains `blocking_state` in notifications,
which is reported missing and unexpected in the same call.

## The host-model vocabulary (decompiled)

**Evidence class: vendor binary.** Two sources, combined:

1. **Field types**, from the template arguments of every
   `match_dictionary_elements<…>` instantiation. The display models use a
   second matcher family, `symbol_lookup_helper_with_registry` /
   `…_without_registry::matcher_scalar<T>`: one keyed by registry id and one
   by plain string.
2. **Field names**, from a contiguous key-string pool near offset 6.34 MB of
   the arm64 slice, in declaration order.

### Parameters are typed

| Kind (variant tag) | Value | Styles | Metadata |
|---|---|---|---|
| `continuous_parameter` | `continuous_parameter_value` | `knob`, `value` | — |
| `continuous_bipolar_parameter` | `continuous_bipolar_parameter_value` | `knob`, `value` | — |
| `discrete_parameter` | `discrete_parameter_value` | `value`, `menu`, `waveform`, `filtertype` | `discrete_parameter_metadata` |
| `discrete_bipolar_parameter` | `discrete_bipolar_parameter_value` | `value` | `discrete_bipolar_parameter_metadata` |
| `toggle_parameter` | `toggle_parameter_value` | `value`, `power`, `tempo_sync` | — |

Fields: `parameter_value`, `parameter_style`, `parameter_metadata`; style
sub-fields include `index`, `value_image`, `num_values`, `menu`, `image`.

Device-drawn graphics are enumerated: filter types `hi_pass`,
`resonant_hi_pass`, `lo_pass`, `resonant_lo_pass`, `band_pass`, `all_pass`,
`bell`, `prop_q`, `band_shelf`, `analog_lo_shelf`, `analog_hi_shelf`,
`baxandall_lo_shelf`, `baxandall_hi_shelf`, `comb`, `band_reject`; waveforms
`sine`, `square`…`square_4`, `triangle`…`triangle_4`, `saw`…`saw_4`, `noise`,
`pwm`, `wavetable`…`wavetable_4`, `complex`.

### The NKS layout

```text
host_owned_viewstate:   { nks1_layout: [nks1_parameter_page],
                          nks2_layout: { sections: [layout_section],
                                         groups:   [layout_group] } }
nks1_parameter_page:    { <name>, [optional<parameter_placement_info>; 8] }   one per knob
device_owned_viewstate: { view_address: nks1_view_address | nks2_view_address,
                          force_host_viewstate }
view address fields:    group_index, section_index, parameter_index
```

Eight placement slots per page is NKS's eight-knob page, modelled directly.

### Host state that likely drives the button LEDs

`transport_section_state` with `transport` ∈ {`stopped`, `paused`,
`playing`}, `recording`, `metro`, `loop`, `tap_tempo`, `count_in`, `armed`,
`disabled`; `undo_state` with `can_undo`, `can_redo`; `quantize_state` with
`can_quantize`, `can_quantize_half`; plus `tempo`, `current_preset_name`,
`current_track_name`, `current_track_number`, `status_message`,
`master_volume`.

The hypothesis is that transport LEDs follow host-reported state rather than
direct commands. Untested.

### Other pages

- **Mixer:** `track_infos`, `focused_track`, `selected_track_parameters`,
  `current_parameter_page`, `number_of_parameter_pages`, and per track
  `volume`, `volume_label`, `pan`, `pan_label`, `meter`, `mute`,
  `mute_by_solo`, `solo`, `arm`, `automation`, `color`, `short_name`,
  `selected`. Track types are `generic_track`, `midi`, `master`, `empty`.
- **Browser:** `filters`, `sounds`, `items`, `selected_index(es)`,
  `thumbnail_image`, `asset_image`, `is_favorite`, `is_user_saved`,
  `prehear_enabled`, `quickbrowse_enabled`, `speak_preset_enabled`.
- **Plugin chain:** `plugins`, `current_plugin_index`, `identifier`,
  `can_bypass`, `bypass_enabled`, `can_remove`, `can_move_forward/backward`,
  `supported_nks_version` ∈ {`non_nks`, `nks1`, `nks2`}.
- **SmartPlay:** `arpeggiator`, `note_repeat` (`rate`, `swing`, `hold`,
  `octaves`, `retrigger`, …), `scale_engine` (`scale`, `root_note`,
  `chord_set`, `user_chord`, `harmonizer`, `key_mapping`).

### The parameter entry (host observation)

Recovered field by field with `examples/page_probe.rs`:

```text
parameters: {
  <integer parameter id>: {          # string ids are fatal
    continuous_parameter: {          # exactly one key: the kind tag. A sibling
                                     # key such as "name" is fatal.
      parameter_value: { value: 0.5, display_value: "50%" },   # confirmed
      parameter_style: { knob_parameter_style: nil },          # confirmed
    }
  }
}
```

`toggle_parameter` parses with the same two required fields.

`parameter_style` is `variant<knob_parameter_style, value_parameter_style>`
for a continuous parameter, encoded as a **one-entry map from the tag to
`nil`**. Both `{knob_parameter_style: nil}` and `{value_parameter_style: nil}`
parse with no further missing keys, which makes this a complete parameter.

Six blind encodings failed first. Two decompiles settled it:

- `convert_variant_with_symbol_registry<variant<knob…, value…>, false>` at
  `0x1002be770` requires a map of size 1. It resolves the key through
  `continuous_parameter_styles_enum`, then requires the value's msgpack type
  to be **nil**. Reading the 24-byte `msgpack::object` layout (type at +0, via
  at +8) is what showed this checks the value's type. It is not checking that
  a map is empty, which is where `{tag: {}}` went wrong.
- `lookup_enum_from_string<continuous_parameter_styles_enum>` at `0x1002be654`
  binary-searches a two-entry table starting at the string
  `knob_parameter_style`. The tag spelling was right all along.

The parameter's name is not a sibling of the kind tag. It is most likely
carried by the NKS layout's per-knob placement, which matches how NKS
separates parameter values from page layout.

### The NKS1 layout (host observation + decompiled)

```text
host_owned_viewstate: {
  nks1_layout: [                                   # pages
    [                                              # a page IS a list of sections
      { section_name: "Main",
        parameters: [                              # up to 8 knob slots
          { id: 0, name: "Cutoff" },               # references parameters[0]
          nil,                                     # an empty knob slot
        ] },
    ],
  ],
  nks2_layout: nil,
}
```

- **A page is an array of sections**, not a `{name, placements}` struct as
  first read from the type list. `convert_scalar_struct_delegate
  <nks1_parameter_page>` (`0x1002e59d8`) requires a msgpack array and converts
  each element as a `layout_section`. That is a single-member struct encoded
  as its member. It matches NKS on screen, where section headers span groups
  of knobs.
- **A section** requires `section_name`. Its placement list is the optional
  field `parameters`. Offered alongside `placements`, `parameter_placements`,
  `slots` and `controls`, it was the only one the agent did not flag as
  unexpected.
- **A placement** is `{id, name}`, both required. `nil` is accepted as an empty
  slot.
- `nks2_layout: nil` and `nks1_layout: []` are both valid.

## CONFIRMED: a working parameter page with live knobs

**Evidence class: hardware observation, confirmed by the device's owner.** The
screen showed **TACTUS**, the section **Synth**, and eight named knobs.
Turning a knob moved its value on screen, tracking the hand.

Two pieces completed it:

**1. The view address.** Without `device_owned_viewstate` the page was
accepted but not shown. With it, the agent logged nothing at all and the page
appeared:

```text
device_owned_viewstate: { view_address: { nks1_view_address: <page index> },
                          force_host_viewstate: true }
```

Decompiled (`0x1002e0b64`): a one-entry map keyed by
`nks_view_address_property`. For NKS1 the value must be a positive integer
`<= 255`. The agent's own serializer writes it the same way.

**2. The host owns parameter values.** A turn arrives as a relative step,
`plugin_action {plugin_parameter_value_change: [id, delta]}` (steps of
about ±0.002–0.006). Nothing on screen moves until the host applies it and
sends the result back:

```text
client_parameter_page_set_parameter_value(instance, serial, id,
    { continuous_parameter: { value, display_value } })
```

Four arguments, from `args_to_tuple size mismatch: expected 4`; the value is
kind-tagged, as in the page.

**Touch.** Each knob reports `plugin_parameter_erp_touched: [id, true|false]`
when a finger lands and lifts, independently of turning. In one 90-second
session all eight knobs reported: 140 touch events, 1,444 turn steps.

### The complete call, as `tactus-odr` sends it

```text
client_parameter_page_set_data(instance, serial, {
  plugin_data: {
    name, background: nil,
    parameters: { <id>: { continuous_parameter: {
        parameter_value: { value, display_value },
        parameter_style: { knob_parameter_style: nil } } } },
    plugin_color: { r, g, b }, nks_control_color: { r, g, b } },
  device_owned_viewstate: { view_address: { nks1_view_address: 0 },
                            force_host_viewstate: true },
  host_owned_viewstate: {
    nks1_layout: [ [ { section_name, parameters: [ { id, name }, … ] } ] ],
    nks2_layout: nil } })
```

The agent logs no warnings for it.
