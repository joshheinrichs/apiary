# gaggimate

Nix package for [GaggiMate](https://github.com/jniebuhr/gaggimate) ESP32-S3 espresso machine firmware.

## Build

```bash
nix-build /path/to/apiary -A gaggimate
```

Output in `result/firmware/`:

```
display/
  firmware.bin       # display board application
  bootloader.bin
  partitions.bin
  filesystem.bin     # SPIFFS web UI + profiles
  spiffs.bin         # same image, alternate name
display-headless/
  firmware.bin       # headless variant (no display drivers/UI)
  bootloader.bin
  partitions.bin
  filesystem.bin
controller/
  firmware.bin       # controller board application
  bootloader.bin
  partitions.bin
```

## Flash

Both boards are ESP32-S3. To enter download mode: hold **BOOT**, tap **RESET**, release **BOOT**.

`esptool` is available via `nix-shell -p esptool` if not already on PATH.

### Display board (LilyGo T-RGB — 16 MB flash)

```bash
esptool --chip esp32s3 --port /dev/ttyUSB0 --baud 921600 write_flash \
  0x0000   result/firmware/display/bootloader.bin \
  0x8000   result/firmware/display/partitions.bin \
  0x10000  result/firmware/display/firmware.bin \
  0xc90000 result/firmware/display/filesystem.bin
```

### Controller board (GaggiMate Controller — 8 MB flash)

```bash
esptool --chip esp32s3 --port /dev/ttyUSB0 --baud 921600 write_flash \
  0x0000  result/firmware/controller/bootloader.bin \
  0x8000  result/firmware/controller/partitions.bin \
  0x10000 result/firmware/controller/firmware.bin
```

The controller has no filesystem partition. It is updated by the display board over BLE after initial flash.

### Flash offsets reference

These come from the boards' partition tables (`default_16MB.csv` / `default_8MB.csv`):

| Partition | Display (16 MB) | Controller (8 MB) |
|-----------|-----------------|-------------------|
| bootloader | `0x0000` | `0x0000` |
| partitions | `0x8000` | `0x8000` |
| app (ota_0) | `0x10000` | `0x10000` |
| spiffs | `0xc90000` | `0x670000` (unused) |

## OTA

The stock firmware pulls updates directly from GitHub releases and compares semver tags. It cannot receive pushed updates — OTA is pull-only, initiated from the display's web UI.

The controller has no Wi-Fi; the display downloads `board-firmware.bin` from the release URL and pushes it to the controller over BLE.

To point at a self-hosted server instead of GitHub, use the fork at
`~/workspace/gaggimate` which adds an `otaUrl` setting. Set it to a base URL
(e.g. `http://192.168.1.10/firmware/`) serving:

- `version.txt` — version string, e.g. `v1.8.2`
- `display-firmware.bin`
- `display-filesystem.bin`
- `board-firmware.bin`

Leave `otaUrl` empty to fall back to GitHub.
