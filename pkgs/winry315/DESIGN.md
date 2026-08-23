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

## Wire protocol

32-byte reports both directions, first byte is the opcode. This is the contract
the single flash locks in, so unused opcodes ship in the firmware from day one.

Pad → host:

| Op | Payload | Meaning |
|---|---|---|
| `0x01` | `[1]`=index 0–17, `[2]`=1/0 | key down/up |
| `0x02` | `[1]`=index 0–2, `[2]`=int8 | encoder delta |

Host → pad:

| Op | Payload | Meaning |
|---|---|---|
| `0x01` | `[1..3]`=rgb | solid fill (superseded by `0x05`) |
| `0x02` | `[1]`=offset, `[2]`=count, `[3..]`=rgb triples | per-key chunk, ≤9/report (superseded by `0x05`) |
| `0x03` | `[1..3]`=rgb, `[4]`=count, `[5..]`=levels | per-column bar graph |
| `0x04` | — | heartbeat |
| `0x05` | `[1..3]`=wash rgb, `[4]`=count, `[5..]`=`(led, r, g, b)` × ≤6 | whole frame |

**Use `0x05` for anything the host draws.** A frame has to arrive in one report:
`raw_hid_receive` and `rgb_matrix_task` both run from QMK's main loop, so the
matrix can render *between* two reports and briefly show a half-applied frame.
Painting a wash with `0x01` and then patching indicators with `0x02` visibly
flickers those indicators roughly one update in eight (reports land ~1–2ms
apart against a ~16ms render interval).

Raw pixels can't work — 27 LEDs × 3 bytes is 81, over the 32-byte report — so
`0x05` sends a background wash plus up to six per-LED overrides, which is enough
for a full bottom row of mode indicators. The daemon has a `const` assertion
tying `MODE_SLOTS.len()` to that limit so adding a seventh indicator fails to
compile rather than silently truncating.

- Key indices 0–14 are the keys in reading order; 15/16/17 are the left/centre/
  right encoder switches, renumbered from the raw matrix columns so they line up
  with the encoder indices.
- `0x04` is not optional: the host must talk at least every 2s or the pad
  decides the daemon is dead. A daemon that only sends on change will let the
  pad go grey while idle.
- `0x03` bakes a little rendering policy into the firmware, accepted only
  because it is 3× cheaper on the wire than `0x02` at audio frame rates.

## Mode selection

The bottom row of keys picks the mode, one key per mode, driven by a single
table in the daemon so the key and its indicator LED cannot drift apart.

Only colour mode exists so far. The rest of the row is reserved for the modes
in INTENT.md.

## Constraints that will bite

- **28672 bytes of flash** (32K minus the 4K DFU bootloader). nixpkgs' avr-gcc
  is far newer than QMK's CI toolchain and overflows the *stock* keymap by ~10%;
  `LTO_ENABLE = yes` alone brings it back under (31644 → 22530). No custom
  toolchain needed.
- `raw_hid_send` silently drops anything not exactly 32 bytes.
- **The bottom row's LEDs are not contiguous.** Reading order gives keys 10–14,
  but their LEDs are 8, 9, 14, 15, 20 (see the LED map in `winry315.c`). `0x02`
  addresses a contiguous run, so painting that row means one report per key
  rather than one chunk for the row.
- QMK's raw HID reports are **unnumbered**, so hidraw writes are 33 bytes — a
  leading `0x00` then the payload. Reads are 32.
- Flashing changes the USB product string (`dztech YD3xn15mx` → `Winry
  Winry315`), so match on vid:pid plus the `0xFF60` usage page, never the name.
  Only one of the three interfaces carries that usage page.
- Two udev rules are needed, for two different identities: `f1f1:0315` so the
  daemon can open `/dev/hidraw*` (root-only by default), and `03eb:2ff4` — the
  Atmel DFU bootloader the pad becomes while flashing — so the applicator does
  not need root either. Model both on the GameCube adapter rule in
  `desktop-system-applicator`.
- Bootloader entry: hold the top-left key while plugging in, the physical button
  on the back of the PCB, or hold top-left + press bottom-right (the keymap's
  `QK_BOOT`, so iterating doesn't need unplugging).
- dfu-programmer 1.x starts the flashed application with `launch`; the `reset`
  command older guides use no longer exists. Getting this wrong leaves the pad
  in the bootloader looking unflashed when the write actually succeeded.
