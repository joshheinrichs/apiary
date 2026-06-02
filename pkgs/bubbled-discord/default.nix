{ pkgs, bubblewand }:
let
  closure = pkgs.closureInfo {
    rootPaths = [
      pkgs.discord
      pkgs.xdg-utils
      pkgs.coreutils
    ];
  };
in
pkgs.runCommand "bubbled-discord"
  {
    nativeBuildInputs = [ bubblewand.generator ];
  }
  ''
    # --share-tmp: electron singleton socket lives in /tmp
    bubblewand-generator install \
      --gui \
      --gpu-render \
      --wayland \
      --pasta \
      --pasta-mac=02:00:00:00:00:00 \
      --cage \
      --camera \
      --audio-capture \
      --new-session \
      --dbus-talk=org.freedesktop.DBus \
      '--dbus-talk=org.freedesktop.portal.*' \
      --set-env=DISPLAY=:0 \
      --set-env=NIXOS_XDG_OPEN_USE_PORTAL=1 \
      --set-env=NIXOS_OZONE_WL=1 \
      --persist-home=discord \
      --share-tmp=discord \
      "--set-env=PATH=${pkgs.coreutils}/bin:${pkgs.xdg-utils}/bin" \
      "--ro-bind-file=${closure}/store-paths" \
      ${pkgs.discord} \
      $out
  ''
