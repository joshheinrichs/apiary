let
  sources = import ./sources.nix;
  nixpkgs = import sources.nixpkgs-src { };
in
nixpkgs.mkShell {
  packages = [
    nixpkgs.nixfmt-tree
    nixpkgs.statix
  ];
}
