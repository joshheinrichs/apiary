{
  pkgs,
  nix-cachyos-kernel,
  home-manager,
  apiary,
}:
rec {
  apis = import ./apis { inherit pkgs; };
  fm1ctl = import ./fm1ctl { inherit pkgs; };
  fuzzel-window-switcher = import ./fuzzel-window-switcher { inherit pkgs; };
  home-applicator = import ./home-applicator { inherit pkgs; };
  bubblewand = import ./bubblewand { inherit pkgs; };
  bubbled-spotify = import ./bubbled-spotify { inherit pkgs bubblewand; };
  bubbled-discord = import ./bubbled-discord { inherit pkgs bubblewand; };
  bubbled-syncthing = import ./bubbled-syncthing { inherit pkgs bubblewand; };
  wlroots = import ./wlroots { inherit pkgs; };
  sway = import ./sway { inherit pkgs wlroots; };
  seatmux = import ./seatmux { inherit pkgs; };
  scoper = import ./scoper { inherit pkgs; };
  mic-filter = import ./mic-filter { inherit pkgs; };
  dictate = import ./dictate { inherit pkgs; };
  device-dumper = import ./device-dumper { inherit pkgs; };
  desktop-devices = import ./desktop-devices;
  desktop-system-applicator = import ./desktop-system-applicator {
    inherit pkgs nix-cachyos-kernel apiary;
  };
  desktop-iso = import ./desktop-system-applicator {
    inherit pkgs nix-cachyos-kernel apiary;
    isIso = true;
  };
  gaggimate = import ./gaggimate { inherit pkgs; };
  winry315 = import ./winry315 { inherit pkgs; };
  winry315-applicator = winry315.applicator;
  winry315-daemon = winry315.daemon;
  crosspoint-reader = import ./crosspoint-reader { inherit pkgs; };
  slippi-dolphin = import ./slippi-dolphin { inherit pkgs; };
  steam = import ./steam { inherit pkgs; };
  desktop-home = import ./desktop-home { inherit pkgs home-manager apiary; };
  desktop-home-applicator = pkgs.writeShellScriptBin "desktop-home-applicator" ''
    exec ${home-applicator}/bin/home-applicator ${desktop-home}
  '';
}
