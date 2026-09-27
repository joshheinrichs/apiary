{ pkgs }:
let
  common = {
    version = "0.1.0";
    src = ./.;
    cargoLock.lockFile = ./Cargo.lock;
  };
  runtime = pkgs.rustPlatform.buildRustPackage (
    common
    // {
      pname = "seal";
      cargoBuildFlags = [
        "--package"
        "seal"
      ];
      cargoTestFlags = [
        "--package"
        "seal"
      ];
      # Bake dependency paths into the binary at compile time
      BWRAP = "${pkgs.bubblewrap}/bin/bwrap";
      XDG_DBUS_PROXY = "${pkgs.xdg-dbus-proxy}/bin/xdg-dbus-proxy";
      PASTA = "${pkgs.passt}/bin/pasta";
      CAGE = "${pkgs.cage}/bin/cage";
      PIPEWIRE = "${pkgs.pipewire}/bin/pipewire";
      WIREPLUMBER = "${pkgs.wireplumber}/bin/wireplumber";
      WIREPLUMBER_SHARE = "${pkgs.wireplumber}/share";
      PIPEWIRE_SANDBOX_CONF = "${./pipewire-sandbox.conf}";
      PIPEWIRE_SANDBOX_CAPTURE_CONF = "${./pipewire-sandbox-capture.conf}";
    }
  );
in
{
  inherit runtime;
  generator = pkgs.rustPlatform.buildRustPackage (
    common
    // {
      pname = "seal-generator";
      cargoBuildFlags = [
        "--package"
        "seal-generator"
      ];
      cargoTestFlags = [
        "--package"
        "seal-generator"
      ];
      # Bake the runtime path into the generator at compile time
      SEAL = "${runtime}/bin/seal";
    }
  );
}
