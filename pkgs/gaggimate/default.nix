{ pkgs }:

let
  version = "1.8.1";

  src = pkgs.fetchFromGitHub {
    owner = "jniebuhr";
    repo  = "gaggimate";
    rev   = "v${version}";
    hash  = "sha256-Fe8CzFPa1XB3teIExF1gRAotNdsJGFbdZErT7LvuVec=";
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

  # Build a single library into a store path with .piopm (and library.json if
  # the source has no manifest), so PlatformIO treats it as already installed.
  makeLib = l:
    pkgs.runCommand (pkgs.lib.strings.sanitizeDerivationName l.name) {} ''
      cp -rL ${l.src} $out
      chmod -R u+w $out
      printf '%s' ${pkgs.lib.escapeShellArg (builtins.toJSON {
        type = "library";
        inherit (l) name version;
        spec = { inherit (l) owner name; id = null; requirements = null; uri = null; };
      })} > $out/.piopm
      if ! [ -f $out/library.json ] && \
         ! [ -f $out/library.properties ] && \
         ! [ -f $out/module.json ]; then
        printf '%s' ${pkgs.lib.escapeShellArg (builtins.toJSON { inherit (l) name version; })} \
          > $out/library.json
      fi
    '';

  # Produce a linkFarm whose entries match the .pio/libdeps/{env}/ layout.
  libdepsFor = libs: pkgs.linkFarm "gaggimate-libdeps"
    (map (l: { name = l.name; path = makeLib l; }) libs);

  # Pre-assembled PLATFORMIO_CORE_DIR layout; copied and chmod'd in buildPhase.
  pioCoreDir = pkgs.linkFarm "gaggimate-pio-core" [
    { name = "packages/toolchain-xtensa-esp32s3";     path = packages.toolchain-xtensa; }
    { name = "packages/toolchain-riscv32-esp";        path = packages.toolchain-riscv32; }
    { name = "packages/framework-arduinoespressif32"; path = packages.framework; }
    { name = "packages/tool-esptoolpy";               path = packages.tool-esptoolpy; }
    { name = "packages/tool-mkspiffs";                path = packages.tool-mkspiffs; }
    { name = "packages/tool-scons";                   path = packages.tool-scons; }
    { name = "platforms/espressif32";                 path = packages.platform; }
  ];

  displayLibs = with libs; [
    { name = "AsyncTCP";               src = asyncTcp;           version = "3.4.9";  owner = "esp32async";        }
    { name = "ESPAsyncWebServer";      src = espAsyncWebServer;  version = "3.9.1";  owner = "esp32async";        }
    { name = "ArduinoJson";            src = arduinoJson;        version = "7.4.3";  owner = "bblanchon";         }
    { name = "MQTT";                   src = mqtt;               version = "2.5.3";  owner = "256dpi";            }
    { name = "NimBLE-Arduino";         src = nimBleArduino;      version = "1.4.3";  owner = "h2zero";            }
    { name = "HomeSpan";               src = homeSpan;           version = "1.9.1";  owner = "homespan";          }
    { name = "esp-arduino-ble-scales"; src = espArduinoBleScales; version = "0.0.0"; owner = "gaggimate";         }
    { name = "ADS1X15";                src = ads1x15;            version = "0.5.4";  owner = "robtillaart";       }
    { name = "MAX31855";               src = max31855;           version = "0.6.2";  owner = "robtillaart";       }
    { name = "PCA9634";                src = pca9634;            version = "0.4.1";  owner = "robtillaart";       }
    { name = "PSM";                    src = psm;                version = "0.0.0";  owner = "gaggimate";         }
    { name = "PWFusion_VL53L3C";       src = pwFusionVL53L3C;    version = "0.0.0";  owner = "playingwithfusion"; }
    { name = "VL53L0X";                src = vl53l0x;            version = "1.3.1";  owner = "pololu";            }
  ];

  displayOnlyLibs = with libs; [
    { name = "lvgl";                    src = lvgl;       version = "8.4.0";  owner = "lvgl";            }
    { name = "TFT_eSPI";                src = tftEspi;    version = "2.5.43"; owner = "bodmer";          }
    { name = "SensorLib";               src = sensorLib;  version = "0.2.3";  owner = "lewisxhe";        }
    { name = "GFX Library for Arduino"; src = gfxLibrary; version = "1.5.9";  owner = "moononournation"; }
  ];

  controllerLibs = with libs; [
    { name = "NimBLE-Arduino";   src = nimBleArduino;   version = "1.4.3";  owner = "h2zero";            }
    { name = "ArduinoJson";      src = arduinoJson;     version = "7.4.3";  owner = "bblanchon";         }
    { name = "ADS1X15";          src = ads1x15;         version = "0.5.4";  owner = "robtillaart";       }
    { name = "MAX31855";         src = max31855;        version = "0.6.2";  owner = "robtillaart";       }
    { name = "PCA9634";          src = pca9634;         version = "0.4.1";  owner = "robtillaart";       }
    { name = "PSM";              src = psm;             version = "0.0.0";  owner = "gaggimate";         }
    { name = "PWFusion_VL53L3C"; src = pwFusionVL53L3C; version = "0.0.0"; owner = "playingwithfusion"; }
    { name = "VL53L0X";          src = vl53l0x;         version = "1.3.1";  owner = "pololu";            }
  ];

in pkgs.stdenv.mkDerivation {
  pname = "gaggimate";
  inherit version src;

  nativeBuildInputs = [ pkgs.platformio-core ];

  SOURCE_DATE_EPOCH = "0";

  postPatch = ''
    sed -i '/extra_scripts/d; /pre:scripts/d' platformio.ini
    cp ${versionH} src/version.h
    substituteInPlace platformio.ini \
      --replace-fail 'https://github.com/ESP32Async/AsyncTCP.git#v3.4.9' 'ESP32Async/AsyncTCP@3.4.9' \
      --replace-fail 'https://github.com/ESP32Async/ESPAsyncWebServer.git#v3.9.1' 'ESP32Async/ESPAsyncWebServer@3.9.1' \
      --replace-fail 'https://github.com/gaggimate/esp-arduino-ble-scales' 'gaggimate/esp-arduino-ble-scales@0.0.0'
    substituteInPlace lib/GaggiMateController/library.json \
      --replace-fail '"https://github.com/gaggimate/PSM.Library.git"' '">=0.0.0"' \
      --replace-fail '"https://github.com/PlayingWithFusion/PWFusion_VL53L3C"' '">=0.0.0"'
  '';

  buildPhase = ''
    export HOME=$TMPDIR/home
    mkdir -p $HOME
    export PLATFORMIO_CORE_DIR=$TMPDIR/pio

    cp -rL ${pioCoreDir} $PLATFORMIO_CORE_DIR
    chmod -R u+w $PLATFORMIO_CORE_DIR

    mkdir -p .pio/libdeps
    cp -rL ${libdepsFor (displayLibs ++ displayOnlyLibs)} .pio/libdeps/display
    cp -rL ${libdepsFor displayLibs} .pio/libdeps/display-headless
    cp -rL ${libdepsFor controllerLibs} .pio/libdeps/controller
    chmod -R u+w .pio/libdeps

    mkdir -p data/w data/p
    cp -r ${gaggimate-web}/. data/w/

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
    # display-headless shares the same web UI data partition
    cp .pio/build/display/spiffs.bin $out/firmware/display-headless/filesystem.bin
  '';
}
