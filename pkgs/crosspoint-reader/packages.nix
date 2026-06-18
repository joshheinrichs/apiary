{ pkgs }:

# The pioarduino platform-espressif32 55.x package set, reduced to exactly what
# an offline ESP32-C3 (RISC-V) arduino build needs. Versions/URLs are the ones
# PlatformIO resolved in a network capture build (see INTENT.md).
#
# Each entry is laid out so PlatformIO core treats it as already installed: the
# real source plus a synthetic `.piopm` (and `package.json` where the upstream
# archive lacks one). The firmware build copies these into PLATFORMIO_CORE_DIR.
# A package is matched as "installed" when its `.piopm` spec matches what the
# platform requests — by `uri` for URL-pinned packages, by owner/name otherwise.

let
  lib = pkgs.lib;
  py = pkgs.python3Packages;

  # The two native Python modules main.py imports at load time (filesystem image
  # builders). Not in nixpkgs; both bundle their C sources, so they build cleanly
  # from the PyPI sdist — no manylinux wheels, no source patches.
  littlefs-python = py.buildPythonPackage rec {
    pname = "littlefs-python";
    version = "0.18.0";
    pyproject = true;
    src = py.fetchPypi {
      pname = "littlefs_python";
      inherit version;
      hash = "sha256-qGev/QWzgbMV24J7EIUfjmTblMsw7sQIP0tEKs6yD/g=";
    };
    build-system = with py; [ setuptools setuptools-scm wheel cython ];
    SETUPTOOLS_SCM_PRETEND_VERSION = version;
    pythonImportsCheck = [ "littlefs" ];
  };

  fatfs-ng = py.buildPythonPackage rec {
    pname = "fatfs-ng";
    version = "0.1.15";
    pyproject = true;
    src = py.fetchPypi {
      pname = "fatfs_ng";
      inherit version;
      hash = "sha256-qwSMf3s8+IxVi6mHEWr8vYa6/bMUXcaGWhULUMaADAs=";
    };
    build-system = with py; [ setuptools cython ];
    pythonImportsCheck = [ "fatfs" ];
  };

  # Render PlatformIO's "installed package" marker.
  piopm = { type, name, version, specName ? name, owner ? null, id ? null, uri ? null }:
    builtins.toJSON {
      inherit type name version;
      spec = { inherit owner id uri; name = specName; requirements = null; };
    };

  pkgJson = name: version: builtins.toJSON { inherit name version; };

  # Unpack a release tar.xz into $out (stripping its single top dir) and drop a
  # synthetic .piopm. Done in one step so multi-GB trees aren't copied. pioarduino
  # requests these by URL, so the .piopm uri is the fetch url.
  unpackPiopm = { name, version, owner, url, hash }:
    pkgs.runCommand name { nativeBuildInputs = [ pkgs.gnutar pkgs.xz ]; } ''
      mkdir -p $out
      tar -xf ${pkgs.fetchurl { inherit url hash; }} -C $out --strip-components=1
      printf '%s' ${lib.escapeShellArg (piopm {
        type = "tool"; inherit name version owner; uri = url;
      })} > $out/.piopm
    '';

  # An empty package that exists only so PlatformIO sees it installed and skips
  # the download. The build never reads its contents.
  stubPackage = { name, version, uri, owner ? "pioarduino" }:
    pkgs.runCommand name {} ''
      mkdir -p $out
      printf '%s' ${lib.escapeShellArg (piopm {
        type = "tool"; inherit name version owner uri;
      })} > $out/.piopm
      printf '%s' ${lib.escapeShellArg (pkgJson name version)} > $out/package.json
    '';

in rec {
  # pioarduino platform source, placed at platforms/espressif32. One patch to
  # platform source: pioarduino force-reinstalls esptool into the penv from
  # tool-esptoolpy via `uv pip install` (a build-time network op, not gated on
  # connectivity). We provide esptool in the penv directly, so turn it off via
  # the `install_esptool` parameter pioarduino's own setup_penv_minimal exposes
  # for exactly this. The rest of its network bootstrap is skipped offline anyway.
  platform =
    let
      raw = pkgs.fetchzip {
        url = "https://github.com/pioarduino/platform-espressif32/releases/download/55.03.37/platform-espressif32.zip";
        hash = "sha256-7GgP+9qnv+nEM1C92S1TQ7WRD23wN+ZiaozNWyMCG4Q=";
        stripRoot = false;
      };
    in pkgs.runCommand "platform-espressif32" {} ''
      # The archive nests everything under a single platform-espressif32-*/ dir.
      cp -rL ${raw}/*/ $out
      chmod -R u+w $out
      substituteInPlace $out/platform.py \
        --replace-fail 'setup_penv_minimal(self, core_dir, install_esptool=True)' \
                       'setup_penv_minimal(self, core_dir, install_esptool=False)'
      printf '%s' ${lib.escapeShellArg (piopm {
        type = "platform"; name = "espressif32"; version = "55.3.37";
        specName = "platform-espressif32";
        uri = "https://github.com/pioarduino/platform-espressif32/releases/download/55.03.37/platform-espressif32.zip";
      })} > $out/.piopm
    '';

  framework = unpackPiopm {
    name = "framework-arduinoespressif32";
    version = "3.3.7";
    owner = "espressif";
    url = "https://github.com/espressif/arduino-esp32/releases/download/3.3.7/esp32-core-3.3.7.tar.xz";
    hash = "sha256-ndCbEa51uiWwYQ52/xJlpOWUQcxf3zyyDc0yOQSBQYY=";
  };

  frameworkLibs = unpackPiopm {
    name = "framework-arduinoespressif32-libs";
    version = "5.5.0+sha.87912cd291";
    owner = "espressif";
    url = "https://github.com/espressif/arduino-esp32/releases/download/3.3.7/esp32-core-3.3.7-libs.tar.xz";
    hash = "sha256-pn6Cxa9QHbMSYbN8rkzwJwucCKjXO2jYZ/glZp6FovY=";
  };

  # riscv32-esp-elf GCC 14.2.0. Espressif's prebuilt release is generic-linux
  # x86_64 host binaries; autoPatchelf them for the Nix sandbox (the same way
  # pkgs/gaggimate consumes its toolchains). The archive's own package.json
  # already carries the version, so we only add the .piopm.
  toolchain = pkgs.stdenv.mkDerivation {
    pname = "toolchain-riscv32-esp";
    version = "14.2.0+20251107";
    src = pkgs.fetchurl {
      url = "https://github.com/espressif/crosstool-NG/releases/download/esp-14.2.0_20251107/riscv32-esp-elf-14.2.0_20251107-x86_64-linux-gnu.tar.xz";
      hash = "sha256-HTobagZGhtm3fE23cx+C4mwHLjEuJ5acRf6WQQ7LJnE=";
    };
    # Archive unpacks to a riscv32-esp-elf/ top dir.
    sourceRoot = "riscv32-esp-elf";
    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = with pkgs; [ stdenv.cc.cc.lib glibc zlib ];
    # The bundled gdb wants libpython; we only use gcc/as/ld, so ignore it.
    autoPatchelfIgnoreMissingDeps = [ "libpython*" ];
    dontStrip = true;
    installPhase = ''
      mkdir -p $out
      cp -r . $out/
      printf '%s' ${lib.escapeShellArg (piopm {
        type = "tool"; name = "toolchain-riscv32-esp"; version = "14.2.0+20251107";
        owner = "pioarduino";
        uri = "https://github.com/pioarduino/registry/releases/download/0.0.1/riscv32-esp-elf-14.2.0_20251107.zip";
      })} > $out/.piopm
    '';
  };

  # Stubs: platform packages PlatformIO insists on resolving but a build never
  # reads — esptool runs from the penv binary, and idf_tools/piohome are unused.
  # With no tools/idf_tools.py, platform.py's `has_idf_tools` stays false so the
  # idf_tools install path is never taken.
  tool-esptoolpy = stubPackage {
    name = "tool-esptoolpy"; version = "5.1.2";
    uri = "https://github.com/pioarduino/registry/releases/download/0.0.1/esptoolpy-v5.1.2.zip";
  };

  tool-esp_install = stubPackage {
    name = "tool-esp_install"; version = "5.3.4";
    uri = "https://github.com/pioarduino/esp_install/releases/download/v5.3.4/esp_install-v5.3.4.zip";
  };

  contrib-piohome = stubPackage {
    name = "contrib-piohome"; version = "3.4.4";
    uri = "https://github.com/pioarduino/registry/releases/download/0.0.1/contrib-piohome-3.4.4.tar.gz";
  };

  # PlatformIO drives the build with SCons; provide it as the tool-scons package
  # (same approach as pkgs/gaggimate).
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
      printf '%s' ${lib.escapeShellArg (piopm {
        type = "tool"; name = "tool-scons"; version = "4.40801.0";
        owner = "platformio"; id = 8192;
      })} > $out/.piopm
      printf '%s' ${lib.escapeShellArg (pkgJson "tool-scons" "4.40801.0")} > $out/package.json
      cp ${launcher} $out/scons.py
    '';

  # Pre-built Python venv dropped at $PLATFORMIO_CORE_DIR/penv. penv_setup skips
  # its network bootstrap offline and uses this as-is. The build only needs a
  # python that can import littlefs/fatfs (which penv_setup.setup_python_paths
  # adds to the running python's sys.path) plus an esptool binary — both straight
  # from nixpkgs, which pins the exact versions the capture used (python 3.13.13,
  # esptool 5.3.0). esp-idf-size/esp-coredump are only reached by `pio run -t`.
  penv =
    let
      pyEnv = pkgs.python3.withPackages (ps: [ ps.certifi littlefs-python fatfs-ng ]);
      sitePackages = pkgs.python3.sitePackages;
    in pkgs.runCommand "crosspoint-penv" {} ''
      mkdir -p $out/bin "$(dirname "$out/${sitePackages}")"
      ln -s ${pyEnv}/bin/python3 $out/bin/python
      ln -s ${pyEnv}/bin/python3 $out/bin/python3
      ln -s ${pkgs.esptool}/bin/esptool $out/bin/esptool
      ln -s ${pkgs.esptool}/bin/esptool $out/bin/esptool.py
      ln -s ${pyEnv}/${sitePackages} $out/${sitePackages}
      cat > $out/pyvenv.cfg <<EOF
      home = ${pyEnv}/bin
      include-system-site-packages = false
      version = ${pkgs.python3.version}
      EOF
    '';
}
