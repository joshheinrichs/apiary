{ pkgs }:
pkgs.wlroots_0_20.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [ ./drm-lease-fd.patch ];
})
