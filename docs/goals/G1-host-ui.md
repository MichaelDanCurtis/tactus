# Goal G1 — a real host: plugin windows, a clock, audio output choice, AU, and a UI built on NSH

**Status:** proposed, 2026-10-22
**Starting point:** `tactus-plugin-host` plays an NKS preset in a VST3 plugin
from the keyboard, with no Komplete Kontrol (see `docs/hosting.md`). It is a
command-line example with no window, a fixed 120 BPM time context, the default
audio output only, and VST3 only.

**Done means:** a desktop app, built on NSH, where you pick a preset, the
plugin loads in VST3 or AU form, its own window opens on request, it follows a
tempo you control from the screen or the keyboard's transport, and it plays
through the audio output you chose. Every hardware-facing item is confirmed on
the S88 MK3 by its owner, not only by tests.

**Out of scope:** VST2 (skipped by decision), CLAP (the owner is researching
it first; see the end), the patchable rack (`PROGRAM.md` §D), which G1 lays
groundwork for but does not build.

---

## Workstream 1 — Research NSH, then decide (gate for everything visual)

NSH (`github.com/longbridge/nsh`, Apache-2.0) is Longbridge's "Native Shell":
native UI written in Rust and TSX, rendered with each platform's own controls
(macOS, Windows, Linux, iOS, Android, GPUI, web). It is pre-1.0 (0.1.x) and
moving quickly, so the first job is to find out what it can do for a host,
not to assume it.

Questions to answer, with a small spike app for each:

1. **Embedding a foreign native view.** A plugin's GUI attaches to a native
   `NSView` (macOS) or `HWND` (Windows) that the host provides. Can NSH give
   us a view handle to hand to `IPlugView::attached`, or host a child view we
   create ourselves? *This decides the architecture.* If NSH cannot, the
   fallback is a separate top-level window that the host owns directly
   (`objc2` / AppKit), with NSH for everything else.
2. **Threads.** NSH's UI thread must be the main thread that VST3 controllers
   and AU views expect. Can the audio engine, the agent connection and the MIDI
   thread live beside NSH's event loop without starving it? Does NSH pump the
   main run loop so `pump_main_run_loop` is no longer needed?
3. **Rust ↔ TSX boundary.** How do Rust state (preset list, parameters,
   transport, devices) and UI events cross into TSX and back? How frequently
   can it update (parameter values move at audio-control rates)?
4. **Packaging.** Can it build a signed macOS `.app` alongside the existing
   workspace (`cargo` + its toolchain), with no web runtime shipped?
5. **Licence and health.** Apache-2.0 matches ours. Record the version pinned,
   open issues that touch the above, and how active maintenance is.

**When NSH falls short, we extend it.** Owner's decision: if the host
needs something NSH cannot do (a native child view for plugin windows, say),
we build it and offer it upstream as a pull request explaining why. Open an
issue first so Longbridge can say whether they want it and in what shape.
Keep the change in a fork, pinned, until it is merged. Read their
contribution terms (CLA or DCO) before the first PR; the licences match
(Apache-2.0).

**Deliverable:** `docs/research/nsh.md` with the answers, the spike code, and
a go/no-go. **Gate:** a window from NSH showing live parameter values from a
running plugin, and the answer to question 1 demonstrated, not inferred.

## Workstream 2 — Plugin windows

- VST3: `IEditController::createView("editor")` → `IPlugView`;
  `isPlatformTypeSupported("NSView")`, `attached(view, "NSView")`, `getSize`,
  `onSize`, and the host's `IPlugFrame::resizeView` for plugins that resize
  themselves. Close cleanly with `removed()` before the view is released.
- AU (Workstream 4): `kAudioUnitProperty_CocoaUI` (AUv2) or
  `requestViewController` (AUv3).
- Edits made in the plugin window arrive through `IComponentHandler::performEdit`
  (already recorded by the host). Show them on the keyboard's screen and send
  them to the processor.
- **Hardware gate:** open the Massive X window from the app. Turn a knob in
  the window and watch the keyboard's screen follow. Turn a knob on the
  keyboard and watch the window follow.

## Workstream 3 — Tempo and the clock

Today the plugin sees 120 BPM, 4/4, transport stopped, forever.

- A host transport: tempo, time signature, play/stop, position in samples and
  quarter notes, fed into `ProcessContext` every block (`kPlaying`,
  `kTempoValid`, `kTimeSigValid`, `kProjectTimeMusicValid`,
  `kBarPositionValid`).
- Control it from the UI **and from the keyboard**: Play, Stop and Tap Tempo
  arrive as `Event::Transport` already. The transport lights follow host state
  through `set_host_state`, confirmed for 12 of 14 states.
- Tempo on the keyboard's screen through the host state's `tempo` field.
- Optional, stretch: follow an external MIDI clock, or Ableton Link.
- **Hardware gate:** a tempo-synced Massive X patch (a synced LFO or arp)
  speeds up and slows down with the tempo. The Play button starts and stops
  it, and Tap Tempo sets the tempo.

## Workstream 4 — Choosing the audio output

- List output devices (`cpal`), with sample rates and buffer-size ranges.
- Choose device, rate and buffer size in the UI. Persist the choice.
- Changing device restarts audio without reloading the plugin: stop the
  stream, `setupProcessing` at the new rate, start again.
- Handle f32 and the other common sample formats, not f32 only.
- Survive the device disappearing (headphones out, interface unplugged).
- Look into the start-up underrun noted in `hosting.md`.
- **Hardware gate:** switch between the Mac's speakers and an interface while
  holding a note, with no crash and the plugin state intact.

## Workstream 5 — Audio Units

- Find AUs through AudioToolbox (`AudioComponentFindNext`, type `aumu` for
  instruments) and load them (`AudioComponentInstanceNew`).
- Render through `AudioUnitRender` with a render callback for MIDI
  (`MusicDeviceMIDIEvent`), feeding the same `Audio`-style handle the VST3
  path uses, so the rest of the host does not care which format it is.
- **Presets:** an NKS preset's `PCHK` is the VST3 state for VST3 plugins. Whether
  NI's AU builds accept the same bytes (through `kAudioUnitProperty_ClassInfo`)
  is unknown. Find out with Massive X, which ships both. If they do not, AU is
  for non-NKS plugins and NKS stays VST3-first.
- Parameter ids: map NKS knob ids to AU parameter ids, and check whether
  they match as they do for VST3.
- **Hardware gate:** Massive X AU plays from the keyboard with its knobs on the
  screen. A third-party AU instrument with no NKS support at all also plays.

## Workstream 6 — The app

The `kontrol` example becomes `tactus-app`, an NSH application:

- a preset browser (product, bank, type, from `tactus-nks`), with the
  keyboard's preset buttons and 4-way control driving the same list;
- the loaded plugin, its parameter pages mirrored from the keyboard, and a
  button to open its window;
- the transport (tempo, play/stop, time signature);
- audio settings (device, rate, buffer);
- status: agent connected, keyboard found, plugin loaded, CPU and underruns.

The keyboard keeps working with the app's window closed.

---

## Order

1. Workstream 1 (NSH research). Its answer to question 1 decides how
   Workstream 2 is built.
2. Workstreams 3 and 4 in parallel with it: neither needs a UI to start, and
   both can be checked with the existing example.
3. Workstream 2, then Workstream 6.
4. Workstream 5 (AU) last, once the host's shape is settled, so AU slots in
   behind the same interfaces.

## How we know it is done

| # | Check | Who |
|---|---|---|
| 1 | NSH research written up, spike demonstrates embedding or the fallback | Claude |
| 2 | Massive X window opens from the app; window and keyboard follow each other | owner, on hardware |
| 3 | Synced patch follows tempo; Play, Stop and Tap Tempo work from the keyboard | owner, on hardware |
| 4 | Output switched mid-note without a crash | owner |
| 5 | Massive X AU and a non-NKS AU both play | owner |
| 6 | The whole flow from the app: pick, load, open, play, change tempo, change output | owner |

Tests and gates (`cargo fmt`, `clippy -D warnings`, `cargo test`) stay green
throughout. Each workstream's findings go into `docs/`, and open ends go to
`docs/LATER.md`.

## CLAP — pending the owner's research

CLAP is an open plugin standard (MIT), designed host-first, with a simpler
threading model than VST3 and good per-note expression. It would fit the same
`Audio` interface. Whether NKS presets can target CLAP plugins is unknown;
NKS's `PLID` chunk names VST2 and VST3 ids only, as far as seen so far. It is
not part of G1 until the owner decides.
