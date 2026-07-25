# device-dumper — intent

- Emit static hardware facts as JSON on stdout, for `pkgs/desktop-devices` to
  consume (e.g. the monitor identity that keys the sway `output` config in
  `desktop-home`).
- **Scope: monitors + audio + input devices + raw USB.** A **manifest of static
  hardware facts** — explicitly *not* live state or anything that changes
  frequently.
- **Append-only.** Read the prior record on stdin, merge by identity (monitors:
  make/model/serial; audio: USB serial; inputs: sway identifier; usb:
  idVendor:idProduct[:serial]), and never drop entries — a device stays in the
  record after it's unplugged. Output is
  deterministic (no timestamps) so it diffs cleanly and Nix can consume it.
- **EDID parsed with `libdisplay-info`** (the Wayland reference parser that
  wlroots/sway use). Two reasons it beats the pure-Rust crates:
  - Robust on real monitors, including extension blocks (HDR, colorimetry).
    `edid-rs` rejected the actual LG ("expected detailed timing block"); `edid`
    drags in nom 3 (2017).
  - Its high-level `di_info` returns the **PNP-expanded make** ("LG Electronics",
    not "GSM"), so the emitted identity matches sway's output identifier.
  - Needs the C lib: `pkg-config` + `bindgenHook` + `libdisplay-info`; crate
    feature `v0_3` tracks the nixpkgs lib version (0.3.0) and unlocks the
    HDR/color-primaries/colorimetry accessors.
- **Inputs from sysfs** (`/sys/class/input/event*`), no extra deps. Emit the
  `identifier` computed exactly as sway's `input_device_get_identifier` does
  (`vendor:product:sanitized-name`), so it can key `seat <name> attach` config.
  USB/Bluetooth buses only (attachable peripherals; ACPI buttons and ALSA jack
  nodes are noise), and physical devices only — empty `phys` means a virtual
  uinput node (e.g. Steam's virtual pads), which would churn the manifest.
- **Audio via `pw-dump`.** Shell out to `pw-dump` (store path baked in at build
  via a `PWDUMP` compile-time env — `env!("PWDUMP")`) and parse its JSON — same data the
  `pipewire` crate would give, far less code, and a static manifest needs no live
  PipeWire connection, so the crate's streaming/subscription buys nothing.
  Capture identity (USB serial / vendor / product / `device.name` / bus) + the
  available **profiles** (a fixed capability). Deliberately exclude live state:
  the active profile and the current sink/source nodes (node names are
  profile-dependent, so they'd churn).
- **Raw USB via `nusb`.** The whole-device USB identity
  (idVendor:idProduct[:serial]) for peripherals that need udev permission rules —
  notably the GameCube controller adapter (`057e:0337`), which Dolphin opens via
  libusb and so never surfaces as an input event node for `inputs` to catch.
  `nusb` (pure Rust, no libusb C dep) reads descriptors without opening the
  device, so no privileges needed. Exclude **hubs** (`bDeviceClass 09`, incl. the
  root/xHCI controllers) — pure infrastructure. Overlaps `inputs`/`audio` on
  purpose: this is the permissions axis, a different identity than sway seat ids
  or PipeWire profiles.
