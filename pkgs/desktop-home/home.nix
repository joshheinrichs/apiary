{
  pkgs,
  lib,
  apiary,
  config,
  ...
}:

let
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
    apiary.steam
    keepassxc
    libsecret
    gcr
    qbittorrent
    vlc
    obs-studio
    apiary.bubbled-spotify
    apiary.bubbled-discord
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
    # opencode
    gnome-themes-extra # dark theme
    nautilus
    pavucontrol
    shotman
    wl-clipboard
    # apiary.bubbled-xdg-open
    pavucontrol
    landrun
    bubblewrap
    rtk
    (pkgs.writeShellScriptBin "wm" ''
      export TZ="America/Regina"
      export GTK_THEME="Adwaita:dark"
      export EDITOR="${pkgs.neovim}/bin/nvim"
      exec ${apiary.scoper}/bin/scoper \
        --slice=session-$(${pkgs.systemd}/bin/systemd-escape "$XDG_SESSION_ID") \
        --name=sway \
        -- ${pkgs.sway}/bin/sway "$@"
    '')
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
      terminal = "${apiary.scoper}/bin/scoper --slice=apps -- ${pkgs.foot}/bin/foot";
      keybindings = lib.mkOptionDefault {
        "${modifier}+space" = "exec ${apiary.fuzzel-window-switcher}/bin/fuzzel-window-switcher";
        "Print" = "exec shotman --capture output";
        "Alt+Print" = "exec shotman --capture region";
      };
      # window.commands = [
      #   { criteria = { class = ".*"; }; command = "move container to workspace 1, workspace 1"; }
      # ];
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
      };
      Service = {
        ExecStart = "${pkgs.wireplumber}/bin/wireplumber";
        Restart = "on-failure";
      };
      Install = {
        WantedBy = [ "default.target" ];
      };
    };

    bubbled-syncthing =
      let
        pkg = apiary.bubbled-syncthing.override {
          extraArgs = [
            "--pasta-tcp=127.0.0.1/8384"
            "--rw-bind=/home/josh/syncthing:/home/josh/syncthing"
          ];
        };
      in
      {
        Unit.Description = "Sandboxed Syncthing (bubblewand + pasta)";
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
    settings = {
      extra-experimental-features = "nix-command";
    };
  };

  # Replaced by sandboxed bubbled-syncthing; see systemd.user.services.bubbled-syncthing below.
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
    configPath = "${config.xdg.configHome}/mozilla/firefox";
    profiles.default.settings = {
      "widget.content.allow-gtk-dark-theme" = true;
      "ui.systemUsesDarkTheme" = 2;
    };
    # TODO: extensions
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
        launch-prefix = "${apiary.scoper}/bin/scoper --slice=apps";
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
