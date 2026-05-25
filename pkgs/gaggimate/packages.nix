{ pkgs }:

let
  piopm = attrs: builtins.toJSON ({
    spec = { requirements = null; uri = null; } // attrs.spec;
  } // builtins.removeAttrs attrs [ "spec" ]);

  # Minimal package.json required by PlatformIO 6.x load_manifest() for TOOL packages.
  pkgJson = name: version: builtins.toJSON { inherit name version; };

  addPiopm = name: piopmAttrs: src: pkgs.runCommand name { } ''
    cp -rL ${src} $out
    chmod -R u+w $out
    echo '${piopm piopmAttrs}' > $out/.piopm
  '';

in rec {
  # ---------------------------------------------------------------------------
  # Binary toolchains — prebuilt by Espressif, autoPatchelf'd here so the
  # firmware build (a regular derivation) can reference stable nix store paths.
  # URLs: espressif/crosstool-NG GitHub releases, tag esp-2021r2-patch5.
  # ---------------------------------------------------------------------------
  toolchain-xtensa = pkgs.stdenv.mkDerivation {
    pname = "toolchain-xtensa-esp32s3";
    version = "8.4.0+2021r2-patch5";

    src = pkgs.fetchurl {
      url = "https://github.com/espressif/crosstool-NG/releases/download/esp-2021r2-patch5/xtensa-esp32s3-elf-gcc8_4_0-esp-2021r2-patch5-linux-amd64.tar.gz";
      hash = "sha256-iqF6at8B76WxYoyKxXgGOkTSaulYHTlIa5IiOkHvJi8=";
    };

    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = with pkgs; [ stdenv.cc.cc.lib glibc zlib expat ];
    autoPatchelfIgnoreMissingDeps = [ "libpython2.7.so.1.0" ];
    dontStrip = true;

    installPhase = ''
      mkdir -p $out
      cp -r . $out/
      echo '${piopm { type = "tool"; name = "toolchain-xtensa-esp32s3"; version = "8.4.0+2021r2-patch5"; spec = { owner = "espressif"; id = 12550; name = "toolchain-xtensa-esp32s3"; }; }}' > $out/.piopm
      echo '${pkgJson "toolchain-xtensa-esp32s3" "8.4.0+2021r2-patch5"}' > $out/package.json
    '';
  };

  toolchain-riscv32 = pkgs.stdenv.mkDerivation {
    pname = "toolchain-riscv32-esp";
    version = "8.4.0+2021r2-patch5";

    src = pkgs.fetchurl {
      url = "https://github.com/espressif/crosstool-NG/releases/download/esp-2021r2-patch5/riscv32-esp-elf-gcc8_4_0-esp-2021r2-patch5-linux-amd64.tar.gz";
      hash = "sha256-99c+X54t8+psqOLJXWym0j1rOP0QHqXTAS88s81Z858=";
    };

    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = with pkgs; [ stdenv.cc.cc.lib glibc zlib expat ];
    autoPatchelfIgnoreMissingDeps = [ "libpython2.7.so.1.0" ];
    dontStrip = true;

    installPhase = ''
      mkdir -p $out
      cp -r . $out/
      echo '${piopm { type = "tool"; name = "toolchain-riscv32-esp"; version = "8.4.0+2021r2-patch5"; spec = { owner = "espressif"; id = 15395; name = "toolchain-riscv32-esp"; }; }}' > $out/.piopm
      echo '${pkgJson "toolchain-riscv32-esp" "8.4.0+2021r2-patch5"}' > $out/package.json
    '';
  };

  # ---------------------------------------------------------------------------
  # Arduino framework — source from espressif/arduino-esp32.
  # PlatformIO version string encodes the commit: 3.20017.241212+sha.dcc1105b
  # ---------------------------------------------------------------------------
  framework = addPiopm "framework-arduinoespressif32"
    { type = "tool"; name = "framework-arduinoespressif32"; version = "3.20017.241212+sha.dcc1105b"; spec = { owner = "platformio"; id = 8070; name = "framework-arduinoespressif32"; }; }
    (pkgs.fetchFromGitHub {
      owner = "espressif";
      repo = "arduino-esp32";
      rev = "dcc1105b";
      hash = "sha256-HcetEbuw0MPMrLrwEXG17PbiekhSKveX2DP/oo++k70=";
    });

  # ---------------------------------------------------------------------------
  # Platform build scripts — platformio/platform-espressif32 @ v6.12.0
  # Goes into PLATFORMIO_CORE_DIR/platforms/espressif32/
  # ---------------------------------------------------------------------------
  platform = addPiopm "espressif32-platform"
    { type = "platform"; name = "espressif32"; version = "6.12.0"; spec = { owner = "platformio"; id = 4; name = "espressif32"; }; }
    (pkgs.fetchFromGitHub {
      owner = "platformio";
      repo = "platform-espressif32";
      rev = "v6.12.0";
      hash = "sha256-YwFp/DUfsmCEt8TuOps8evHYKkwJ0pTkl4h5i0/fOfk=";
    });

  # ---------------------------------------------------------------------------
  # tool-esptoolpy — used for upload / buildfs image generation.
  # Wrap nixpkgs esptool into the directory layout PlatformIO expects.
  # ---------------------------------------------------------------------------
  tool-esptoolpy = pkgs.runCommand "tool-esptoolpy" { } ''
    mkdir -p $out
    echo '${piopm { type = "tool"; name = "tool-esptoolpy"; version = "2.40900.250804"; spec = { owner = "platformio"; id = 8161; name = "tool-esptoolpy"; }; }}' > $out/.piopm
    echo '${pkgJson "tool-esptoolpy" "2.40900.250804"}' > $out/package.json
    # Bundle the esptool package so Python prefers the directory over esptool.py itself.
    cp -rL ${pkgs.esptool}/lib/python*/site-packages/esptool $out/esptool
    chmod -R u+w $out/esptool
    # Launcher: the wrapped script sets up all dependency site-packages then calls _main().
    ln -s ${pkgs.esptool}/bin/.esptool-wrapped $out/esptool.py
  '';

  # ---------------------------------------------------------------------------
  # tool-mkspiffs — creates SPIFFS filesystem images for the buildfs target.
  # The espressif32 platform calls the binary as mkspiffs_espressif32_arduino.
  # ---------------------------------------------------------------------------
  tool-mkspiffs = pkgs.runCommand "tool-mkspiffs" { } ''
    mkdir -p $out/bin
    echo '${piopm { type = "uploader"; name = "tool-mkspiffs"; version = "2.230.0"; spec = { owner = "platformio"; id = 9222; name = "tool-mkspiffs"; }; }}' > $out/.piopm
    echo '${pkgJson "tool-mkspiffs" "2.230.0"}' > $out/package.json
    ln -s ${pkgs.mkspiffs}/bin/mkspiffs $out/bin/mkspiffs_espressif32_arduino
    ln -s ${pkgs.mkspiffs}/bin/mkspiffs $out/bin/mkspiffs
  '';

  # ---------------------------------------------------------------------------
  # tool-scons — platformio-core already ships SCons in its Python env,
  # but the espressif32 platform script may look for it in packages/.
  # ---------------------------------------------------------------------------
  tool-scons = pkgs.runCommand "tool-scons" { } ''
    mkdir -p $out/scons
    cp -rL ${pkgs.scons}/lib/python*/site-packages/SCons $out/scons/SCons
    echo '${piopm { type = "tool"; name = "tool-scons"; version = "4.40801.0"; spec = { owner = "platformio"; id = 8192; name = "tool-scons"; }; }}' > $out/.piopm
    echo '${pkgJson "tool-scons" "4.40801.0"}' > $out/package.json
    cat > $out/scons.py <<'EOF'
import sys
import os
scripts_dir = os.path.dirname(os.path.abspath(__file__))
sys.path = [os.path.join(scripts_dir, "scons")] + sys.path
from SCons.Script.Main import main
sys.exit(main())
EOF
  '';
}
