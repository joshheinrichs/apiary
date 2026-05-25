{ pkgs }:

# Each attribute is a fetchFromGitHub derivation — the raw library source.
# The firmware build pre-populates .pio/libdeps/{env}/ from these and writes
# a synthetic .piopm so PlatformIO treats them as already installed offline.

{
  arduinoJson = pkgs.fetchFromGitHub {
    owner = "bblanchon";
    repo = "ArduinoJson";
    rev = "v7.4.3";
    hash = "sha256-Chv6nj0u6pjxS0BxNqJ9iigBXiuw9pi6xZCd/AT9AHE=";
  };

  asyncTcp = pkgs.fetchFromGitHub {
    owner = "ESP32Async";
    repo = "AsyncTCP";
    rev = "v3.4.9";
    hash = "sha256-2NvBknkxLVLDV4wEIVSVC4CrkhmGZ5c2VMpQ4hx2Wgc=";
  };

  espAsyncWebServer = pkgs.fetchFromGitHub {
    owner = "ESP32Async";
    repo = "ESPAsyncWebServer";
    rev = "v3.9.1";
    hash = "sha256-jgga/CuRn1FZEdYx083BEUNLStZ+sNMuwq4SLlJPVlk=";
  };

  nimBleArduino = pkgs.fetchFromGitHub {
    owner = "h2zero";
    repo = "NimBLE-Arduino";
    rev = "1.4.3";
    hash = "sha256-1JFMa45Bqf8LutL0tFgkwfE6YODOcmcULmuwQHspfEo=";
  };

  mqtt = pkgs.fetchFromGitHub {
    owner = "256dpi";
    repo = "arduino-mqtt";
    rev = "v2.5.3";
    hash = "sha256-d4goI7gHMG/ubv66XsrcKeoCN+xBGqgH+ijxYou5mEA=";
  };

  homeSpan = pkgs.fetchFromGitHub {
    owner = "HomeSpan";
    repo = "HomeSpan";
    rev = "1.9.1";
    hash = "sha256-DUiytXuROje9JAa2z93K+jQ6U0gbZAAfRPmESGyHlng=";
  };

  espArduinoBleScales = pkgs.fetchFromGitHub {
    owner = "gaggimate";
    repo = "esp-arduino-ble-scales";
    rev = "HEAD";
    hash = "sha256-h42MgjlTaZhoqm8dAdpIqNG3P/DDAD4fqvpIKE4C6to=";
  };

  psm = pkgs.fetchFromGitHub {
    owner = "gaggimate";
    repo = "PSM.Library";
    rev = "HEAD";
    hash = "sha256-SYKwIoh4/vbl0DguS6FDAK2GjwSb7rfIZrOPwbs1NX8=";
  };

  # Display-only libraries
  lvgl = pkgs.fetchFromGitHub {
    owner = "lvgl";
    repo = "lvgl";
    rev = "v8.4.0";
    hash = "sha256-9IrcWUUsem3so8trM+0odNWpuqVEdtkqXOfJsV9kFFM=";
  };

  tftEspi = pkgs.fetchFromGitHub {
    owner = "Bodmer";
    repo = "TFT_eSPI";
    rev = "V2.5.43";
    hash = "sha256-GzF9y+O18fWrg4nqdB1eJ/8vvChvrZ1q6P1scTewYeg=";
  };

  sensorLib = pkgs.fetchFromGitHub {
    owner = "lewisxhe";
    repo = "SensorsLib";
    rev = "v0.2.3";
    hash = "sha256-MxoOCrmJW6m/vuCtXyJpPYu1dKaiEN1gSdUxkULvowk=";
  };

  gfxLibrary = pkgs.fetchFromGitHub {
    owner = "moononournation";
    repo = "Arduino_GFX";
    rev = "v1.5.9";
    hash = "sha256-jKASG6ykAIM0fnFYr+RZpqiVek4H90+WJdew+KwAERI=";
  };

  # Transitive dependencies
  ads1x15 = pkgs.fetchFromGitHub {
    owner = "RobTillaart";
    repo = "ADS1X15";
    rev = "0.5.4";
    hash = "sha256-Ed8rRU46E65QlGOer/bIVhO7Q1wXQjlS0tR+iLXQkro=";
  };

  max31855 = pkgs.fetchFromGitHub {
    owner = "RobTillaart";
    repo = "MAX31855_RT";
    rev = "0.6.2";
    hash = "sha256-1pNInuYVxB7azmDETlwSDjrvAO/v0Q+q0HlW1WaANec=";
  };

  pca9634 = pkgs.fetchFromGitHub {
    owner = "RobTillaart";
    repo = "PCA9634";
    rev = "0.4.1";
    hash = "sha256-yo+sjq6G9EEDLd5Xz4UGoB8nskEZniVWc+/HAj7NvwE=";
  };

  vl53l0x = pkgs.fetchFromGitHub {
    owner = "pololu";
    repo = "vl53l0x-arduino";
    rev = "1.3.1";
    hash = "sha256-vKhDS0PDpPlB3OMb1I8zCHCNl4czlyiE5j6LgpfKQgM=";
  };

  pwFusionVL53L3C = pkgs.fetchFromGitHub {
    owner = "PlayingWithFusion";
    repo = "PWFusion_VL53L3C";
    rev = "HEAD";
    hash = "sha256-jlZUKPZAWB1RKCWhiioQY5YUT3zXtYRqSxEsvnKUnRE=";
  };
}
