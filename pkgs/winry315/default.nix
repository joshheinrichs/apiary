{ pkgs }:
let
  version = "0.1.0";
  keymap = "apiary";

  qmkSrc = pkgs.fetchFromGitHub {
    owner = "qmk";
    repo = "qmk_firmware";
    rev = "3f26a9232a2696a99fec54239a7e177da53a5105";
    hash = "sha256-HAlazuASS2P0k4iAOPcleeVaf/Z5bQ0VSFabgwX3+Bk=";
  };

  # lib/lufa is a submodule, which the GitHub source archive omits; the AVR
  # build is the only thing that needs it.
  lufaSrc = pkgs.fetchFromGitHub {
    owner = "qmk";
    repo = "lufa";
    rev = "549b97320d515bfca2f95c145a67bd13be968faa";
    hash = "sha256-BCaLSOn9ksj0+gYNdiTkZqrgKbGEbWNxnxrh3TOpsOY=";
  };

  firmware = pkgs.stdenv.mkDerivation {
    pname = "winry315-firmware";
    inherit version;
    src = qmkSrc;

    nativeBuildInputs = [
      pkgs.qmk
      pkgs.pkgsCross.avr.buildPackages.gcc
      pkgs.pkgsCross.avr.buildPackages.binutils
    ];

    postPatch = ''
      cp -r ${lufaSrc}/. lib/lufa/
      mkdir -p keyboards/winry/winry315/keymaps/${keymap}
      cp -r ${./firmware}/. keyboards/winry/winry315/keymaps/${keymap}/
    '';

    buildPhase = ''
      runHook preBuild
      export HOME=$TMPDIR
      make winry/winry315:${keymap}
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      install -Dm444 winry_winry315_${keymap}.hex $out/winry315.hex
      runHook postInstall
    '';
  };

  daemon = pkgs.rustPlatform.buildRustPackage {
    pname = "winry315";
    inherit version;
    # Only the crate, so editing the firmware doesn't rebuild the daemon.
    src = pkgs.lib.fileset.toSource {
      root = ./.;
      fileset = pkgs.lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./src
      ];
    };
    cargoLock.lockFile = ./Cargo.lock;
  };

  # Flashing is the stateful, privileged act of making the hardware match the
  # built firmware -- the same relationship desktop-home-applicator has to
  # desktop-home, hence the name.
  applicator = pkgs.writeShellScriptBin "winry315-applicator" ''
    set -eu
    dfu=${pkgs.dfu-programmer}/bin/dfu-programmer
    echo "Put the pad in bootloader mode first: hold the top-left key while" >&2
    echo "plugging it in, or hold top-left and press bottom-right." >&2
    "$dfu" atmega32u4 erase --force
    "$dfu" atmega32u4 flash ${firmware}/winry315.hex
    "$dfu" atmega32u4 launch
  '';
in
{
  inherit daemon firmware applicator;
}
