{
  pkgs,
  lib,
  apiary,
  config,
  ...
}:

let
  # Sway's output identity string ("make model serial"); sway uses "Unknown"
  # when the EDID carries no serial.
  outputId = m: "${m.make} ${m.model} ${if m.serial == null then "Unknown" else m.serial}";

  deskMonitorLeft = apiary.mainframe-devices.monitorById "0x00025AF7";
  deskOutputLeft = outputId deskMonitorLeft;
  deskMonitorRight = apiary.mainframe-devices.monitorById "112NTFA27619";
  deskOutputRight = outputId deskMonitorRight;
  deskAudio = apiary.mainframe-devices.audioById "4397123E";
  deskAudioMode = "analog-surround-21";
  deskWidthLeft = (builtins.head deskMonitorLeft.detailed_timings).horiz_video;

  # The echo-cancelled mic. A bare node name: the source is a module in the
  # daemon now, so there is no package to ask for it.
  micSource = "mic-filter";
  dictate = apiary.dictate.override { target = micSource; };

  rtk-init =
    pkgs.runCommand "rtk-init"
      {
        nativeBuildInputs = [ pkgs.rtk ];
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p $HOME/.claude
        rtk init -g --auto-patch
        cp -r $HOME/.claude $out
      '';

  # --- seatmux: the TV as a second seat on the same GPU ----------------------
  tvMonitor = apiary.mainframe-devices.monitorByModel "55R617CA";
  tvOutput = outputId tvMonitor;
  # The couch inputs, matched by evdev name. Not vendor:product: the K400 Plus
  # sits on a Logitech Unifying receiver, and udev reports the receiver's USB id
  # for every device paired to it -- the MX Master on the desk included. The air
  # mouse splits into three evdev nodes, each needing its own name.
  tvInputIds = map (name: (apiary.mainframe-devices.inputByName name).name) [
    "Logitech K400 Plus"
    "ZhenYe Tech BLE Remote"
    "ZhenYe Tech BLE Remote Keyboard"
    "ZhenYe Tech BLE Remote Mouse"
  ];
  tomlList = xs: "[ ${lib.concatMapStringsSep ", " (x: ''"${x}"'') xs} ]";

  deskSink = "alsa_output.${lib.removePrefix "alsa_card." deskAudio.device_name}.${deskAudioMode}";
  # HDMI audio rides the dGPU's own PCI function, independent of the display
  # side being leased. The TV is the card's *fourth* HDMI output (hdmi-output-3,
  # ELD monitor_name 55R617CA); plain "hdmi-stereo" is output-0, the LG.
  tvAudioProfile = "output:hdmi-stereo-extra3";
  tvSink = "alsa_output.pci-0000_03_00.1.hdmi-stereo-extra3";

  # sources.xml is reconciled by the launcher on every start, so the library
  # root is declared here. Content type and scraper stay Kodi's: they live in
  # its database, not in any file we can build.
  moviesDir = "${config.home.homeDirectory}/Videos/Movies";
  kodi = apiary.kodi.override {
    videoSources = [
      {
        name = "Movies";
        path = "${moviesDir}/";
      }
    ];
    settings = {
      # Pick up new files on launch without blocking the UI on the scan.
      "videolibrary.updateonstartup" = "true";
      "videolibrary.backgroundupdate" = "true";
    };
  };

  # The TV compositor is a bare sway: no systemd session integration and no
  # environment import, both of which belong to the desk instance alone —
  # a second importer would overwrite its WAYLAND_DISPLAY and SWAYSOCK.
  # Kodi is therefore exec'd by sway directly rather than through scoper: a
  # systemd-run child of this seat would inherit neither WAYLAND_DISPLAY nor the
  # audio environment seatmux sets, and would land on the desk's speakers.
  tvSwayConfig = pkgs.writeText "sway-tv.conf" ''
    output "${tvOutput}" mode 3840x2160@60Hz position 0 0 scale 2

    set $mod Mod4
    bindsym $mod+Return exec ${launch} ${pkgs.foot}/bin/foot
    bindsym $mod+d exec ${pkgs.fuzzel}/bin/fuzzel
    bindsym $mod+Shift+q kill
    bindsym $mod+f fullscreen toggle
    bindsym $mod+k exec ${kodi}/bin/kodi

    for_window [app_id="Kodi"] fullscreen enable

    exec swaymsg 'workspace 1; layout tabbed'
  '';

  # Compositors are forked by seatmux directly: the lease fd is inherited across
  # exec, and systemd-run (what scoper uses) would spawn them from the user
  # manager in another process tree, where the fd cannot follow.
  seatmuxConfig = pkgs.writeText "seatmux.toml" ''
    [[seat]]
    name = "desk"
    connectors = [ "${deskMonitorLeft.connector}", "${deskMonitorRight.connector}" ]
    exclude = ${tomlList tvInputIds}
    # No sink: the desk sink already wins the default by priority, so it inherits.
    # The source is still declared, because the rule that makes mic-filter the
    # default only applies once WirePlumber reloads, and nothing in the apply
    # path makes it. Drop this once that is no longer true.
    source = "${micSource}"
    command = [ "${apiary.sway}/bin/sway", "-d" ]

    [seat.env]
    TZ = "America/Regina"
    GTK_THEME = "Adwaita:dark"
    EDITOR = "${pkgs.neovim}/bin/nvim"

    [[seat]]
    name = "tv"
    connectors = [ "${tvMonitor.connector}" ]
    include = ${tomlList tvInputIds}
    sink = "${tvSink}"
    command = [ "${apiary.sway}/bin/sway", "-d", "-c", "${tvSwayConfig}" ]

    [seat.env]
    TZ = "America/Regina"
    GTK_THEME = "Adwaita:dark"
    EDITOR = "${pkgs.neovim}/bin/nvim"
  '';

  # The app launch pipeline: place apps in the apps slice.
  launch = "${apiary.scoper}/bin/scoper --slice=apps --";

in
{
  # Home Manager needs a bit of information about you and the paths it should
  # manage.
  home.username = "josh";
  home.homeDirectory = "/home/josh";

  # This value determines the Home Manager release that your configuration is
  # compatible with. This helps avoid breakage when a new Home Manager release
  # introduces backwards incompatible changes.
  #
  # You should not change this value, even if you update Home Manager. If you do
  # want to update the value, then make sure to first check the Home Manager
  # release notes.
  home.stateVersion = "26.05";

  # The home.packages option allows you to install Nix packages into your
  # environment.
  home.packages = with pkgs; [
    apiary.apis
    apiary.steam
    keepassxc
    libsecret
    gcr_4
    qbittorrent
    vlc
    obs-studio
    apiary.seal.runtime
    apiary.seal.generator
    apiary.sealed-spotify
    apiary.sealed-discord
    apiary.btrmaps
    ripgrep
    jq
    bat
    neovim
    zed-editor
    fzf
    btop
    fuzzel
    dejavu_fonts # foot
    font-awesome # waybar
    dust
    zoekt
    xdg-utils
    fd
    sd
    lurk
    samply
    isd
    # opencode
    gnome-themes-extra # dark theme
    nautilus
    pavucontrol
    shotman
    wl-clipboard
    pavucontrol
    landrun
    bubblewrap
    rtk
    (pkgs.writeShellScriptBin "seatmux" ''
      # Only `start` gets the config, the logind backend and the journal. Every
      # other word -- status, stop, or a typo -- goes straight through, so its
      # answer lands on the terminal and a bare `seatmux` just prints usage.
      if [ "''${1-}" != start ]; then
        exec ${apiary.seatmux}/bin/seatmux "$@"
      fi
      shift

      # Force the logind backend: falling through to libseat's builtin would open
      # devices directly, and there are no uaccess ACLs on /dev/input, so every
      # seat would come up with no keyboard or mouse.
      export LIBSEAT_BACKEND=logind
      # The VT goes dark and loses keyboard the moment the session is taken, so
      # stderr on the console is unreadable. Send everything to the journal:
      #   journalctl -t seatmux -b
      exec ${pkgs.systemd}/bin/systemd-cat -t seatmux --stderr-priority=warning \
        ${apiary.seatmux}/bin/seatmux start ${seatmuxConfig} "$@"
    '')
    (pkgs.writeShellScriptBin "wm" ''
      export TZ="America/Regina"
      export GTK_THEME="Adwaita:dark"
      export EDITOR="${pkgs.neovim}/bin/nvim"
      exec ${apiary.scoper}/bin/scoper \
        --slice=session-$(${pkgs.systemd}/bin/systemd-escape "$XDG_SESSION_ID") \
        --name=sway \
        -- ${pkgs.sway}/bin/sway "$@"
    '')
    dexed
  ];

  # Home Manager is pretty good at managing dotfiles. The primary way to manage
  # plain files is through 'home.file'.
  home.file = {
    # # Building this configuration will create a copy of 'dotfiles/screenrc' in
    # # the Nix store. Activating the configuration will then make '~/.screenrc' a
    # # symlink to the Nix store copy.
    # ".screenrc".source = dotfiles/screenrc;

    # # You can also set the file content immediately.
    # ".gradle/gradle.properties".text = ''
    #   org.gradle.console=verbose
    #   org.gradle.daemon.idletimeout=3600000
    # '';

    ".nix-profile".source = config.home.path;

    # Kodi's library root: the source is declared, so the directory has to exist.
    "Videos/Movies/.keep".text = "";
  };

  home.language.base = "en_CA.UTF-8";

  systemd.user.sessionVariables.SHELL = "${pkgs.fish}/bin/fish";

  wayland.windowManager.sway = {
    enable = true;
    xwayland = true;
    systemd.enable = true;
    wrapperFeatures.gtk = true; # Fixes common issues with GTK 3 apps
    config = rec {
      modifier = "Mod4";
      # bars = [
      #   { command = "${pkgs.waybar}/bin/waybar"; }
      # ];
      menu = "fuzzel";
      terminal = "${launch} ${pkgs.foot}/bin/foot";
      output = {
        "${deskOutputLeft}".position = "0 0";
        "${deskOutputRight}" = {
          scale = "1.5";
          position = "${toString deskWidthLeft} 0";
        };
      };
      keybindings = lib.mkOptionDefault {
        "${modifier}+space" = "exec ${apiary.fuzzel-window-switcher}/bin/fuzzel-window-switcher";
        "Print" = "exec shotman --capture output";
        "Alt+Print" = "exec shotman --capture region";
        # Hold-to-talk dictation: hold, speak, release; text is typed into
        # the focused window.
        "--no-repeat ${modifier}+m" = "exec ${dictate}/bin/dictate start";
        "--release ${modifier}+m" = "exec ${dictate}/bin/dictate stop";
      };
      startup = [
        { command = "swaymsg 'workspace 1; layout tabbed'"; }
      ];
    };
    # TODO: why doesn't systemd.enable do this?
    # https://github.com/NixOS/nixpkgs/issues/189851
    extraConfig = ''
      exec systemctl --user import-environment PATH DISPLAY WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP TZ GTK_THEME EDITOR
    '';
  };

  # Single source of truth for dark mode: xdg-desktop-portal-gtk reads this
  # and reports it as org.freedesktop.appearance color-scheme, so all
  # portal-aware apps (Firefox included) go dark.
  #
  # The dconf user database is a build product, not `dconf load`ed at
  # activation (home-manager's dconf.settings needs the activation script,
  # which home-applicator deliberately never runs). Settings written by apps
  # at runtime last only until the next apply replaces the database.
  xdg.configFile."dconf/user".source =
    pkgs.runCommand "dconf-user-db"
      {
        nativeBuildInputs = [ pkgs.dconf ];
      }
      ''
        dconf compile $out ${pkgs.writeTextDir "00-mainframe-home" ''
          [org/gnome/desktop/interface]
          color-scheme='prefer-dark'
        ''}
      '';

  # GSettings only consults dconf in processes that load the dconf GIO
  # module; scope it to the portal (the one reader we need) instead of
  # setting it session-wide.
  xdg.configFile."systemd/user/xdg-desktop-portal-gtk.service.d/dconf.conf".text = ''
    [Service]
    Environment=GIO_EXTRA_MODULES=${pkgs.dconf.lib}/lib/gio/modules
  '';

  # Virtual source "Microphone": the desk mic with speaker bleed subtracted
  # against what the desk sink is playing. A module in the daemon rather than a
  # standalone process -- it is pipewire's own code, so there is nothing to
  # sandbox, and hosting it here means the node exists as soon as the graph
  # does. The cost is that retuning it needs a daemon restart.
  #
  # monitor.mode taps the sink's monitor ports instead of publishing a virtual
  # sink, so nothing about playback routing changes. target.object on the sink
  # stream pins which sink is the reference rather than following the default.
  #
  # Mono output: the card captures 3-channel surround and everything downstream
  # (the pitch chain, soundboard-mic) is mono, as the old filter chain's source
  # was. The rate is pinned to the card's so the reference and the capture never
  # end up on opposite sides of a resampler.
  #
  # No aec.args: the canceller is enabled unconditionally, so nothing here has
  # to turn it on, and every other WebRTC stage is left at its default. The one
  # stage that defaults off is gain_control -- enabling it amplified the
  # cancellation residual until it was audible. Leave it alone.
  xdg.configFile."pipewire/pipewire.conf.d/51-echo-cancel.conf" = {
    text = ''
      context.modules = [
        {
          name = libpipewire-module-echo-cancel
          # nofail, because a context.modules entry is mandatory by default and
          # a module that cannot connect takes the whole daemon down with it
          # ("could not load mandatory module" -> "failed to create context").
          # A missing mic is survivable; a dead pipewire takes every seat's audio.
          flags = [ nofail ]
          args = {
            monitor.mode = true
            audio.rate = 48000
            audio.channels = 1
            audio.position = [ MONO ]
            capture.props = {
              node.name = "capture.${micSource}"
              node.passive = true
            }
            source.props = {
              node.name = "${micSource}"
              node.description = "Microphone"
            }
            sink.props = {
              node.name = "${micSource}-reference"
              node.passive = true
              target.object = "${deskSink}"
            }
          }
        }
      ]
    '';
    # The daemon reads pipewire.conf.d only at startup, so a changed drop-in
    # means nothing until it restarts. PartOf carries that restart on to
    # pipewire-pulse, wireplumber and soundboard-mic, so this one line is the
    # whole stack. Runs only when the rendered file actually differs.
    onChange = ''
      ${pkgs.systemd}/bin/systemctl --user restart pipewire.service || true
    '';
  };

  # set default sink/source via priority (soft default). the hard default
  # (wpctl set-default) is stateful, so it'd belong in home-applicator, not here.
  xdg.configFile."wireplumber/wireplumber.conf.d/51-desk-audio.conf".text = ''
    monitor.alsa.rules = [
      {
        matches = [
          { device.name = "${deskAudio.device_name}" }
        ]
        actions = {
          update-props = {
            device.profile = "output:${deskAudioMode}+input:${deskAudioMode}"
          }
        }
      }
      # The TV hangs off the dGPU's fourth HDMI output, so pin that profile:
      # the sink only exists while its profile is active, and the card's default
      # ("hdmi-stereo") is output 0, which is the LG on DisplayPort.
      {
        matches = [
          { device.name = "alsa_card.pci-0000_03_00.1" }
        ]
        actions = {
          update-props = {
            device.profile = "${tvAudioProfile}"
          }
        }
      }
      {
        matches = [
          { api.alsa.card.name = "${deskAudio.product}" }
        ]
        actions = {
          update-props = {
            priority.session = 10000
          }
        }
      }
    ]
    node.rules = [
      {
        matches = [
          { node.name = "capture.${micSource}" }
        ]
        actions = {
          update-props = {
            target.object = "alsa_input.${lib.removePrefix "alsa_card." deskAudio.device_name}.${deskAudioMode}"
            node.dont-fallback = true
          }
        }
      }
      # The desk mic *is* mic-filter, so it has to outrank the raw input that the
      # card rule boosts to 10000. Otherwise the desk default source is the
      # unfiltered hardware and only PulseAudio clients ever reach the filter.
      {
        matches = [
          { node.name = "${micSource}" }
        ]
        actions = {
          update-props = {
            priority.session = 20000
          }
        }
      }
    ]
  '';

  xdg.portal = {
    enable = true;
    xdgOpenUsePortal = true;
    extraPortals = with pkgs; [
      xdg-desktop-portal-wlr
      # for org.freedesktop.portal.OpenURI
      xdg-desktop-portal-gtk
    ];
    config.common.default = [ "*" ];
  };
  xdg.mimeApps = {
    enable = true;
    defaultApplications = {
      "text/html" = "firefox.desktop";
      "x-scheme-handler/http" = "firefox.desktop";
      "x-scheme-handler/https" = "firefox.desktop";
    };
  };

  systemd.user.services = {
    pipewire = {
      Unit = {
        After = [ "dbus.service" ];
        BindsTo = [ "dbus.service" ];
      };
      Service = {
        ExecStart = "${pkgs.pipewire}/bin/pipewire";
        Restart = "on-failure";
      };
      Install = {
        WantedBy = [ "default.target" ];
      };
    };

    pipewire-pulse = {
      Unit = {
        After = [
          "pipewire.service"
          "dbus.service"
        ];
        Requires = [ "pipewire.service" ];
        PartOf = [ "pipewire.service" ];
        BindsTo = [ "dbus.service" ];
      };
      Service = {
        ExecStart = "${pkgs.pipewire}/bin/pipewire-pulse";
        Restart = "on-failure";
      };
      Install = {
        WantedBy = [ "default.target" ];
      };
    };

    wireplumber = {
      Unit = {
        After = [ "pipewire.service" ];
        Requires = [ "pipewire.service" ];
        PartOf = [ "pipewire.service" ];
      };
      Service = {
        ExecStart = "${pkgs.wireplumber}/bin/wireplumber";
        Restart = "on-failure";
      };
      Install = {
        WantedBy = [ "default.target" ];
      };
    };

    sealed-syncthing =
      let
        pkg = apiary.sealed-syncthing.override {
          extraArgs = [
            "--pasta-tcp=127.0.0.1/8384"
            "--rw-bind=/home/josh/syncthing:/home/josh/syncthing"
          ];
        };
      in
      {
        Unit.Description = "Sandboxed Syncthing (seal + pasta)";
        Service = {
          ExecStart = "${pkg}/bin/syncthing --no-browser";
          Restart = "on-failure";
          RestartSec = 5;
        };
        Install.WantedBy = [ "default.target" ];
      };
  };

  nix = {
    package = pkgs.nix;
    nixPath = [ "nixpkgs=${pkgs.path}" ];
    settings = {
      extra-experimental-features = "nix-command";
    };
  };

  # Replaced by sandboxed sealed-syncthing; see systemd.user.services.sealed-syncthing below.
  # services.syncthing.enable = true;
  services.gnome-keyring = {
    enable = true;
    components = [ "secrets" ];
  };
  systemd.user.services.gnome-keyring.Service.RuntimeDirectory = "keyring";
  systemd.user.services.gnome-keyring.Service.RuntimeDirectoryMode = "0700";
  services.protonmail-bridge.enable = true;

  # Setup: run `protonmail-bridge --cli`, then `info joshheinrichs@protonmail.com`
  # to get the bridge password, then write it to a file:
  #   echo -n "<password>" > ~/.local/share/protonmail/bridge-password
  #   chmod 600 ~/.local/share/protonmail/bridge-password
  # TODO: get the bridge password directly via the bridge gRPC API instead of a file
  accounts.email.accounts.protonmail = {
    primary = true;
    realName = "Josh Heinrichs";
    address = "joshheinrichs@protonmail.com";
    userName = "joshheinrichs@protonmail.com";
    passwordCommand = [
      "cat"
      "/home/josh/.local/share/protonmail/bridge-password"
    ];
    imap = {
      host = "127.0.0.1";
      port = 1143;
      tls.enable = false;
    };
    smtp = {
      host = "127.0.0.1";
      port = 1025;
      tls.enable = false;
    };
    aerc.enable = true;
  };

  programs.aerc = {
    enable = true;
    extraConfig.general.unsafe-accounts-conf = true;
  };

  programs.home-manager.enable = true;
  programs.claude-code = {
    enable = true;
    settings.model = "opus";
    settings.effortLevel = "xhigh";
    settings.voiceEnabled = true;
    settings.voice = {
      enabled = true;
      mode = "hold";
    };
    settings.hooks.PreToolUse = [
      {
        matcher = "Bash";
        hooks = [
          {
            type = "command";
            command = "${pkgs.rtk}/bin/rtk hook claude";
          }
        ];
      }
    ];
    context = "@${rtk-init}/RTK.md";
  };
  programs.fish = {
    enable = true;
    shellAliases = {
      o = "xdg-open";
    };
  };
  programs.git = {
    enable = true;
    settings = {
      user = {
        name = "Josh Heinrichs";
        email = "joshiheinrichs@gmail.com";
      };
      alias = {
        co = "checkout";
        st = "status";
        sw = "switch";
      };
      core.guess = false;
      merge.ff = false;
      pull.ff = "only";
      rebase = {
        autoSquash = true;
        updateRefs = true;
      };
      init.defaultBranch = "main";
      # delta
      delta.naviate = true;
      merge.conflictStyle = "zdiff3";
    };
    ignores = [ ".claude" ];
  };
  programs.delta = {
    enable = true;
    enableGitIntegration = true;
  };
  programs.zoxide = {
    enable = true;
    enableFishIntegration = true;
  };
  programs.firefox = {
    enable = true;
    # The nixpkgs Firefox wrapper sets MOZ_LEGACY_PROFILES=1, so Firefox reads
    # ~/.mozilla/firefox. home-manager's default for stateVersion >= 26.05 is the
    # XDG path (~/.config/mozilla/firefox), which Firefox never opens here, so pin
    # configPath to the legacy path home-manager would otherwise migrate away from.
    configPath = ".mozilla/firefox";
    policies.ExtensionSettings = {
      "uBlock0@raymondhill.net" = {
        install_url = "https://addons.mozilla.org/firefox/downloads/latest/ublock-origin/latest.xpi";
        installation_mode = "force_installed";
      };
      "sponsorBlocker@ajay.app" = {
        install_url = "https://addons.mozilla.org/firefox/downloads/latest/sponsorblock/latest.xpi";
        installation_mode = "force_installed";
      };
    };
    profiles.default = {
      id = 0;
      isDefault = true;
    };
  };
  programs.zed-editor = {
    enable = true;
    extensions = [ "nix" ];
    userSettings = {
      telemetry.metrics = false;
      terminal.shell.program = "${pkgs.fish}/bin/fish";
      buffer_font_features.calt = false;
    };
  };
  programs.foot = {
    enable = true;
    settings.main = {
      shell = "${pkgs.fish}/bin/fish";
      font = "Deja Vu Sans Mono:size=11";
    };
  };
  programs.fuzzel = {
    enable = true;
    settings = {
      main = {
        launch-prefix = launch;
        horizontal-pad = 8;
        vertical-pad = 4;
        inner-pad = 0;
        font = "monospace:size=8";
        width = 60;
      };
      # roughly matching sway
      colors = {
        background = "222222FF";
        text = "888888FF";
        match = "2E9EF4FF";
        selection = "285577FF";
        selection-text = "FFFFFFFF";
        border = "5F676AFF";
      };
      border = {
        width = 1;
        radius = 0;
      };
    };
  };

  fonts.fontconfig.enable = true;
}
