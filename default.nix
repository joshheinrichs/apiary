let
  sources = import ./sources.nix;
  pkgs = import sources.nixpkgs-src {
    config.allowUnfreePredicate =
      pkg:
      builtins.elem (pkg.pname or pkg.name) [
        "spotify"
        "discord"
        "claude-code"
        "steam"
        "steam-original"
        "steam-run"
        "steam-unwrapped"
      ];
  };
  nix-cachyos-kernel = import "${sources.nix-cachyos-kernel-src}/default.nix";
  home-manager = import "${sources.home-manager-src}/lib" { lib = pkgs.lib; };
  apiary = import ./pkgs {
    inherit
    pkgs
      nix-cachyos-kernel
      home-manager
      apiary
      ;
  };
in
{
  inherit (apiary)
    desktop-system-applicator
    desktop-home
    desktop-home-applicator
    desktop-iso
    blog
    bubbled-syncthing
    ;
}
