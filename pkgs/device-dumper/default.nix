{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "device-dumper";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [
    pkgs.pkg-config
    pkgs.rustPlatform.bindgenHook
  ];
  buildInputs = [ pkgs.libdisplay-info ];
}
