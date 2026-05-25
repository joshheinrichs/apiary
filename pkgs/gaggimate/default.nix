{ pkgs }:

let
  version = "1.8.1";

  src = pkgs.fetchFromGitHub {
    owner = "jniebuhr";
    repo = "gaggimate";
    rev = "v${version}";
    hash = "sha256-Fe8CzFPa1XB3teIExF1gRAotNdsJGFbdZErT7LvuVec=";
  };

  versionH = pkgs.writeText "version.h" ''
    #pragma once
    #ifndef GIT_VERSION_H
    #define GIT_VERSION_H
    #define BUILD_GIT_VERSION "v${version}"
    #define BUILD_TIMESTAMP "1970-01-01T00:00:00Z"
    #endif
  '';

  gaggimate-web = pkgs.buildNpmPackage {
    pname = "gaggimate-web";
    inherit version;
    src = "${src}/web";
    npmDepsHash = "sha256-A1bv0LB4Hd3gLjbQ/1LojRQk3ETzqD8cI9s2NUOO/6E=";

    buildPhase = ''
      node node_modules/vite/bin/vite.js build
    '';

    installPhase = ''
      mkdir -p $out
      cp -r dist/. $out/
      find $out/assets -name "*.js" -o -name "*.css" | xargs -r gzip -n -f
      find $out -maxdepth 1 -name "*.html" | xargs -r gzip -n -f
    '';
  };

  packages = import ./packages.nix { inherit pkgs; };
  libs     = import ./libraries.nix { inherit pkgs; };

  # Write a .piopm marker so PlatformIO treats a pre-populated libdeps entry
  # as already-installed and skips network resolution.
  libPiopm = name: version: owner:
    builtins.toJSON {
      type    = "library";
      inherit name version;
      spec    = { inherit owner name; id = null; requirements = null; uri = null; };
    };

  # Per-environment library sets matching what PlatformIO resolved in practice.
  # NayrodPID and GaggiMateController live in gaggimate/lib/ and are local.
  displayLibs = with libs; [
    { name = "AsyncTCP";              src = asyncTcp;          version = "3.4.9";  owner = "esp32async"; }
    { name = "ESPAsyncWebServer";     src = espAsyncWebServer; version = "3.9.1";  owner = "esp32async"; }
    { name = "ArduinoJson";           src = arduinoJson;       version = "7.4.3";  owner = "bblanchon"; }
    { name = "MQTT";                  src = mqtt;              version = "2.5.3";  owner = "256dpi"; }
    { name = "NimBLE-Arduino";        src = nimBleArduino;     version = "1.4.3";  owner = "h2zero"; }
    { name = "HomeSpan";              src = homeSpan;          version = "1.9.1";  owner = "homespan"; }
    { name = "esp-arduino-ble-scales"; src = espArduinoBleScales; version = "0.0.0"; owner = "gaggimate"; }
    { name = "ADS1X15";               src = ads1x15;           version = "0.5.4";  owner = "robtillaart"; }
    { name = "MAX31855";              src = max31855;          version = "0.6.2";  owner = "robtillaart"; }
    { name = "PCA9634";               src = pca9634;           version = "0.4.1";  owner = "robtillaart"; }
    { name = "PSM";                   src = psm;               version = "0.0.0";  owner = "gaggimate"; }
    { name = "PWFusion_VL53L3C";      src = pwFusionVL53L3C;   version = "0.0.0";  owner = "playingwithfusion"; }
    { name = "VL53L0X";               src = vl53l0x;           version = "1.3.1";  owner = "pololu"; }
  ];

  displayOnlyLibs = with libs; [
    { name = "lvgl";                     src = lvgl;       version = "8.4.0"; owner = "lvgl"; }
    { name = "TFT_eSPI";                 src = tftEspi;    version = "2.5.43"; owner = "bodmer"; }
    { name = "SensorLib";                src = sensorLib;  version = "0.2.3"; owner = "lewisxhe"; }
    { name = "GFX Library for Arduino";  src = gfxLibrary; version = "1.5.9"; owner = "moononournation"; }
  ];

  controllerLibs = with libs; [
    { name = "NimBLE-Arduino";  src = nimBleArduino; version = "1.4.3"; owner = "h2zero"; }
    { name = "ArduinoJson";     src = arduinoJson;   version = "7.4.3"; owner = "bblanchon"; }
    { name = "ADS1X15";         src = ads1x15;       version = "0.5.4"; owner = "robtillaart"; }
    { name = "MAX31855";        src = max31855;      version = "0.6.2"; owner = "robtillaart"; }
    { name = "PCA9634";         src = pca9634;       version = "0.4.1"; owner = "robtillaart"; }
    { name = "PSM";             src = psm;           version = "0.0.0"; owner = "gaggimate"; }
    { name = "PWFusion_VL53L3C"; src = pwFusionVL53L3C; version = "0.0.0"; owner = "playingwithfusion"; }
    { name = "VL53L0X";         src = vl53l0x;       version = "1.3.1"; owner = "pololu"; }
  ];

  # Shell fragment that installs a list of library descriptors into an env dir.
  installLibs = env: libList: pkgs.lib.concatMapStrings (l: ''
    cp -rL ${l.src} .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}
    chmod -R u+w .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}
    printf '%s' ${pkgs.lib.escapeShellArg (libPiopm l.name l.version l.owner)} \
      > .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}/.piopm
    if ! [ -f .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}/library.json ] && \
       ! [ -f .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}/library.properties ] && \
       ! [ -f .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}/module.json ]; then
      printf '%s' ${pkgs.lib.escapeShellArg (builtins.toJSON { name = l.name; version = l.version; })} \
        > .pio/libdeps/${env}/${pkgs.lib.escapeShellArg l.name}/library.json
    fi
  '') libList;

in pkgs.stdenv.mkDerivation {
  pname = "gaggimate";
  inherit version src;

  nativeBuildInputs = [ pkgs.platformio-core ];

  SOURCE_DATE_EPOCH = "0";

  postPatch = ''
    sed -i '/extra_scripts/d; /pre:scripts/d' platformio.ini
    cp ${versionH} src/version.h

    # Replace git-URL lib_deps with plain version specs so PlatformIO matches
    # them against our pre-populated .pio/libdeps without hitting the network.
    sed -i 's|https://github.com/ESP32Async/AsyncTCP.git#v3\.4\.9|ESP32Async/AsyncTCP@3.4.9|' platformio.ini
    sed -i 's|https://github.com/ESP32Async/ESPAsyncWebServer.git#v3\.9\.1|ESP32Async/ESPAsyncWebServer@3.9.1|' platformio.ini
    sed -i 's|https://github.com/gaggimate/esp-arduino-ble-scales|gaggimate/esp-arduino-ble-scales@0.0.0|' platformio.ini

    # Patch git-URL deps in local library manifests to version specs.
    sed -i 's|"https://github.com/gaggimate/PSM.Library.git"|">=0.0.0"|' lib/GaggiMateController/library.json
    sed -i 's|"https://github.com/PlayingWithFusion/PWFusion_VL53L3C"|">=0.0.0"|' lib/GaggiMateController/library.json
  '';

  buildPhase = ''
    export HOME=$TMPDIR/home
    mkdir -p $HOME
    export PLATFORMIO_CORE_DIR=$TMPDIR/pio

    # ---- Assemble PLATFORMIO_CORE_DIR ----------------------------------------
    mkdir -p $PLATFORMIO_CORE_DIR/packages
    mkdir -p $PLATFORMIO_CORE_DIR/platforms

    cp -rL ${packages.toolchain-xtensa} $PLATFORMIO_CORE_DIR/packages/toolchain-xtensa-esp32s3
    cp -rL ${packages.toolchain-riscv32} $PLATFORMIO_CORE_DIR/packages/toolchain-riscv32-esp
    cp -rL ${packages.framework}   $PLATFORMIO_CORE_DIR/packages/framework-arduinoespressif32
    cp -rL ${packages.tool-esptoolpy} $PLATFORMIO_CORE_DIR/packages/tool-esptoolpy
    cp -rL ${packages.tool-mkspiffs} $PLATFORMIO_CORE_DIR/packages/tool-mkspiffs
    cp -rL ${packages.tool-scons}  $PLATFORMIO_CORE_DIR/packages/tool-scons
    cp -rL ${packages.platform}    $PLATFORMIO_CORE_DIR/platforms/espressif32
    chmod -R u+w $PLATFORMIO_CORE_DIR

    # ---- Pre-populate library deps per environment ---------------------------
    for env in display display-headless display-headless-8m display-headless-4m controller; do
      mkdir -p .pio/libdeps/$env
    done

    ${installLibs "display"               displayLibs}
    ${installLibs "display"               displayOnlyLibs}
    ${installLibs "display-headless"      displayLibs}
    ${installLibs "display-headless-8m"   displayLibs}
    ${installLibs "display-headless-4m"   displayLibs}

    ${installLibs "controller" controllerLibs}

    # ---- Web UI data ---------------------------------------------------------
    mkdir -p data/w data/p
    cp -r ${gaggimate-web}/. data/w/

    # ---- Compile -------------------------------------------------------------
    platformio run --environment display
    platformio run --environment display-headless
    platformio run --environment controller
    platformio run --target buildfs --environment display
  '';

  installPhase = ''
    mkdir -p $out/firmware
    for env in display display-headless controller; do
      if [ -d .pio/build/$env ]; then
        mkdir -p $out/firmware/$env
        find .pio/build/$env -maxdepth 1 \( -name "*.bin" -o -name "*.elf" \) \
          -exec cp {} $out/firmware/$env/ \;
      fi
    done
    cp .pio/build/display/spiffs.bin $out/firmware/display/filesystem.bin
    cp .pio/build/display/spiffs.bin $out/firmware/display-headless/filesystem.bin
  '';
}
