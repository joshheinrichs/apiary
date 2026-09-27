{ pkgs }:
pkgs.rustPlatform.buildRustPackage {
  pname = "apiary-fmt";
  version = "0.1.0";
  src = pkgs.lib.fileset.toSource {
    root = ./.;
    fileset = pkgs.lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  postPatch = ''
    substituteInPlace src/main.rs \
      --replace-fail '@git@' '${pkgs.git}/bin/git' \
      --replace-fail '@nixfmt@' '${pkgs.nixfmt}/bin/nixfmt' \
      --replace-fail '@statix@' '${pkgs.statix}/bin/statix' \
      --replace-fail '@cargo@' '${pkgs.cargo}/bin/cargo' \
      --replace-fail '@cargoFmt@' '${pkgs.rustfmt}/bin/cargo-fmt' \
      --replace-fail '@rustfmt@' '${pkgs.rustfmt}/bin/rustfmt'
  '';
}
