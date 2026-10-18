{ pkgs }:
pkgs.wrapFirefox (pkgs.firefox-unwrapped.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [
    ./vaapi-pool-retire-unlinked.patch
  ];
})) { }
