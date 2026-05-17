{
  pkgs,
  home-manager,
  apiary,
}:
let
  hm = home-manager.homeManagerConfiguration {
    pkgs = pkgs;
    extraSpecialArgs = { inherit apiary; };
    modules = [ ./home.nix ];
  };
in
pkgs.runCommand "desktop-home" { } ''
  ln -s ${hm.activationPackage}/home-files $out
''
