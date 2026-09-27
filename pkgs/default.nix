{
  pkgs,
  nix-cachyos-kernel,
  home-manager,
  apiary,
}:
rec {
  anix = import ./anix { inherit pkgs; };
  apiary-fmt = import ./apiary-fmt { inherit pkgs; };
  fm1ctl = import ./fm1ctl { inherit pkgs; };
  fuzzel-window-switcher = import ./fuzzel-window-switcher { inherit pkgs; };
  home-applicator = import ./home-applicator { inherit pkgs; };
  seal = import ./seal { inherit pkgs; };
  sealed-spotify = import ./sealed-spotify { inherit pkgs seal; };
  sealed-discord = import ./sealed-discord { inherit pkgs seal; };
  sealed-syncthing = import ./sealed-syncthing { inherit pkgs seal; };
  wlroots = import ./wlroots { inherit pkgs; };
  sway = import ./sway { inherit pkgs wlroots; };
  seatmux = import ./seatmux { inherit pkgs; };
  scoper = import ./scoper { inherit pkgs; };
  dictate = import ./dictate { inherit pkgs; };
  device-dumper = import ./device-dumper { inherit pkgs; };
  mainframe-devices = import ./mainframe-devices;
  mainframe-system-applicator = import ./mainframe-system-applicator {
    inherit pkgs nix-cachyos-kernel apiary;
  };
  mainframe-iso = import ./mainframe-system-applicator {
    inherit pkgs nix-cachyos-kernel apiary;
    isIso = true;
  };
  gaggimate = import ./gaggimate { inherit pkgs; };
  kodi = import ./kodi { inherit pkgs; };
  winry315 = import ./winry315 { inherit pkgs; };
  winry315-applicator = winry315.applicator;
  winry315-daemon = winry315.daemon;
  crosspoint-reader = import ./crosspoint-reader { inherit pkgs; };
  slippi-dolphin = import ./slippi-dolphin { inherit pkgs; };
  steam = import ./steam { inherit pkgs; };
  mainframe-home = import ./mainframe-home { inherit pkgs home-manager apiary; };
  mainframe-home-applicator = pkgs.writeShellScriptBin "mainframe-home-applicator" ''
    exec ${home-applicator}/bin/home-applicator ${mainframe-home}
  '';
}
