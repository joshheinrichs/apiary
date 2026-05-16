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
  mkdir $out
  ln -s ${hm.activationPackage}/home-files $out/home-files
  ln -s ${hm.config.home.path} $out/home-path
''
