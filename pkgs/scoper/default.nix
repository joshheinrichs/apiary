{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "scoper";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  SYSTEMD_RUN = "${pkgs.systemd}/bin/systemd-run";
}
