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
    # Wine builds GetLogicalProcessorInformation from /sys/devices/system/cpu;
    # without it MSVC's Concurrency Runtime throws (MapleStory's jypc.dll).
    # No 32-bit graphics drivers: 32-bit Windows games that crash need
    # PROTON_USE_WOW64=1 %command% in their launch options.
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
      '--dbus-own=com.steampowered.PressureVessel.LaunchAlongsideSteam.*' \
      --persist-home=steam \
      --share-tmp=steam \
      --ro-bind=/nix/store:/nix/store \
      --ro-bind=/sys/devices/system/cpu:/sys/devices/system/cpu \
      --set-env=PATH=${pkgs.coreutils}/bin \
      --set-env=NIXOS_XDG_OPEN_USE_PORTAL=1 \
      --set-env=STEAM_EXTRA_COMPAT_TOOLS_PATHS=${pkgs.proton-ge-bin.steamcompattool} \
      ${pkgs.steam} \
      $out
  ''
