# Later

Small open items, parked so they are not lost. Each says what is known and
where the evidence is. Larger work lives in `PROGRAM.md`.

## Keyboard (ODR)

- **Undo / Redo lights and button.** `undo_state {can_undo, can_redo}` parses
  but the lights stay dark. With `can_undo: false` the Undo button sent nothing.
  Untested: pressing Undo while `can_undo` is true. `docs/protocol/mk3-odr-ipc.md`
  (per-step check), `kk-client.md` (Undo).
- **Host-driven blinking did not show.** Toggling `tap_tempo` four times a
  second while playing showed nothing for eight seconds. Steady `tap_tempo: on`
  does light while stopped. Try the same toggle while stopped, and at slower
  rates. Nothing in NI's own software blinks, so this is a curiosity, not a
  blocker.
- **`plugin_color`** parses but changes nothing on the parameter page. Find
  the page that uses it (plugin chain? browser?).
- **NKS2 layout** (`nks2_layout: {sections, groups}`, `performance_mode`,
  `nks2_view_address`). Unmapped. NI ships a 1280x152 banner beside the
  1280x212 one, which hints that NKS2 pages arrange the screen differently.
- **Per-entry images on menu parameters.** The registry groups `image` with
  `menu`, `index`, `num_values`. It might put icons on individual controls.
- **Mixer / track actions:** `select_track`, `mute_track`, `solo_track`,
  `track_volume_change`, `track_pan_change`, `octave_change`,
  `set_host_pad_mode` and five more. KK 3.5.4 knows them; agent 2.0.7, the
  latest, does not register them. Probably for Maschine or other hardware.
  Revisit with that hardware. `kk-client.md`.
- **Hard-coded serial** in the older examples. `Client::devices()` now
  discovers the keyboard, and `tactus-plugin-host`'s `kontrol` uses it; the
  ODR probes and tests still hard-code `99FE7169`.
- **Arc colours by eye.** Super 8 R2 (`#FC6949`) and Massive X (`#E17600`)
  both look orange. A visual check across products needs a pair that clearly
  differs (FM8 `#EBE9EA` against Super 8 `#24B0E8`, say). The mechanism itself
  is already confirmed.

## Hosting

The known gaps at the first playable milestone are listed in
`hosting.md` (Known gaps).

## Shim

- **Keyboard selector** in the strip; one keyboard is assumed. The default
  preset for a fresh shim still comes from `shim.conf`.
- **Transport buttons in a DAW:** VST3 gives a plugin no host-transport
  control. Options: send MIDI Machine Control, or let full mode bridge to the
  DAW's own control surface protocol.
- **Plug-in scan side effect:** the DAW's scan instantiates the shim, which
  loads its instrument and grabs the keyboard briefly.
- **No DAW on the development machine.** Ableton checks need the owner.

## Other

- **DAW-port MIDI protocol.** Recovered from NI's Cubase script, never
  hardware-verified. `docs/protocol/mk3-daw-port.md`.
- **Replacing the agent.** Drive USB interface 3 directly. The screen would
  still be a renderer on the device, so it would not give a full-screen
  image by itself.
- **The project name.** `tactus` is a placeholder; decide before publishing.
