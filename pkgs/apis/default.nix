{ pkgs }:
let
  # All nix-bindings-* crates come from one git rev, so they share one hash.
  bindingsHash = "sha256-Y994r/xOFxHdk7gsQLVsMkhCqZbsBdyLp5lJ7Ba2zhI=";
in
pkgs.rustPlatform.buildRustPackage {
  pname = "apis";
  version = "0.1.0";
  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "nix-bindings-bdwgc-sys-0.2.2" = bindingsHash;
      "nix-bindings-expr-0.2.2" = bindingsHash;
      "nix-bindings-expr-sys-0.2.2" = bindingsHash;
      "nix-bindings-store-0.2.2" = bindingsHash;
      "nix-bindings-store-sys-0.2.2" = bindingsHash;
      "nix-bindings-util-0.2.2" = bindingsHash;
      "nix-bindings-util-sys-0.2.2" = bindingsHash;
    };
  };

  # The -sys crates generate bindings with bindgen (needs libclang) and locate
  # the Nix C API + Boehm GC via pkg-config.
  nativeBuildInputs = with pkgs; [
    pkg-config
    rustPlatform.bindgenHook
  ];
  buildInputs = with pkgs; [
    nixVersions.latest
    boehmgc
  ];

  # Absolute paths to the tools apis shells out to, baked into the binary at
  # build time (read via option_env!), so no PATH wrapper is needed.
  env = {
    APIS_NIX = "${pkgs.nixVersions.latest}/bin/nix";
    APIS_NOM = "${pkgs.nix-output-monitor}/bin/nom";
    APIS_GIT = "${pkgs.git}/bin/git";
  };
}
