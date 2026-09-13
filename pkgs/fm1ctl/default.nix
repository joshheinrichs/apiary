{ pkgs }:
# Reads and writes M-Vave FM-1 firmware over USB MIDI. The protocol was
# recovered from the vendor's Windows updater; see DESIGN.md.
#
# The firmware itself is runtime data, not a build input: `fm1ctl firmware
# --fetch` downloads it from the vendor into XDG_STATE_HOME. Nothing here
# redistributes it.
pkgs.rustPlatform.buildRustPackage {
  pname = "fm1ctl";
  version = "0.1.0";
  # Excludes target/, which is far larger than the crate.
  src = pkgs.lib.fileset.toSource {
    root = ./.;
    fileset = pkgs.lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = [ pkgs.alsa-lib ];
}
