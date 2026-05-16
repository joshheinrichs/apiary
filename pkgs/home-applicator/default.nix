{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "home-applicator";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
}
