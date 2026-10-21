# Kontrol S-series MK3 — ODR, the host-mode protocol

**This is the only way to drive the MK3's display.** In MIDI mode the keyboard
renders its own assignment list and the host cannot put anything on screen. To
show menus, browsers, parameter pages or meters, a host must speak ODR.

**Evidence class: vendor binary.** Recovered from symbols in
`NIHardwareConnectionService` (arm64 slice, `sha256
3a2a874742fab454802296ad95a3a935064b398d2b41f1c6a9557582c20a872c`). Nothing on
this page has been exercised against hardware yet.

## Transport

| Property | Value | Class |
|---|---|---|
| USB interface | **3**, named `KONTROL S88 MK3 ODR` by the device | hardware |
| Interface class / subclass | `0xFF` / `0x0D` | hardware |
| Endpoints | 3 | hardware |
| Wire format | msgpack-RPC, size-prefixed | vendor binary |
| Framing | `msgpack_support::size_prefixed_buffer`, `self_healing_stream_parser` | vendor binary |
| Alternate carrier | TCP — `socket_device<boost::asio::ip::tcp>`, `kks_mk3_simulator::prepareTCP` | vendor binary |
| Agent IPC socket | `/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks` | vendor binary |

Two carriers, one protocol:

```text
ni::odr::transport::usb::usb_device                 ← the keyboard
ni::odr::transport::socket::socket_device<tcp>      ← the simulator
ni::odr::transport::self_healing_stream_parser
ni::odr::agent::kks::device_factory                 ← builds from either
ni::odr::agent::kks::abstract_device_communicator
ni::odr::agent::kks::device_manager::hotplug_device_added(
    device_serial_tag, abstract_device_communicator)
```

**The TCP simulator is the single most useful thing on this page.** It means an
ODR client can be developed and tested with no keyboard attached, which turns
this from hardware archaeology into ordinary protocol work.

## Session lifecycle

```text
instance_hello / instance_heartbeat / instance_goodbye / instance_sync
client_id / client_info
host_protocol_version / device_protocol_version
device_connected / device_disconnected
client_instance_add / client_instance_remove
```

Focus arbitration — several clients can be attached, one owns the surface:

```text
client_focus_acquire / client_focus_release
client_request_focus / client_release_focus
focus_changed
active_instance_client_changed
client_set_host_owned_instance_client_state
client_instance_set_device_owned_instance_client_state
client_instance_set_host_owned_instance_client_state
device_owned_instance_client_state_changed
```

`ni::odr::protocol::kks::host::start_agent_heartbeat()` exists, so the
connection is kept alive explicitly rather than idling.

## Host → device: putting things on the screen

This is the API that answers "how do I send a menu to the keyboard". Every name
below is a real symbol in the shipped binary. `client_*` is the plain form;
`client_instance_*` is the instance-scoped form, for a host managing several
plug-in instances.

**Page selection** — four pages, per
[mk3-display.md](mk3-display.md): `browser=0, mixer=1, parameter=2, smartplay=3`.

```text
client_page_set / client_set_page
```

**Browser** — the preset menu:

```text
client_browser_set_data_model          client_browser_set_sounds
client_browser_set_filters             client_browser_set_filter
client_browser_set_filter_selection    client_browser_set_settings
client_browser_set_sound_item          client_browser_set_sound_selection
```

**Mixer**:

```text
client_mixer_set_trackinfo             client_mixer_set_track_data
client_mixer_set_meters
client_instance_mixer_update_property  client_instance_mixer_update_track_property
```

**Parameter pages** — the knob labels and values:

```text
client_parameter_page_set_data               client_parameter_page_set_plugin_data
client_parameter_page_set_parameter_value
client_parameter_page_set_device_owned_viewstate
client_parameter_page_set_host_owned_viewstate
client_instance_parameter_page_model_update_property
client_instance_parameter_page_model_update_parameter_value
```

**Plugin chain**, **SmartPlay**, **light guide**, **misc**:

```text
client_plugin_chain_set_data           client_plugin_chain_set_index
client_instance_plugin_chain_model_set_current_index
client_smartplay_set_data              client_smartplay_set_device_owned_viewstate
client_lightguide_set_leds             client_instance_lightguide_set
client_metro_flash                     client_notification
client_midi_addressing
```

Note `client_lightguide_set_leds`: the light guide is reachable **both** over
HID output report `0x82` (hardware-confirmed, see [mk3-hid.md](mk3-hid.md)) and
over ODR. The HID path works today and needs none of this.

Device → host state reports:

```text
device_properties / device_setting / device_viewstate
device_owned_viewstate / host_owned_viewstate
```

## Device → host: the control surface, already named

**This is the answer to semantic control naming.** The MK3 does not report
"button 7 pressed" — it reports `play`. The protocol is functional, so the
names come with it and there is nothing to map.

Recovered from `concrete_host_command_parser::register_handlers`, whose handler
variant enumerates every event the device can send. Four wrappers describe the
*shape* of an event:

| Wrapper | Meaning |
|---|---|
| `trigger_event_tpl<T>` | momentary — a button |
| `scroll_event_tpl<T>` | relative delta — an encoder |
| `sound_event_tpl<T>` | acts on a browser sound |
| `plugin_chain_entry_event_tpl<T>` | acts on a chain entry |

**Transport** (`odrda`), as triggers:

```text
play  restart  stop  record  metro  loop  count_in
undo  redo  quantize  quantize_half
set_tempo  tap_tempo        ← carry a value, not triggers
```

**Browser** (`odrbr`) — triggers and sound events:

```text
load  load_next_sound  load_previous_sound
toggle_favorite  toggle_prehear  toggle_speak_preset
toggle_filter_favorite_presets  toggle_filter_user_presets
quick_browse  reset_filters
```

and as scrolls — one per browser column:

```text
navigate_brand     navigate_filetype  navigate_product
navigate_bank      navigate_subbank   navigate_type
navigate_subtype   navigate_character navigate_sound
```

**Parameters** (`odrpa`):

```text
parameter_value_set      parameter_value_change
parameter_erp_touched    parameter_page_view_address_change
plugin_parameter_value_change   plugin_parameter_erp_touched
```

`erp` is almost certainly **endless rotary potentiometer** — the
touch-sensitive knobs — which makes `parameter_erp_touched` the touch sensor,
and plausibly explains the touch bits declared in HID report `0x02`. Marked as
inference, not fact.

**Plugin chain** (`odrpr`):

```text
plugin_chain_navigate (scroll)   plugin_chain_entry_move
plugin_chain_entry_bypass        plugin_chain_entry_remove
master_volume_change
```

**SmartPlay / arp / scale** (`odrsp`):

```text
arp_engine_change     arp_dynamic_change    arp_gate_change
arp_hold_changed      arp_swing_change
arp_min_key_change    arp_max_key_change
scale_engine_change   smartplay_scale_engine_change
note_repeat_gate_change  note_repeat_hold_changed  note_repeat_swing_change
smartplay_device_owned_viewstate_changed
```

### Strings deliberately excluded

The binary statically links OpenSSL and a crash reporter, so a naive string
sweep also returns `tls1_change`, `tls13_change`, `tls_construct_change`,
`ssl3_do_change`, `dtls_construct_change`, `program_change`, and the
infrastructure tags `request_id_tag`, `parse_tag`, `forward_traversal_tag`,
`add_sentryio_tag`, `client_identity_tag`, `device_serial_tag`,
`client_odr_client_name_tag`. **None of these are ODR events.** Recorded so the
exclusion is a decision rather than an oversight.

## The vendor left development flags in the shipped binary

**Evidence class: vendor binary.** Read out of `NIHardwareConnectionService`'s
string table as `getenv` keys. **None of these have been exercised yet** — an
attempt to launch the agent with them set was blocked by this environment's
command policy, so treat the descriptions as what the names claim, not as
observed behaviour.

| Variable | What the name claims | Why it matters here |
|---|---|---|
| `NI_ODR_AGENT_ENABLE_TCP` | Serve ODR over TCP as well as the Unix socket | A client could be developed against a local TCP endpoint, with no shared-filesystem socket and no keyboard |
| `NI_ODR_DISABLE_SYMBOL_REGISTRY` | Stop interning method names | **The single most valuable flag.** If the wire carries plain strings instead of registry indices, the "unknown index assignment" problem disappears entirely |
| `NI_ODR_DISABLE_HEARTBEAT_VALIDATION` | Stop requiring `instance_heartbeat` | Removes a timing requirement from a first client |
| `NI_ODR_DISABLE_SELF_HEALING_PARSER` | Disable `self_healing_stream_parser` | Framing errors would fail loudly instead of being silently resynchronised — better for learning the framing |
| `NI_ODR_IGNORE_API_MISMATCH` | Skip the protocol-version gate | A client reporting the wrong `host_protocol_version` would still be served |
| `NI_ODR_FORCE_API_MISMATCH` | Force the gate to fail | Useful only for testing the mismatch path |

Together the first two change the difficulty of this project materially: a
TCP endpoint speaking msgpack with **human-readable method names** is ordinary
protocol work, where a USB vendor interface carrying index-interned msgpack is
not.

## Observed: the agent accepts unauthenticated clients

**Evidence class: hardware/host observation.** Connecting a plain
`AF_UNIX`/`SOCK_STREAM` socket to
`/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks`
**succeeds**. The agent performs no handshake of its own and sends nothing for
at least 6 seconds: it waits for the client to speak first.

So there is no authentication barrier to becoming an ODR client. What is
missing is only the message format.

## The route into host mode

Assembled from the symbols above; **not yet executed**, so read it as the
hypothesis to test rather than a procedure that works.

1. Claim USB interface 3 (or connect the simulator over TCP).
2. `instance_hello`, exchange `host_protocol_version` / `device_protocol_version`.
3. Identify with `client_id` / `client_info`.
4. `client_instance_add`, then `client_focus_acquire` to own the surface.
5. `client_page_set` to choose a page.
6. Push a view model — `client_browser_set_sounds`, `client_mixer_set_trackinfo`,
   `client_parameter_page_set_data`.
7. Keep `instance_heartbeat` going; handle inbound events; `instance_goodbye`.

The manual says the keyboard falls back to MIDI mode when "there is no USB data
connection" and when Komplete Kontrol closes. The reading that fits is that
**host mode is a consequence of an ODR client holding focus**, not a mode set by
a command. Testable: step 5 either changes the screen or it does not.

## What is still unknown

| Unknown | Why it matters |
|---|---|
| The msgpack payload **schema** of every method above | Names without argument layouts are not yet callable |
| The **symbol registry** contents and index assignment | `ni::msgpack_support::symbol_registry_const` interns these names; the wire likely carries indices, not strings |
| ODR endpoint addresses, directions and packet sizes | `ioreg` does not expose pipe children for an unopened interface |
| Whether any **control transfer** or vendor request is needed to claim the interface | Affects step 1 |
| Whether macOS will hand interface 3 to a user-space process while NI's agent runs | Affects whether coexistence is possible at all |
| Whether the simulator ships in this install, and on what port | Would remove hardware from the development loop |

The symbol registry is the next thing worth extracting: if the wire carries
interned indices, the ordered registry *is* the method table, and it is the one
artifact that turns this list of names into a callable protocol.
