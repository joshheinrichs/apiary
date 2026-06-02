{ pkgs, bubblewand }:
let
  closure = pkgs.closureInfo {
    rootPaths = [ pkgs.spotify ];
  };
in
pkgs.runCommand "bubbled-spotify"
  {
    nativeBuildInputs = [ bubblewand.generator ];
  }
  ''
    bubblewand-generator install \
      --gui \
      --gpu-render \
      --cage \
      --pasta \
      --pasta-mac=02:00:00:00:00:00 \
      --dbus-own=org.mpris.MediaPlayer2.spotify \
      --dbus-talk=org.freedesktop.DBus \
      '--dbus-talk=org.freedesktop.portal.*' \
      --set-env=GIO_USE_PORTALS=1 \
      --set-env=NIXOS_OZONE_WL=1 \
      --persist-home=spotify \
      "--ro-bind-file=${closure}/store-paths" \
      ${pkgs.spotify} \
      $out
  ''
