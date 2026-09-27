{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "apiary-fmt";
  version = "0.1.0";
  src = pkgs.lib.fileset.toSource {
    root = ./.;
    fileset = pkgs.lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  env = {
    GIT = "${pkgs.git}/bin/git";
    NIXFMT = "${pkgs.nixfmt}/bin/nixfmt";
    STATIX = "${pkgs.statix}/bin/statix";
    RUST_CARGO = "${pkgs.cargo}/bin/cargo";
    RUST_CARGO_FMT = "${pkgs.rustfmt}/bin/cargo-fmt";
    RUSTFMT = "${pkgs.rustfmt}/bin/rustfmt";
  };
}
