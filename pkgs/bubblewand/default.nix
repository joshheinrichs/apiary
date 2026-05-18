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
      pname = "bubblewand";
      cargoBuildFlags = [
        "--package"
        "bubblewand"
      ];
      cargoTestFlags = [
        "--package"
        "bubblewand"
      ];
      # Bake dependency paths into the binary at compile time
      BWRAP = "${pkgs.bubblewrap}/bin/bwrap";
      XDG_DBUS_PROXY = "${pkgs.xdg-dbus-proxy}/bin/xdg-dbus-proxy";
      PASTA = "${pkgs.passt}/bin/pasta";
      XWAYLAND_SATELLITE = "${pkgs.xwayland-satellite}/bin/xwayland-satellite";
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
      pname = "bubblewand-generator";
      cargoBuildFlags = [
        "--package"
        "bubblewand-generator"
      ];
      cargoTestFlags = [
        "--package"
        "bubblewand-generator"
      ];
      # Bake the runtime path into the generator at compile time
      BUBBLEWAND = "${runtime}/bin/bubblewand";
    }
  );
}
