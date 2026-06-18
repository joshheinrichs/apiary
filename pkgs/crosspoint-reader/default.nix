{ pkgs }:

let
  version = "1.3.0";

  src = pkgs.fetchFromGitHub {
    owner = "crosspoint-reader";
    repo  = "crosspoint-reader";
    rev   = "2266060e84d1e46d725443efc8d4333eec991a47";
    hash  = "sha256-tWGvjqCqAdDIRpCCVbnCnx5D2OEoF2bxiaj/crHLO0c=";
  };

  # The open-x4-sdk submodule (community-sdk) provides the symlink:// libs
  # (BatteryMonitor, InputManager, EInkDisplay, SDCardManager). The GitHub
  # source archive omits submodules, so fetch and place it at open-x4-sdk/.
  sdk = pkgs.fetchFromGitHub {
    owner = "crosspoint-reader";
    repo  = "community-sdk";
    rev   = "26648d643a1c883ab2f71e1869d05fe2a0c9d498";
    hash  = "sha256-VfrxA5HZDzTgMK6TE4y7LHFSOOAEoW4crQFuEDvXlOg=";
  };

  packages = import ./packages.nix { inherit pkgs; };
  libs     = import ./libraries.nix { inherit pkgs; };

  # CrossPoint patches JPEGDEC for a decode bug. Upstream applies the patches at
  # build time with `git apply` against the libdep's .git tree (the
  # patch_jpegdec.py extra_script); offline there is no .git, so we apply them to
  # the source here and drop that script in postPatch.
  jpegdecPatched = pkgs.runCommand "JPEGDEC-patched" { nativeBuildInputs = [ pkgs.git ]; } ''
    cp -rL ${libs.jpegdec} $out
    chmod -R u+w $out
    cd $out
    for p in ${src}/scripts/jpegdec_patches/*.patch; do
      git apply -p1 "$p"
    done
  '';

  # Build one library into a store path with a synthetic .piopm so PlatformIO
  # treats it as already installed offline (same approach as pkgs/gaggimate).
  makeLib = l:
    pkgs.runCommand l.name {} ''
      cp -rL ${l.src} $out
      chmod -R u+w $out
      printf '%s' ${pkgs.lib.escapeShellArg (builtins.toJSON {
        type = "library";
        inherit (l) name version;
        spec = { inherit (l) owner name; id = null; requirements = null; uri = null; };
      })} > $out/.piopm
    '';

  externalLibs = [
    { name = "ArduinoJson"; src = libs.arduinoJson; version = "7.4.2"; owner = "bblanchon"; }
    { name = "PNGdec";      src = libs.pngdec;      version = "1.1.6"; owner = "bitbank2"; }
    { name = "QRCode";      src = libs.qrcode;      version = "0.0.1"; owner = "ricmoo";   }
    { name = "SdFat";       src = libs.sdfat;       version = "2.3.1"; owner = "greiman";  }
    { name = "WebSockets";  src = libs.websockets;  version = "2.7.3"; owner = "links2004"; }
    { name = "JPEGDEC";     src = jpegdecPatched;   version = "1.2.7"; owner = "bitbank2"; }
  ];

  # .pio/libdeps/gh_release/ layout.
  libdeps = pkgs.linkFarm "crosspoint-libdeps"
    (map (l: { name = l.name; path = makeLib l; }) externalLibs);

  # Pre-assembled PLATFORMIO_CORE_DIR layout (penv is placed separately).
  pioCoreDir = pkgs.linkFarm "crosspoint-pio-core" [
    { name = "packages/framework-arduinoespressif32";      path = packages.framework; }
    { name = "packages/framework-arduinoespressif32-libs"; path = packages.frameworkLibs; }
    { name = "packages/toolchain-riscv32-esp";             path = packages.toolchain; }
    { name = "packages/tool-esptoolpy";                    path = packages.tool-esptoolpy; }
    { name = "packages/tool-esp_install";                  path = packages.tool-esp_install; }
    { name = "packages/tool-scons";                        path = packages.tool-scons; }
    { name = "packages/contrib-piohome";                   path = packages.contrib-piohome; }
    { name = "platforms/espressif32";                      path = packages.platform; }
  ];

in pkgs.stdenv.mkDerivation {
  pname = "crosspoint-reader";
  inherit version src;

  nativeBuildInputs = [ pkgs.platformio-core ];

  SOURCE_DATE_EPOCH = "0";

  postPatch = ''
    # Drop the JPEGDEC git-apply script (patched into the source instead), the
    # optional local override config that does not exist in a clean checkout, and
    # the cppcheck static-analysis options (only used by `pio check`, which would
    # otherwise pull the cppcheck tool over the network).
    sed -i '/patch_jpegdec.py/d; /extra_configs/d; /check_tool/d; /check_flags/d; /check_skip_packages/d' platformio.ini
    # The JPEGDEC lib_deps entry is a git URL; we pre-place a patched copy, so
    # reference it by bare name to keep PlatformIO from cloning it.
    substituteInPlace platformio.ini \
      --replace-fail 'https://github.com/bitbank2/JPEGDEC.git#86282979224c8a32fd51e091ed5a35b0c699a52b' 'JPEGDEC'
  '';

  buildPhase = ''
    runHook preBuild

    export HOME=$TMPDIR/home
    mkdir -p $HOME
    export PLATFORMIO_CORE_DIR=$TMPDIR/pio

    cp -rL ${pioCoreDir} $PLATFORMIO_CORE_DIR
    chmod -R u+w $PLATFORMIO_CORE_DIR
    ln -s ${packages.penv} $PLATFORMIO_CORE_DIR/penv

    mkdir -p .pio/libdeps
    cp -rL ${libdeps} .pio/libdeps/gh_release
    chmod -R u+w .pio/libdeps

    rm -rf open-x4-sdk
    cp -rL ${sdk} open-x4-sdk
    chmod -R u+w open-x4-sdk

    platformio run --environment gh_release

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $out/firmware
    cp .pio/build/gh_release/firmware.bin   $out/firmware/
    cp .pio/build/gh_release/bootloader.bin $out/firmware/
    cp .pio/build/gh_release/partitions.bin $out/firmware/

    runHook postInstall
  '';

  meta = with pkgs.lib; {
    description = "CrossPoint Reader ESP32-C3 e-reader firmware";
    homepage = "https://github.com/crosspoint-reader/crosspoint-reader";
    license = licenses.gpl3Plus;
    platforms = platforms.linux;
  };
}
