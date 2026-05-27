{ pkgs }:

let
  piopm = attrs: builtins.toJSON ({
    spec = { requirements = null; uri = null; } // attrs.spec;
  } // builtins.removeAttrs attrs [ "spec" ]);

  pkgJson = name: version: builtins.toJSON { inherit name version; };

  addPiopm = name: piopmAttrs: src: pkgs.runCommand name {} ''
    cp -rL ${src} $out
    chmod -R u+w $out
    printf '%s' ${pkgs.lib.escapeShellArg (piopm piopmAttrs)} > $out/.piopm
  '';

  # Both Espressif GCC toolchains are identical modulo pname, PIO registry id, and URL.
  makeGccToolchain = { pname, id, url, hash }:
    let version = "8.4.0+2021r2-patch5";
    in pkgs.stdenv.mkDerivation {
      inherit pname version;
      src = pkgs.fetchurl { inherit url hash; };
      nativeBuildInputs = [ pkgs.autoPatchelfHook ];
      buildInputs = with pkgs; [ stdenv.cc.cc.lib glibc zlib expat ];
      autoPatchelfIgnoreMissingDeps = [ "libpython2.7.so.1.0" ];
      dontStrip = true;
      installPhase = ''
        mkdir -p $out
        cp -r . $out/
        printf '%s' ${pkgs.lib.escapeShellArg (piopm {
          type = "tool"; name = pname; inherit version;
          spec = { owner = "espressif"; inherit id; name = pname; };
        })} > $out/.piopm
        printf '%s' ${pkgs.lib.escapeShellArg (pkgJson pname version)} > $out/package.json
      '';
    };

in rec {
  toolchain-xtensa = makeGccToolchain {
    pname = "toolchain-xtensa-esp32s3";
    id    = 12550;
    url   = "https://github.com/espressif/crosstool-NG/releases/download/esp-2021r2-patch5/xtensa-esp32s3-elf-gcc8_4_0-esp-2021r2-patch5-linux-amd64.tar.gz";
    hash  = "sha256-iqF6at8B76WxYoyKxXgGOkTSaulYHTlIa5IiOkHvJi8=";
  };

  toolchain-riscv32 = makeGccToolchain {
    pname = "toolchain-riscv32-esp";
    id    = 15395;
    url   = "https://github.com/espressif/crosstool-NG/releases/download/esp-2021r2-patch5/riscv32-esp-elf-gcc8_4_0-esp-2021r2-patch5-linux-amd64.tar.gz";
    hash  = "sha256-99c+X54t8+psqOLJXWym0j1rOP0QHqXTAS88s81Z858=";
  };

  framework = addPiopm "framework-arduinoespressif32"
    { type = "tool"; name = "framework-arduinoespressif32"; version = "3.20017.241212+sha.dcc1105b"; spec = { owner = "platformio"; id = 8070; name = "framework-arduinoespressif32"; }; }
    (pkgs.fetchFromGitHub {
      owner = "espressif";
      repo  = "arduino-esp32";
      rev   = "dcc1105b";
      hash  = "sha256-HcetEbuw0MPMrLrwEXG17PbiekhSKveX2DP/oo++k70=";
    });

  platform = addPiopm "espressif32-platform"
    { type = "platform"; name = "espressif32"; version = "6.12.0"; spec = { owner = "platformio"; id = 4; name = "espressif32"; }; }
    (pkgs.fetchFromGitHub {
      owner = "platformio";
      repo  = "platform-espressif32";
      rev   = "v6.12.0";
      hash  = "sha256-YwFp/DUfsmCEt8TuOps8evHYKkwJ0pTkl4h5i0/fOfk=";
    });

  tool-esptoolpy = pkgs.runCommand "tool-esptoolpy" {} ''
    mkdir -p $out
    printf '%s' ${pkgs.lib.escapeShellArg (piopm { type = "tool"; name = "tool-esptoolpy"; version = "2.40900.250804"; spec = { owner = "platformio"; id = 8161; name = "tool-esptoolpy"; }; })} > $out/.piopm
    printf '%s' ${pkgs.lib.escapeShellArg (pkgJson "tool-esptoolpy" "2.40900.250804")} > $out/package.json
    cp -rL ${pkgs.esptool}/lib/python*/site-packages/esptool $out/esptool
    chmod -R u+w $out/esptool
    ln -s ${pkgs.esptool}/bin/.esptool-wrapped $out/esptool.py
  '';

  tool-mkspiffs = pkgs.runCommand "tool-mkspiffs" {} ''
    mkdir -p $out/bin
    printf '%s' ${pkgs.lib.escapeShellArg (piopm { type = "uploader"; name = "tool-mkspiffs"; version = "2.230.0"; spec = { owner = "platformio"; id = 9222; name = "tool-mkspiffs"; }; })} > $out/.piopm
    printf '%s' ${pkgs.lib.escapeShellArg (pkgJson "tool-mkspiffs" "2.230.0")} > $out/package.json
    ln -s ${pkgs.mkspiffs}/bin/mkspiffs $out/bin/mkspiffs_espressif32_arduino
    ln -s ${pkgs.mkspiffs}/bin/mkspiffs $out/bin/mkspiffs
  '';

  tool-scons =
    let
      launcher = pkgs.writeText "scons.py" ''
        import sys, os
        sys.path = [os.path.join(os.path.dirname(os.path.abspath(__file__)), "scons")] + sys.path
        from SCons.Script.Main import main
        sys.exit(main())
      '';
    in pkgs.runCommand "tool-scons" {} ''
      mkdir -p $out/scons
      cp -rL ${pkgs.scons}/lib/python*/site-packages/SCons $out/scons/SCons
      printf '%s' ${pkgs.lib.escapeShellArg (piopm { type = "tool"; name = "tool-scons"; version = "4.40801.0"; spec = { owner = "platformio"; id = 8192; name = "tool-scons"; }; })} > $out/.piopm
      printf '%s' ${pkgs.lib.escapeShellArg (pkgJson "tool-scons" "4.40801.0")} > $out/package.json
      cp ${launcher} $out/scons.py
    '';
}
