{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "home-applicator";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = [ pkgs.systemd ];
  NIX_STORE_BIN = "${pkgs.nix}/bin/nix-store";
}
