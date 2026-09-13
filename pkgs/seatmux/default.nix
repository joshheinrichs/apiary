{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "seatmux";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = [
    pkgs.seatd
    pkgs.libdrm
    pkgs.systemdLibs
  ];
  SYSTEMD_RUN = "${pkgs.systemd}/bin/systemd-run";
}
