{
  pkgs,
  nix-cachyos-kernel,
  isIso ? false,
}:
let
  nixos = import "${pkgs.path}/nixos" {
    configuration = {
      imports = [
        ./configuration.nix
      ]
      ++ pkgs.lib.optional isIso "${pkgs.path}/nixos/modules/installer/cd-dvd/iso-image.nix";
      nix.nixPath = [ "pkgs=${pkgs.path}" ];
      nixpkgs.overlays = [ nix-cachyos-kernel.overlays.default ];
    };
  };
  system = nixos.system;
in
if isIso then
  nixos.config.system.build.isoImage
else
  pkgs.writeShellScriptBin "apply" ''
    case "''${1:-switch}" in
      switch|boot) ;;
      *) echo "usage: apply [switch|boot]" >&2; exit 1 ;;
    esac
    sudo nix-env --profile /nix/var/nix/profiles/system --set ${system}
    sudo ${system}/bin/switch-to-configuration "''${1:-switch}"
  ''
