{ pkgs, wlroots }:
pkgs.sway.override {
  sway-unwrapped = pkgs.sway-unwrapped.override { wlroots_0_20 = wlroots; };
}
