{
  pkgs,
  nix-cachyos-kernel,
  apiary,
  isIso ? false,
}:
let
  nixos = import "${pkgs.path}/nixos" {
    # `system = null` keeps eval-config from defaulting to `builtins.currentSystem`
    # (unavailable under pure eval); the platform comes from the pkgs we were
    # handed, via nixpkgs.buildPlatform/hostPlatform below.
    system = null;
    # Make the apiary library available to the config modules (e.g.
    # configuration.nix reads desktop-devices for the GC-adapter udev rule).
    specialArgs = { inherit apiary; };
    configuration = {
      imports = [
        ./configuration.nix
      ]
      ++ pkgs.lib.optional isIso "${pkgs.path}/nixos/modules/installer/cd-dvd/iso-image.nix";
      nix.nixPath = [ "pkgs=${pkgs.path}" ];
      nixpkgs.overlays = [ nix-cachyos-kernel.overlays.default ];
      nixpkgs.buildPlatform = pkgs.stdenv.buildPlatform;
      nixpkgs.hostPlatform = pkgs.stdenv.hostPlatform;
    };
  };
  system = nixos.system;
in
if isIso then
  nixos.config.system.build.isoImage
else
  pkgs.writeShellScriptBin "apply" ''
    case "''${1:-boot}" in
      switch|boot) ;;
      *) echo "usage: apply [switch|boot]" >&2; exit 1 ;;
    esac
    sudo nix-env --profile /nix/var/nix/profiles/system --set ${system}
    sudo ${system}/bin/switch-to-configuration "''${1:-boot}"
  ''
