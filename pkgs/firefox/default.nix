{ pkgs }:
pkgs.wrapFirefox (pkgs.firefox-unwrapped.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [
    (pkgs.fetchpatch {
      name = "bug-2067504-vaapi-batch-renderer-usage.patch";
      url = "https://github.com/mozilla-firefox/firefox/commit/1fd7be717ddde9d2b9ed8e36b622762927d8a5fb.patch";
      hash = "sha256-u1MbG5Q1ILwTxwzLsViXcttBC+wS2hU8/VS05eL6ZaA=";
    })
    ./vaapi-pool-retire-unlinked.patch
  ];
})) { }
