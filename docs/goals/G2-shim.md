# Goal G2 — the shim

Build tactus's shim mode in /Volumes/Main External/Development/tactus: a thin
VST3 plugin ("the frame") that a DAW such as Ableton loads, which in turn loads
one VST3 instrument, passes audio and MIDI straight through, and puts that
instrument on a Kontrol S88 MK3 keyboard with no Komplete Kontrol running: the
preset's NKS pages, banner and arc colours on the screen, knobs that set the
instrument's parameters, page and preset buttons, transport buttons and
lights, and the light guide.

Constraints:
- Rust throughout. A thin Objective-C/Swift shell is allowed only if an
  AUv3 build needs it.
- Real-time discipline on the audio thread: no allocation, no locks, no
  blocking. The keyboard (ODR agent), MIDI-to-screen and file work run on
  other threads and never stall audio.
- The frame's window is the instrument's own window plus a thin strip with
  only the NKS settings (which plugin, which preset folder, which keyboard).
  Nothing more.
- The frame's state, saved in the DAW project, holds the instrument's id and
  its state as opaque bytes. Nothing inside a plugin's state is decoded.
- No NI assets shipped, nothing signed with NI, no NI trademarks in names.
  The plugin handles its own licence check; the shim never touches it.
- Keep cargo fmt, clippy -D warnings and cargo test green. Commit and push to
  the private repo (MichaelDanCurtis/tactus) as work lands.

Done when:
1. tactus's own host loads the shim, the shim loads Massive X, and it plays
   from the keyboard (self-test, no DAW).
2. In Ableton, the shim on a MIDI track plays Massive X from the keyboard,
   with the screen, knobs, page and preset buttons working, as confirmed by
   the owner.
3. Saving and reopening the Ableton set restores the instrument and its
   sound.
4. Two shims in one set: the keyboard follows whichever track is selected,
   or the last-touched shim if selection cannot be seen.
5. Opening the shim's window shows the instrument's own window.

Write findings to docs/ (hosting.md, a new shim.md) and open ends to
docs/LATER.md. Goal G1 (plugin windows, clock, audio output, AU, NSH UI) is
the follow-on.
