# Kontrol S-series MK3 — display architecture

**Answer: the keyboard renders its own UI. The host does not send pixels.**

There is no framebuffer to reverse-engineer on MK3, and no panel geometry to
recover. The host selects one of four pre-defined pages and supplies each with a
typed view-model; the device's own renderer draws it.

This matters because it is the opposite of the MK2/Maschine generation, where
the host composes a raster image and transmits it. Both models are supported by
this SDK, deliberately, as separate arms — see `tactus-display`.

**Evidence class:** vendor binary (`NIHardwareConnectionService`, arm64,
sha256 `3a2a8747…`), corroborated by hardware observation. Not hardware-verified
end to end: no session has been observed on the wire.

---

## 1. The deciding evidence

`ipc_mediator::handle_instance_display_set_page` @ `0x10022b61c` forwards its
page argument **unchanged, by value**, to
`protocol::kks::host::send_instance_set_page` @ `0x1003a9318`, whose signature
decompiles with the page as a **single byte**. The wire method is the literal
string `client_page_set`.

The complete display surface of that mediator is semantic. Observed handlers:

```
handle_instance_mixer_set_meters / set_trackinfo / set_track_data
handle_instance_parameter_page_set_model / set_parameter_value / set_plugin_data
handle_instance_browser_set_sounds / set_filters / set_sound_item
handle_instance_leds_set_lightguide
handle_instance_smartplay_set_data
handle_instance_plugin_chain_set_data
```

There is no `set_pixels`, `set_image`, `blit`, or `draw` anywhere in it.

Corroborating: the protocol vocabulary contains **`renderer_version`** — a
property the *host queries from the device*. The renderer is on the keyboard.

## 2. The page enum — fully recovered

Decompiled from the vendor's own symbol-to-enum lookup-table builder
(`to_enum<instance_page, ni::symbol>` @ `0x1002525a0`):

| Page | Wire byte | Vendor symbol |
|---|---|---|
| Browser | `0` | `browser` |
| Mixer | `1` | `mixer` |
| Parameter | `2` | `parameter` |
| SmartPlay | `3` | `smartplay` |

A four-value, one-byte, symbol-encoded enum. It is an **index selecting a page
the keyboard draws itself**, and it maps exactly onto the four structured model
families above. This single function settles the architecture.

Implemented as `tactus_display::Page`.

## 3. Wire format

**msgpack-RPC over a Unix domain socket.** Confirmed by
`ni::msgpack_rpc::build_notify`, `protocol::msgpack_sender::send_notification_impl`,
`make_host_parser_msgpack` @ `0x10036c248`, and `handle_msgpack_stream_corrupted`.

This **vindicates** the community claim of "msgpack-RPC over a Unix socket",
which earlier work in this project had marked *questionable* on the grounds that
a device descriptor names a Mach port. Both exist; the socket is real.

| Aspect | Detail |
|---|---|
| Socket | `/Users/Shared/Native Instruments/com.native-instruments.odr_agent.kks`, plus a `.lock` |
| Framing | Size-prefixed (`msgpack_support::size_prefixed_buffer`, `size_prefixed_message_chunk_tag`) |
| Notifications | msgpack arrays; a 3-element array (leading byte `0x93`) observed inline at `0x1003a5d74` |
| Method naming | Interned to a `uint16` index via a **negotiated** `symbol_registry`, falling back to the literal string when the registry lacks the symbol |
| Buffers | Reusable pack buffer 1 MiB; default buffer `0x2000` |
| Reassembly | `self_healing_stream_parser`, with `message_accumulation_started/continuing/finished` |

**Transport is agnostic.** Either a USB bulk pipe (`connect_bulk_pipe`,
`bulk_write_helper`) or **TCP**.

> `make_sdb_write_compression_timer_handler` / `ni::asio::compression_timer` is
> **write coalescing**, not image compression. Easy to misread.

## 4. There is a software simulator

`ni::odr::agent::kks::kks_mk3_simulator::prepareTCP` exists alongside
`kks_mk3_device::prepareUSB`.

The same protocol runs over TCP to an NI software simulator. The MK3's MIDI port
list independently includes `Kontrol MK3 Simulator DAW`. **This is potentially
the single most useful development affordance found:** a route to exercising the
protocol with no keyboard attached. Not yet located on disk — see §7.

## 5. The asset channel is not a framebuffer

The one real qualifier. The host *can* send raw pixels:

- `asset_device_model_observer::upload_asset` @ `0x1003a5d74`, wire method
  `assets_add`, payload `(device_asset_id, sha256 array<byte,32>, span<byte const>)`
- The vocabulary contains `is_rgb565`, so artwork may be uploaded pre-converted

But this is **content-addressed, deduplicated, disk-cached, persistent blob
storage** — `asset_cache::make_cache_file_name`, `revive_asset`,
`clear_device_storage`, `total_asset_space`. And the `image` / `asset_image` /
`thumbnail_image` fields occur only inside browser list-row models:

```
{ name, image }
{ identifier, name, icon, asset_image, is_favorite, is_user_saved }
```

That is an **icon and artwork channel**, not a screen framebuffer. A host
uploads a preset's thumbnail once; it does not stream frames.

Separately, Komplete Kontrol's own `rgb565` *code* is all vendored Qt raster
(`blend_untransformed_rgb565`, `QT_FT_Span_`) and libwebp — platform-generic,
not MK3-specific. Do not mistake it for a display path.

## 6. Geometry is absent, consistently

No width, height, stride or resolution field exists in either binary's protocol
vocabulary (361 wire strings recovered from the service, 184 from Komplete
Kontrol; scanned for pixel/bitmap/framebuffer/blit/stride/scanline/bpp/width/
height/geometry/resolution — zero hits outside §5).

A bounded scan of the service's `__text` for immediate moves (78,325 parsed,
decimal and hex forms) found **no co-located 480×272 pair**; the single 320/240
adjacency sits inside OpenSSL `dtls1_get_record` and is ruled out.

This is **consistent with, and explained by, the architecture**: the host never
needs geometry because it never composes a screen.

Independent corroboration already established elsewhere in this project:

- The vendor device table records all three MK3 keyboards as format `0xff`,
  width 0, height 0 — a sentinel, verified against working siblings in the same
  table (S49 MK2 = `0x60`, 480×272; Traktor S4 MK3 = `0x21`, 320×240).
- No `Komplete::KontrolMK3` class exists in any of the vendor's pixel-stack
  binaries, and `eUSBDeviceType 4` is never requested by any examined code.
- **Hardware-observed:** the real MK3 HID descriptor declares feature reports
  only `0xD0`, `0xD8`, `0xD9`. The pixel stack's geometry report `0xF8` is **not
  implemented**, and `D0`/`D8`/`D9` carry no geometry even with Komplete Kontrol
  live (`D0` reads all zeros).
- `kks_mk3_device` has only four members — `device_serial`, `get_device_info`,
  `prepareUSB`, destructor. No display method.

## 7. Unknowns, with the bounds of each negative

| Unknown | Bound of the negative search |
|---|---|
| Panel geometry | Covers both binaries' protocol vocabularies and immediate-move constants in the service's `__text` for 13 common dimension values. Does **not** cover `__const`/`__data` tables, computed or shifted constants, or the device firmware. |
| Whether any non-ODR path can send a raster screen | Bounded to ODR symbols in these two binaries. The `0xff`/0×0 sentinel and absent `0xF8` corroborate, but firmware and the DFU / `MK3BD` bulk-display path were not examined. |
| `assets_add` blob encoding | When `is_rgb565` is true versus an encoded file, and whether asset dimensions follow an agreed convention — not determined. |
| Method index numbering | `get_protocol_symbols` @ `0x10036eb24` was not decompiled. Method **names** are recovered; the `uint16` index mapping is not enumerated. |
| Payload schemas | Field names are recovered for several models; full schemas and types are not. |
| Simulator location | `prepareTCP` exists in code; no simulator binary has been located on disk. |

## 8. What this means for the SDK

1. **Drop the MK3 framebuffer ambition.** It was based on a false premise. This
   is a better outcome: the remaining work is a documented RPC protocol rather
   than an open-ended raster format with an unknown panel size.
2. The MK3 display arm is `DisplayModel::Pages`, carrying **no geometry** —
   its absence is load-bearing, not an omission.
3. The pixel encoder in `tactus-display::encode` remains fully valid and
   necessary — for **MK2, Maschine and Traktor**, whose geometry *is* known.
4. The light guide has **two** routes: this protocol
   (`leds_set_lightguide`) and the raw HID output report `0x82`. The HID route
   needs no agent and is the better primary.
5. Highest-value next steps: enumerate the method-index registry, recover
   payload schemas, and find the simulator.
