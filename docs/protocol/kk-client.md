# Komplete Kontrol as an ODR client: what its code shows

**Evidence class:** static analysis of the vendor's shipped binary, with hardware
observation where noted. No vendor code or scripts are reproduced here; only
identifiers and behaviour, recorded for interoperability.

## Source

- `Komplete Kontrol.app`, version **3.5.4**, arm64 slice of the universal binary
  (fat binary sha256 prefix `5ba8a869e9da79aa`).
- The binary keeps its C++ symbol table: about **333,000** defined symbols,
  demangled with `c++filt`. That makes it far easier to read than the agent.
- Full Ghidra analysis does not finish (55 MB of code; the REA bridge times
  out). What works: find a field-name string in `__cstring`, scan `__text` for
  ARM64 `ADRP`+`ADD` pairs that form its address, then disassemble the hit
  with `objdump --demangle`. Calls show up by name because of the symbols.

## Host state is sent on change, never on a timer

`NI::KOMPLETE_KONTROL::HW::ODRDataSelectors::HostOwnedInstanceClientStateDataState`
is built from KK's model (`…DataState(DATA::Komplete&)`, `0x1000d98cc`) and has an
`operator==` (`0x1000d9170`). A `uniflow::data_store` observer compares the new
state with the last and only then runs
`Transformer<…>::transform()` (`0x1000da384`) and
`rpc_client::notify_rpc<…host_owned_instance_client_state_data…>`.

So KK cannot blink a transport light through host state. Blinking would need a
change on every beat, and nothing produces one.

### What standalone KK fills in

Reading `transform()`: it copies two or three strings and one float (most
likely the names and `tempo`), and sets two adjacent booleans to `true`
(probably `smartplay_enabled` and `browser_enabled`). It clears the presence
flags of the remaining optional fields. The transport, undo and quantize
sections appear to be left unset. This is inferred from field offsets, not
from names.

**Hardware observation, confirmed by the device's owner:** with standalone KK
running and FM8 loaded, the transport lights did not follow KK, nothing
blinked, and Tempo was not lit. KK has no tempo or transport settings of its
own. That matches the reading above: in NI's design the transport lights
belong to a DAW. They are steady states, not blinking ones.

## The full input vocabulary

`NI::KOMPLETE_KONTROL::HW::ODRDeviceActionHandler::operator()` takes one
`std::variant` covering every action the device can send. Its alternatives,
mapped to the wire names in the agent's registry (agent `2.0.7 (R15)`):

| KK type | Wire name | Seen on hardware |
|---|---|---|
| `odrbr::toggle_filter_favorite_presets` | `browser_toggle_filter_favorite_presets` | yes |
| `odrbr::toggle_filter_user_presets` | `browser_toggle_filter_user_presets` | yes |
| `odrbr::toggle_favorite` | `browser_toggle_favorite` | |
| `odrbr::quick_browse` | `browser_quick_browse` | |
| `odrbr::toggle_speak_preset` | `browser_toggle_speak_preset` | |
| `odrbr::toggle_prehear` | `browser_toggle_prehear` | |
| `odrbr::load` | `browser_load` | |
| `odrbr::load_previous_sound`, `load_next_sound` | `browser_load_previous_sound`, `browser_load_next_sound` | yes (preset up / down) |
| `odrbr::reset_filters` | `browser_reset_filters` | |
| `odrbr::navigate_{brand,filetype,product,bank,subbank,type,subtype,character,sound}` | `browser_navigate_…` | brand, bank, sound |
| `odrpa::parameter_value_set` | `plugin_parameter_value_set` | |
| `odrpa::parameter_value_change` | `plugin_parameter_value_change` | yes (knob turn) |
| `odrpa::parameter_erp_touched` | `plugin_parameter_erp_touched` | yes (knob touch) |
| `odrpa::parameter_page_view_address_change` | `plugin_parameter_set_page_index` | yes (page buttons) |
| `odrda::set_tempo`, `tap_tempo` | `daw_set_tempo`, `daw_tap_tempo` | |
| `odrda::{play,restart,stop,record,metro,loop,count_in}` | `daw_…` | play, stop, record, metro, loop |
| `odrda::{undo,redo,quantize,quantize_half}` | `daw_…` | no; see below |
| `odrpr::plugin_chain_navigate`, `plugin_chain_entry_move` | same | |
| `odrpr::plugin_chain_entry_{bypass,remove}` | `plugin_chain_entry_…` | |
| `odrpr::master_volume_change` | `master_volume_change` | |
| `odrsp::{arp_engine_change,arp_engine_trigger,scale_engine_change}` | `smartplay_…` | |
| `odrsp::smartplay_device_owned_viewstate_changed` | same | |

KK also knows these, but **agent 2.0.7's registry does not register them**.
They probably need a newer agent:

`set_host_pad_mode`, `toggle_node_collapsed`, `select_track`, `navigate_track`,
`navigate_track_depth`, `mute_track`, `solo_track`, `track_volume_change`,
`track_pan_change`, `select_host_plugin`, `toggle_plugin_container`,
`octave_change`.

## Undo

The Undo button sent nothing to the host when `can_undo` was false. The light
stayed dark with `undo_state: {can_undo: true}` (see `mk3-odr-ipc.md`). Whether
the button reports once undo is available has not been tested. The question
stays open.

## Also in the binary

KK carries NI's Lua controller scripts for Maschine hardware. Read only for
behaviour: on those HID-driven controllers the host sets an LED to a flash
state and refreshes it from a timer. That is not the MK3's ODR path.
