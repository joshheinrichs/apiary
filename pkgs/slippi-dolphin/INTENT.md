# slippi-dolphin

## Why

- Play Slippi (Melee online) without the Slippi Launcher. The launcher is just a
  manager — it downloads Dolphin, writes the play key, and browses replays. None
  of that is needed at play time.
- We want the emulator itself pinned and built declaratively, not an imperatively
  downloaded binary. The launcher's whole job (fetching/updating binaries,
  managing mutable state) is what Nix should own instead.

## What we want

- Build Slippi's **mainline** Dolphin fork (`project-slippi/dolphin`) from
  source — the netplay build. Not the legacy Ishiiruka fork.
- Not a wrapped upstream AppImage. The community derivations (ssbm-nix,
  lytedev/slippi-nix) only repack the prebuilt AppImage; we deliberately compile.
- Netplay first. Playback (replay viewer) is the same source with
  `-DSLIPPI_PLAYBACK=true`; add as a sibling if/when we want replays.

## Login (no launcher)

- "Logging in" = one file, `user.json` (`{ uid, playKey, connectCode,
  displayName }`), a long-lived per-account credential — no session/daemon.
- **Get it:** log in at <https://slippi.gg/online/enable> and download `user.json`.
- **Put it at** (this netplay build, non-portable Linux):

      ~/.config/slippi-dolphin/netplay-beta/Slippi/user.json

  i.e. `$XDG_CONFIG_HOME/slippi-dolphin/netplay-beta/Slippi/user.json`. The
  `netplay-beta` segment is Dolphin's `NETPLAY_USER_DIR` constant; the resolver
  (`UICommon.cpp`) prefers `portable.txt`, then `$DOLPHIN_EMU_USERPATH`, then
  `~/.dolphin-emu`, then this XDG default. Playback would use `playback-beta`.
- It's a secret → never in the Nix store. For now placed by hand; eventual home
  goal is a home-manager out-of-store symlink (in desktop-home), not this package.
- The ISO (Melee NTSC 1.02) is likewise user-provided data, out of scope.

## Build notes (the non-obvious parts)

- Fork tracks an older Dolphin master, so it links more system libs than current
  nixpkgs `dolphin-emu` (sfml, mbedtls, gtk2, webkit2gtk, soundtouch, portaudio,
  readline, libao). We override nixpkgs' derivation and extend its inputs.
- `Externals/SlippiRustExtensions` is a Rust workspace compiled by the CMake
  build via cargo. All deps are crates.io (no git). Cargo-vendored so the build
  is offline.
- Netplay flag from the fork's `build-linux.sh`: `-DSLIPPI_PLAYBACK=false`. (Its
  `-DLINUX_LOCAL_DEV=true` is for a portable/AppImage layout — we don't use it;
  we do a normal FHS install and patch the Sys path, see below.)
- **Sys path (the "Failed to init core" trap):** unlike upstream Dolphin, the
  fork's Linux `CreateSysDirectoryPath` ignores the compiled data dir and
  hardcodes `~/.config/SlippiOnline/Sys` — a mutable dir the launcher populates.
  With no launcher it's empty, so `Sys/GameSettings` loads nothing and Slippi's
  "only boot Melee" guard in `BootManager::BootCore` rejects the disc (GUI shows
  "Failed to init core"; nogui "Could not boot the specified file"). Fix:
  `sys-dir-store-path.patch` makes that branch use `DATA_DIR` (the store install,
  `$out/share/dolphin-emu/sys`), so the package is self-contained — no launcher,
  no mutable Sys.
- The Rust extensions build as `libslippi_rust_extensions.so` but `make install`
  doesn't ship it; postInstall installs it to `$out/lib` and postFixup rpaths it.
