# winry315 — design

Winry315 macropad: 15 keys, 3 pressable rotary encoders, 27 RGB LEDs,
ATmega32U4. Sold as "YD3xn15mx"/"YD315"; usb id `f1f1:0315`.

## Dumb pad, smart host

All policy — which mode is active, what each knob does, what colour the pad is —
lives in the host daemon. The firmware owns exactly one piece of state: a
27-entry LED buffer the host paints into.

- Adding a mode, recolouring one, or changing a knob's meaning is a daemon
  rebuild rather than a DFU reflash. We flash once.
- Host-computed effects (audio-reactive LEDs off a PipeWire monitor tap) are
  only possible this way — the MCU cannot see the audio.
- Accepted cost: the pad is inert without the daemon. It falls back to dim white
  after 2s of silence so a dead daemon looks broken rather than unresponsive.

Keycodes were rejected as the transport: 24 inputs is past the spare F13–F24
range, encoder deltas and per-key RGB don't map onto keycodes at all, and
unbound keycodes leak into the focused window.

## One module per mode

`main.rs` is wiring only: open the pad, start the threads, run one loop that
folds inputs into state and draws. Everything a mode does lives under
`modes/`, and no mode can see another.

| Module | Owns |
|---|---|
| `pad.rs` | finding the pad, the report format, gamma and dimming, the heartbeat |
| `grid.rs` | which LED is where, and easing |
| `audio.rs` | capturing one PipeWire node, and turning levels into light |
| `board.rs` | what the pad is showing, against what is wanted |
| `modes/mod.rs` | the `SLOTS` table and one exhaustive match per dispatch |
| `modes/<name>` | that mode's state, picture, background threads and effects |

Modes never merge state — each is folded on its own and only the active one is
asked for anything. What they feed is a `Frame`: the picture wanted, plus the
levels to scale it by. The board reconciles that against what the pad is
actually showing and answers with the cells to send, or `None` when it is
already showing them.

    mode state ──► modes::frame ──► Board::reconcile ──► pad.show
                   (+ indicators)    (ease, modulate, diff)

A mode contributes a row to `SLOTS` — its key, its indicator LED, its colour —
and then only what it needs: a `State` with `apply` and `cells`, an `Input` its
own watcher sends, an `Action` its own `Effects` runs, a `Sources` for anything
live it reads. Every dispatch in `modes/mod.rs` matches exhaustively on `Mode`,
so the compiler names each place a new mode has to be wired and nothing else in
the tree changes. Colour (key 10) and Spotify (key 11) exist; the rest of the
bottom row is reserved for the modes in INTENT.md.

- **A `Frame`'s two fields are the two rendering rules.** `cells` is eased
  toward; `levels` scales the eased result and is never eased itself. INTENT
  says colours ease and levels do not, so a mode that wants a hit to land
  immediately says so with `levels` rather than moving its `cells`.
- **The modulated frame is never fed back into the ease.** `Board` keeps the
  eased picture and the sent picture apart; folding the scaled one back would
  drag the cover toward black through every quiet passage and make it climb out
  again on the next beat.
- **Live inputs sit beside the state, not in it.** `State` is `Copy` and folded
  from the channel; `Sources` holds the shared latest-value slots, so a frame
  reads the newest value rather than folding a queue of stale ones.
- **A watcher is handed only the means to announce its own input** — a closure
  that wraps that mode's `Input` — rather than the channel itself.
- **Every mode's `Sources` ticks, not just the active one.** Otherwise switching
  back reveals a frozen frame from minutes ago.
- The loop drains what has arrived and draws once per 30ms tick. Drawing per
  input instead ties every ease and decay rate to how fast a knob is turned.

## Wire protocol

32-byte reports both directions, first byte is the opcode.

Pad → host:

| Op | Payload | Meaning |
|---|---|---|
| `0x01` | `[1]`=index 0–17, `[2]`=1/0 | key down/up |
| `0x02` | `[1]`=index 0–2, `[2]`=int8 | encoder delta |

Host → pad:

| Op | Payload | Meaning |
|---|---|---|
| `0x02` | `[1]`=offset, `[2]`=count, `[3..]`=rgb triples | a run of LEDs, ≤9 per report |
| `0x04` | — | heartbeat |

Two opcodes: *here is some of the picture*, and *I am alive*. The whole pad is
three `0x02` reports.

**Colours are sent at full depth, and that is worth three reports.** Packing all
27 LEDs into one report needs a byte each; RGB332 was tried and every colour
then has to be reasoned about — its red and green scales have 8 levels against
blue's 4, they coincide only at 0 and 255, so grey is not representable and
neutral covers come out tinted. Fixing that in the packing takes thresholds and
search; not packing takes two more reports. The pad does not care and neither
does USB.

A run can be split across reports because the renderer eases toward its target a
quarter of the gap per tick, so consecutive frames differ slightly and a
half-applied update falls between two nearly identical pictures. (An earlier
design painted a wash and then patched indicators over it, which *did* flicker
about one update in eight — because those two reports disagreed sharply.)

- Key indices 0–14 are the keys in reading order; 15/16/17 are the left/centre/
  right encoder switches, renumbered from the raw matrix columns so they line up
  with the encoder indices.
- `0x04` is not optional: the host must talk at least every 2s or the pad
  decides the daemon is dead. A daemon that only sends on change will let the
  pad go grey while idle.

## Sound

Captured from Spotify's own node over PipeWire, four log-spaced bands per
channel, published as a latest value the renderer reads.

- **The sample buffer holds exactly one window, never a queue.** Whatever
  arrives is appended and anything that no longer fits is dropped off the front,
  so every analysis is of the newest audio. Draining half a window per callback
  and working through the rest in order puts the pad behind the music with no
  way to catch up.
- **Decay runs on the renderer's clock, not the audio thread's.** When playback
  stops the capture callback simply stops firing, so levels left in the slot
  stay lit -- the pad held its last frame for ten seconds after a pause.
  Published levels carry an `Instant`; anything older than `AUDIO_TIMEOUT` is
  read as silence and fades.
- **Brightness follows what just arrived, not what is present.** Music has
  energy in every band all the time, so a level display sits high and a kick
  barely moves it -- the pad reads as mud. Each band's rise since the last look
  is spectral flux, the standard onset detector; a hit produces a spike and a
  held note produces nothing. Measure it on the real energy, *before* the gain
  below: the gain makes every band use the whole range, so change measured
  after it makes a kick's faint high-frequency click as large a rise as the
  kick, and every hit flashes every row. `LEVEL_FLOOR` keeps a fraction of
  plain level underneath, because pure onset detection goes dark through a
  sustained chord.
- **Levels are normalised against a rolling peak.** A fixed dB window maps all
  ordinary music into the middle of the range, so the pad glowed at half
  brightness and nothing punched. The peak rises instantly and forgets with a
  2.5s half-life; `GAIN_FLOOR` stops silence being amplified into noise.
- **That bus name is owned by bubbled-spotify's `xdg-dbus-proxy`, not Spotify.**
  The `--dbus-own=org.mpris.MediaPlayer2.spotify` grant in
  `pkgs/bubbled-spotify` is what makes the pad able to drive it. Tighten that
  policy and the mode goes dead in a way that looks like a pad bug.
- A missing session bus is not fatal: the daemon warns once and the pad still
  works as a colour picker. Individual call failures (Spotify not running) go to
  stderr and are dropped.
- Both side knobs scrub, deliberately. Direction comes from the rotation, so the
  knobs' identity only carries meaning on their clicks, where it selects
  previous vs next track.

## What each LED is for

27 LEDs, two groups, from QMK's `initial_led_config`:

| LEDs | group | role |
|---|---|---|
| 6–20 and 0–5 | the 5x4 grid: the keys, plus the knob row above them | the album cover, tilted by stereo |
| 21–26 | underglow, two columns of three | a level meter per channel |

The knob LEDs are the grid's top row rather than a separate strip: they sit
directly above the keys, so they carry the top of the picture. Six of them over
five columns, and by position LED 3 and LED 2 both fall nearest the middle, so
`GRID` holds a *slice* per cell — usually one LED, once two.

The underglow is three down each side against four frequency rows, so each
position takes the nearest band. No stereo tilt there: the sides *are* the
channels, and tilting them would say the same thing twice. A test asserts every
LED belongs to exactly one group and that none is claimed twice.

## The grid

Album art fills all 21 grid LEDs -- the keys and the knob row above them. The
underglow carries on from the outer column of whichever row each side LED sits
beside, so the picture runs off the edges rather than stopping at them. Mode
indicators are composited last, over the cover -- knowing which mode you are in
beats seeing every pixel.

- **Blurhash, not a box average.** Averaging complementary colours gives grey,
  and at 5x4 each cell would average ~30,000 pixels of a 640x640 cover. Blurhash
  low-passes in *linear* light, and the gamma correction on the way to the LEDs
  restores the saturation that band-limiting costs.
- **The cover is square and the grid is 5:4**, so it is centre-cropped. Covers
  overwhelmingly put their subject in the middle; text near the top or bottom
  edge is lost.
- **The grid is serpentine, and row 0 is the bottom.** `GRID[col][row]` with
  columns alternating direction (`8,7,6` then `9,10,11`); there is no constant
  stride, so any per-LED effect goes through the table. Images count rows
  downward and the pad counts them upward, so anything drawn from an image has
  to flip. A test checks the table against the LED positions in QMK's
  `winry315.c`.
- Colours ease a quarter of the gap per 30ms tick. The step is floored at one:
  integer division stalls at a gap of 2 or 3 and would leave a colour
  permanently just short of its target.

## Getting colour out of the LEDs

- **Undo sRGB gamma on the way out.** Colours are gamma encoded for a screen and
  these LEDs are linear in duty cycle, so sending sRGB straight through lifts
  every dark channel about tenfold — a saturated red's 40 emits 15.7% of full
  light instead of 1.6%. That drags every colour toward white, and is why they
  looked pale. `(c/255)^2.2` restores the saturation the picture had.
- **Dim at the single output point**, so everything upstream keeps its full
  range and only the wire carries the reduced one. Rounding matters: truncation
  puts every dark colour at zero.
- The driver is ws2812 — 8 bits per channel, no current control — so dimming
  costs resolution and there is no hardware knob to use instead. `max_brightness`
  in `keyboard.json` does not apply to us: it scales QMK's own effects, not
  `rgb_matrix_set_color` writes from an indicator callback, so the pad was
  running at full 255.

## Album art

The cover is fetched on its own thread and folded into state like any other
input: `mpris:artUrl` from MPRIS metadata, fetched over HTTP, decoded and
low-passed onto the grid. Until it lands — and whenever it fails — the mode
falls back to its own green, so no part of the pad ever waits on the network.

- Spotify's art URL is a content hash, identical for every track on an album,
  so it doubles as the cache key. Fetched art is kept as JPEG under
  `$XDG_CACHE_HOME/winry315-daemon/art/<hash>`, written to a `.part` file and
  renamed so a crash cannot leave a torn image poisoning that album forever.
  The image is cached rather than the extracted colour because the network is
  the expensive half -- re-extracting after a tuning change is milliseconds and
  needs no refetch. Only the URL's last path segment is used, and only when it
  is short and alphanumeric, so a URL can never choose where we write.
- `Properties.Get` answers with a variant *wrapping* the `a{sv}`, so metadata
  needs one layer unwrapped before the dict is reachable. Deserialising straight
  to a map fails with `got 'v', expected 'a{sv}'`.
- The mode used to distil one dominant colour (k-means in Oklab, via
  `okolors`) and wash the pad with it. Showing the cover across the grid
  replaced that, and the dependency went with it. Two things that cost hours
  then, if a single colour is ever wanted again: judge colourfulness as
  **chroma ÷ lightness**, because Oklab chroma scales with lightness and an
  absolute threshold keeps pastels while discarding deep saturated reds — and
  lift a dark colour by **moving lightness, not clamping chroma to a floor**,
  which is what used to light the pad vivid blue for a white cover.

## Constraints that will bite

- **28672 bytes of flash** (32K minus the 4K DFU bootloader). nixpkgs' avr-gcc
  is far newer than QMK's CI toolchain and overflows the *stock* keymap by ~10%;
  `LTO_ENABLE = yes` alone brings it back under (31644 → 22530). No custom
  toolchain needed.
- `raw_hid_send` silently drops anything not exactly 32 bytes.
- **Liveness is its own protocol, on its own thread.** `0x04` pings every 500ms
  from a dedicated thread; frames go out only when state changes. Deriving the
  heartbeat from event traffic does not work: a knob held through a long sweep
  keeps the channel busy, so an idle-timeout heartbeat never fires and the pad
  hits its 2s cutoff and goes white mid-turn. Separating them also means a slow
  D-Bus call cannot starve liveness. The write end is an `Arc<Mutex<File>>`
  shared by the two.
- **The LED map is not the key map.** Reading order gives keys 10–14, but their
  LEDs are 8, 9, 14, 15, 20, and the grid is wired serpentine (see the LED map
  in `winry315.c`). `0x02` sidesteps this by carrying LEDs in index order, but
  any host-side code that thinks in rows and columns has to go through the
  `GRID` table.
- QMK's raw HID reports are **unnumbered**, so hidraw writes are 33 bytes — a
  leading `0x00` then the payload. Reads are 32.
- Flashing changes the USB product string (`dztech YD3xn15mx` → `Winry
  Winry315`), so match on vid:pid plus the `0xFF60` usage page, never the name.
  Only one of the three interfaces carries that usage page.
- Two udev rules are needed, for two different identities: `f1f1:0315` so the
  daemon can open `/dev/hidraw*` (root-only by default), and `03eb:2ff4` — the
  Atmel DFU bootloader the pad becomes while flashing — so the applicator does
  not need root either. Model both on the GameCube adapter rule in
  `mainframe-system-applicator`.
- Bootloader entry: hold the top-left key while plugging in, the physical button
  on the back of the PCB, or hold top-left + press bottom-right (the keymap's
  `QK_BOOT`, so iterating doesn't need unplugging).
- dfu-programmer 1.x starts the flashed application with `launch`; the `reset`
  command older guides use no longer exists. Getting this wrong leaves the pad
  in the bootloader looking unflashed when the write actually succeeded.
