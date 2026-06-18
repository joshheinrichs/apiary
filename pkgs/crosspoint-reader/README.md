# crosspoint-reader

Nix package for [CrossPoint Reader](https://github.com/crosspoint-reader/crosspoint-reader),
ESP32-C3 e-reader firmware. Built fully offline (see `INTENT.md` for how the
pioarduino platform is mirrored hermetically).

## Build

```bash
nix-build /path/to/apiary -A crosspoint-reader
```

Output in `result/firmware/`:

```
firmware.bin      # application (env: gh_release)
bootloader.bin
partitions.bin
```

There is no filesystem image: the web UI is embedded into the firmware at build
time, and the `spiffs` partition holds runtime user data (books) only.

## Flash

The board is an ESP32-C3 (16 MB flash). Put it in download mode, then:

```bash
esptool --chip esp32c3 --port /dev/ttyACM0 --baud 921600 write_flash \
  0x0000  result/firmware/bootloader.bin \
  0x8000  result/firmware/partitions.bin \
  0x10000 result/firmware/firmware.bin
```

`esptool` is available via `nix-shell -p esptool` if not already on PATH.

### Flash offsets reference

From `partitions.csv` (16 MB, dual-OTA):

| Partition | Offset | Size |
|-----------|--------|------|
| bootloader | `0x0000` | — |
| partitions | `0x8000` | — |
| nvs | `0x9000` | `0x5000` |
| otadata | `0xe000` | `0x2000` |
| app0 (ota_0) | `0x10000` | `0x640000` |
| app1 (ota_1) | `0x650000` | `0x640000` |
| spiffs | `0xc90000` | `0x360000` |
| coredump | `0xff0000` | `0x10000` |

After the initial flash the firmware self-updates over the air into the
alternate OTA slot.
