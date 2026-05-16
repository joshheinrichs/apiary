{ pkgs, bubblewand }:
let
  closure = pkgs.closureInfo {
    rootPaths = [ pkgs.xdg-utils ];
  };
in
pkgs.runCommand "bubbled-xdg-open"
  {
    nativeBuildInputs = [ bubblewand.generator ];
  }
  ''
    bubblewand-generator \
      --bin=xdg-open \
      --network \
      --dbus-talk=org.freedesktop.DBus \
      --dbus-talk=org.freedesktop.portal.Desktop \
      --dbus-talk=org.freedesktop.portal.Documents \
      --set-env=NIXOS_XDG_OPEN_USE_PORTAL=1 \
      --fwd-env=XDG_CURRENT_DESKTOP \
      "--ro-bind-file=${closure}/store-paths" \
      ${pkgs.xdg-utils} \
      $out
  ''
