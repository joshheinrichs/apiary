{
  pkgs,
  home-manager,
  apiary,
}:
let
  hm = home-manager.homeManagerConfiguration {
    inherit pkgs;
    extraSpecialArgs = { inherit apiary; };
    modules = [ ./home.nix ];
  };
in
pkgs.runCommand "mainframe-home" { } ''
  ln -s ${hm.activationPackage}/home-files $out
''
