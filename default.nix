{
  system ? builtins.currentSystem,
}:
let
  sources = import ./sources.nix;
  pkgs = import sources.nixpkgs-src {
    inherit system;
    config.allowUnfreePredicate =
      pkg:
      builtins.elem (pkg.pname or pkg.name) [
        "spotify"
        "discord"
        "discord-unwrapped"
        "claude-code"
        "steam"
        "steam-original"
        "steam-run"
        "steam-unwrapped"
        "jieli-toolchain"
      ];
  };
  nix-cachyos-kernel = import "${sources.nix-cachyos-kernel-src}/default.nix";
  home-manager = import "${sources.home-manager-src}/lib" { inherit (pkgs) lib; };
  lanzaboote = import sources.lanzaboote-src { inherit pkgs; };
  apiary = import ./pkgs {
    inherit
      pkgs
      nix-cachyos-kernel
      home-manager
      lanzaboote
      apiary
      ;
  };
in
{
  inherit (apiary)
    apiary-fmt
    mainframe-system-applicator
    mainframe-home
    mainframe-home-applicator
    mainframe-iso
    mainframe-motherboard-firmware
    firefox
    felucca
    blog
    sealed-syncthing
    gaggimate
    crosspoint-reader
    slippi-dolphin
    device-dumper
    btrmaps
    winry315-applicator
    ;
}
