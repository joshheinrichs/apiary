{ pkgs }:

# External libraries pulled from `lib_deps` (and one transitive dep). The
# firmware build pre-populates .pio/libdeps/gh_release/ from these and writes a
# synthetic .piopm so PlatformIO treats them as already installed offline.
#
# Versions/owners are the exact ones PlatformIO resolved in a network capture
# build (see INTENT.md). Most come straight from GitHub tags; WebSockets 2.7.3
# exists only on the PlatformIO registry (GitHub tops out at 2.7.2), so it is
# fetched from the registry artifact.

{
  arduinoJson = pkgs.fetchFromGitHub {
    owner = "bblanchon";
    repo = "ArduinoJson";
    rev = "v7.4.2";
    hash = "sha256-MGspSk9zWdPHMFXm+Oi4sibznHYE1eKXSqi6QHmjVCk=";
  };

  pngdec = pkgs.fetchFromGitHub {
    owner = "bitbank2";
    repo = "PNGdec";
    rev = "1.1.6";
    hash = "sha256-d304ofldI9mo+a6Z2H+nIAOuEgI3xt6rRnU9OSOu9Ik=";
  };

  qrcode = pkgs.fetchFromGitHub {
    owner = "ricmoo";
    repo = "QRCode";
    rev = "v0.0.1";
    hash = "sha256-p5SnViPL2ILLepbeyPFo6PoFpP9GlRV6gyrjMKmVy9A=";
  };

  # Transitive dependency (pulled in by the SDCardManager SDK lib).
  sdfat = pkgs.fetchFromGitHub {
    owner = "greiman";
    repo = "SdFat";
    rev = "2.3.1";
    hash = "sha256-S8IxioER5BHWql7OyKI1+5UP0/l10RPRGhFNYMJzjhg=";
  };

  # PlatformIO-registry-only release (no matching GitHub tag).
  websockets = pkgs.fetchzip {
    url = "https://dl.registry.platformio.org/download/links2004/library/WebSockets/2.7.3/WebSockets-2.7.3.tar.gz";
    hash = "sha256-jw1Ui40tV9ebrZ78EnmM2bZ1SQun9WHY8SEUiISjDjU=";
    stripRoot = false;
  };

  # Pinned by a git commit in lib_deps. CrossPoint patches this source with
  # scripts/jpegdec_patches/* — applied in default.nix before it lands in
  # libdeps (the upstream patch_jpegdec.py extra_script is dropped).
  jpegdec = pkgs.fetchFromGitHub {
    owner = "bitbank2";
    repo = "JPEGDEC";
    rev = "86282979224c8a32fd51e091ed5a35b0c699a52b";
    hash = "sha256-kNr3rLl1x8pAJk6QNaLWklN4AWixL5xM+kk/tcv/fgs=";
  };
}
