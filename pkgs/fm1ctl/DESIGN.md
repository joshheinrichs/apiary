# fm1ctl

Talks to an M-Vave FM-1 over USB MIDI. The vendor ships a Windows and macOS
updater only; this is the Linux side of it.

```
fm1ctl ports
fm1ctl info
fm1ctl firmware [list|fetch|flash]
```

Grouped as a noun with verbs because presets are a plausible second noun
later; `firmware fetch` and `presets fetch` stay parallel where a top-level
`fetch` would not.

## Where the protocol came from

`M-UPGRADE-FM1.exe` (Qt 6, x86-64 PE) from the vendor's `FM1.zip`. Two things
in that zip did most of the work:

- **The vendor left their development logs in `LOG/`.** `app_log.txt` is 19k
  lines of real OTA sessions from 2025-09 to 2026-06, covering the FM-1 plus
  MK300, URM-1000, TANK PRO, FX-120 and SquareMoon. It gives away the whole
  control flow; the disassembly only had to confirm byte offsets.
- **The firmware is embedded in the exe** as Qt resources, `qt_resource_data`
  based at file offset `0x24a30`: `:/Resources/FM-1.fwsc` (704,052 bytes, at
  `0x28a38`) and `:/Resources/FM-1_4bank.bin` (16,384 bytes, at `0x24a34`).

The SoC is a JieLi AC791N; `.fwsc` is a JieLi container, magic `JLUFW` in the
last 16 bytes, and the payload names itself `usb_hid_ota.bin`.

## The update is device-driven

The host does not stream firmware. It answers questions. The device sends
`{flash_type, address, length}` and the host replies with those bytes of the
`.fwsc`. Consequences worth knowing before writing the `flash` command:

- **Addresses are raw file offsets and are not monotonic.** The device
  re-reads ranges and jumps around, so the file has to be held in memory or
  seekable; a sequential cursor will not do. A logged v8 -> v10 upgrade was
  1168 requests in about 13 seconds, nearly all of them 512 bytes.
- **Two addresses are signals, not offsets.** `0xE0000000` means verification
  finished and `0xF0000000` means the upgrade finished. Both are answered with
  the literal `success\0`, framed as an ordinary data reply.
- **Entering OTA mode is one hardcoded MIDI message**, `F0 22 24 35 7F F7`,
  sent verbatim rather than as a packet. Sent once in normal mode -- the device
  reboots and comes back with a different USB MIDI port name, identifying as
  `ota-FM-1` -- then again after reconnecting, which starts the transfer.
- The vendor's own step 2 passes `skipNameCheck = 1`, i.e. it stops checking
  device identity once the device is in OTA mode.

## The captured-bytes check

`codec.rs` has one known-answer test that is worth more than the rest
combined. An identify query -- magic, type, u24 length, inverted checksum, and
the little-endian 7-bit bitstream packing -- has to be assembled correctly for
these ten bytes to come out:

```
F0 00 32 45 00 00 00 40 7F F7
```

Those are the bytes a third party captured off the wire and published as the
FM-1's handshake, having read `00 32 45` as a manufacturer ID prefix. It isn't
one: it is the encoded form of `00 59 11 00 00 00 FF`. That the two derivations
meet, from a disassembly and from a capture, is the strongest evidence the
codec is right short of plugging in hardware.

The same capture's reply decodes to a 34-byte packet, which is exactly the
length the vendor tool requires of an identify reply -- so the framing holds in
both directions.

Beware the rest of that third-party work (`aroum/fm1-custom-fw`): its
`fm1_flasher.py` pushes sequential chunks at a device that pulls, uses
MSB-grouped 7-bit packing rather than a bitstream, and invents its metadata
payload. It cannot work, and its README is honest that it is untested.

## Identity, confirmed against hardware

`info` against a real FM-1 returns type `0x11` with a 27-byte payload holding
one NUL-terminated string: `FM-1_015`. Name and version in one field, same
`name_version` convention as the `.fwsc` filenames, and the version is plain
text -- not the decimal-digit encoding the disassembly's `+ '0'` loop at
`0x140017340` suggested. That loop does something else.

Do not use the `.fwsc` embedded in the updater. The vendor publishes the real
one unauthenticated at

```
https://yms-file-store.oss-cn-hongkong.aliyuncs.com/software/firmware/FM-1.fwsc
```

listed on `m-vave.com/download` as **V15, 2026_07_30**, 699,956 bytes. The copy
embedded in the updater zip is a different build -- 704,052 bytes, exactly
0x1000 larger, and the zip predates the published file by three weeks -- so it
is stale. Both are structurally valid containers with identical file tables;
only the payload and the four leading bytes (which look like a checksum)
differ.

That URL sends no CORS headers, so a browser page cannot fetch it
cross-origin. A web flasher has to take the file from the user instead, which
is where it wanted to be anyway.

The firmware is runtime data, not a build input. `fm1ctl firmware --fetch`
downloads it into `$XDG_STATE_HOME/fm1ctl/`, named by hash. Nothing in the
repo or the derivation redistributes it, which matters because this repo is
public.

State rather than cache, deliberately. A cache is defined by being
regenerable, and this is not: the vendor overwrites `FM-1.fwsc` in place, has
no versioned paths (unlike their own `FootCtrl_051.fwsc`, `SMK25II_156.fwsc`),
blocks bucket listing, and is not archived by the Wayback Machine. Once V16
ships, the V15 bytes are gone from everywhere public, so a cache cleaner would
destroy the only surviving copy of a build.

## Name and version, out of the container

The container opens with a table of 48-byte slots. Each slot carries **one
character, at byte 47, obfuscated by subtracting the slot's own index plus
one**. Decoded left to right they spell `NAME_VVV`; `}` in the *raw* byte ends
the string and pads the rest of the table. Version digits are positional,
hundreds first, so three at most. The vendor tries 36 slots and falls back to
20; FM-1 images are 20-slot.

Worked from the real V15 download, whose first eight slot bytes are
`47 4f 30 35 64 36 38 3d`:

```
slot 0: 0x47 - 0 - 1 = 0x46 'F'      slot 4: 0x64 - 4 - 1 = 0x5f '_'
slot 1: 0x4f - 1 - 1 = 0x4d 'M'      slot 5: 0x36 - 5 - 1 = 0x30 '0'
slot 2: 0x30 - 2 - 1 = 0x2d '-'      slot 6: 0x38 - 6 - 1 = 0x31 '1'
slot 3: 0x35 - 3 - 1 = 0x31 '1'      slot 7: 0x3d - 7 - 1 = 0x35 '5'
```

`FM-1` and `015`, matching what the device reports over MIDI and what the
download page advertises. The updater's embedded copy decodes to **V14** off
the same table with one byte different (`0x3c` in slot 7), which is what makes
it demonstrably stale rather than merely suspected.

So an image self-reports, and `Image::parse` is the only thing that decides
what a file is -- no hash table to maintain, no filename to trust, and `info`
can always compare device against download. Stored files are named the
vendor's way, `FM-1_015.fwsc`, but that name is cosmetic; a misnamed file
still parses correctly.

This doubles as the integrity check. A truncated download is the realistic way
to brick a device with no recovery path, and it has to survive both the
`JLUFW` trailer in the last 16 bytes and a clean slot-table decode. The decode
is deliberately stricter than the vendor's: their state machine accepts a
table with no terminator and reports version 0, which is not a thing worth
being permissive about when the answer selects what gets written to flash.

## Transport

ALSA rawmidi, not the sequencer. The protocol is an `F0`..`F7` byte stream
either way and we own the framing regardless, so rawmidi just removes the
sequencer's event chunking from the middle of it. Refreshing the port list
across the device's mid-flash reboot is a plain re-enumeration -- this is the
part that makes running the vendor exe under Wine a dead end, since Wine's
winmm MIDI driver enumerates ALSA ports once at process start.

## The write path

The device drives, so the host is a server, and the whole decision for one
request is a pure function: `ota::answer(image, request) -> Answer`, returning
either the bytes to serve or which sentinel ended the phase. The I/O loop
around it is about fifteen lines. That split is what makes the interesting
part testable without hardware -- `a_simulated_device_can_reassemble_the_whole_image`
drives it with several thousand non-monotonic, repeated and odd-length reads
over a 700 KB image and asserts every byte came back from the right offset.

Out-of-range reads are an error, not a short or zero-filled reply. A device
asking for something the image cannot satisfy means the wrong file or a wrong
assumption, and neither is worth papering over halfway through a write.

Every check that can refuse runs before the first `ENTER_OTA`:

- the device's name must match the image's, so MK300 firmware cannot go onto
  an FM-1;
- same version is refused outright, as the vendor tool does;
- an older version needs `--allow-downgrade`;
- and confirmation is the literal word `flash`, not `y`, with `--yes` for
  scripts and an outright refusal when stdin is not a terminal.

## What has and has not been exercised

Verified against real hardware: port enumeration, the identify round trip, the
whole codec, and every one of the refusal paths above.

**Not verified: the write itself.** There is nothing to write -- the device
runs V15 and V15 is the only published image, so the same-version guard fires
first, which is the correct behaviour and also a dead end for testing. What
would exercise it is M-Vave publishing V16.

Two things in the sequence rest on inference from the vendor's logs rather
than observation, and are where to look first if a real flash misbehaves:

- **The two phases.** A verify pass ends at `0xE0000000` and a write pass at
  `0xF0000000`, with `ENTER_OTA` sent before each. The logs interleave two
  connections around the reboot, so exactly which handle receives the first
  sentinel is not certain; `ota::run` closes and re-opens between phases,
  which should be right either way but has not been watched happen.
- **Recovery from an abandoned attempt.** The vendor's logs show a device
  reconnecting as a normal `FM-1` at its old version after a failed attempt,
  which suggests entering OTA mode is not by itself a commitment. That is
  encouraging, not proof.

## Why read-only first

The board has no recovery buttons and no exposed debug pads, so an interrupted
or malformed OTA is likely the end of the device. A successful `info` against
real hardware validates magic, framing, checksum, bitstream packing, and the
transport in one shot, at zero risk. `flash` should not be written until that
has happened.

The pieces `flash` will need -- `Request`, the sentinel addresses, `SUCCESS`,
`ENTER_OTA` -- are already in `codec.rs` under `#[allow(dead_code)]`, covered
by tests, with no caller.
