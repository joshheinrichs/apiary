{
  pkgs,
  nix-cachyos-kernel,
  home-manager,
  apiary,
}:
rec {
  fuzzel-window-switcher = import ./fuzzel-window-switcher { inherit pkgs; };
  home-applicator = import ./home-applicator { inherit pkgs; };
  bubblewand = import ./bubblewand { inherit pkgs; };
  bubbled-spotify = import ./bubbled-spotify { inherit pkgs bubblewand; };
  bubbled-discord = import ./bubbled-discord { inherit pkgs bubblewand; };
  bubbled-syncthing = import ./bubbled-syncthing { inherit pkgs bubblewand; };
  scoper = import ./scoper { inherit pkgs; };
  blog = import ./blog { inherit pkgs; };
  desktop-system-applicator = import ./desktop-system-applicator {
    inherit pkgs nix-cachyos-kernel;
  };
  desktop-iso = import ./desktop-system-applicator {
    inherit pkgs nix-cachyos-kernel;
    isIso = true;
  };
  steam = import ./steam { inherit pkgs; };
  desktop-home = import ./desktop-home { inherit pkgs home-manager apiary; };
  desktop-home-applicator = pkgs.writeShellScriptBin "desktop-home-applicator" ''
    exec ${home-applicator}/bin/home-applicator ${desktop-home}/home-files ${desktop-home}/home-path
  '';
}
