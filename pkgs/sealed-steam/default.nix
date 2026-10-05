{ pkgs, seal }:
pkgs.runCommand "sealed-steam"
  {
    nativeBuildInputs = [ seal.generator ];
  }
  ''
    # /dev/input is the live host directory: controllers appear as they
    # connect, and keyboards/mice stay out because only the compositor may open
    # them. SDL sees seal's container marker and watches the directory, since
    # udev events never reach the sandbox netns.
    # --x11 shares the desktop's X server so Steam's windows (friends, chats,
    # games) tile in sway like any other app; X11 clients can see each other.
    # The whole store, read-only: the Vulkan drivers behind
    # /run/opengl-driver are host store paths outside any closure we build.
    seal-generator install \
      --bin=steam \
      --gui \
      --x11 \
      --gpu-render \
      --device=/dev/ntsync \
      --device=/dev/input \
      --net=internet \
      --new-session \
      --dbus-talk=org.freedesktop.DBus \
      '--dbus-talk=org.freedesktop.portal.*' \
      --dbus-talk=org.freedesktop.Notifications \
      --dbus-talk=org.kde.StatusNotifierWatcher \
      --persist-home=steam \
      --share-tmp=steam \
      --ro-bind=/nix/store:/nix/store \
      --ro-bind=${pkgs.pkgsi686Linux.mesa}:/run/opengl-driver-32 \
      --set-env=PATH=${pkgs.coreutils}/bin \
      --set-env=NIXOS_XDG_OPEN_USE_PORTAL=1 \
      ${pkgs.steam} \
      $out
  ''
