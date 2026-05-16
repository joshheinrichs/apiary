# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ config, pkgs, ... }:

{
  imports = [
    # Include the results of the hardware scan.
    ./hardware-configuration.nix
  ];

  # Bootloader.
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
  boot.kernelPackages = pkgs.cachyosKernels.linuxPackages-cachyos-latest;
  # for ebpf + alloy
  boot.kernel.sysctl = {
    "kernel.unprivileged_bpf_disabled" = 0;
    # TODO: can this be strengthened?
    "kernel.perf_event_paranoid" = -1;
    "kernel.kptr_restrict" = 0;
  };

  networking.hostName = "nixos"; # Define your hostname.
  # networking.wireless.enable = true;  # Enables wireless support via wpa_supplicant.

  # Configure network proxy if necessary
  # networking.proxy.default = "http://user:password@proxy:port/";
  # networking.proxy.noProxy = "127.0.0.1,localhost,internal.domain";

  # Enable networking
  networking.networkmanager.enable = true;

  # Set your time zone.
  time.timeZone = "UTC";

  # Select internationalisation properties.
  i18n.defaultLocale = "C.UTF-8";
  i18n.supportedLocales = [ "all" ];

  # Enable the X11 windowing system.
  # services.xserver.enable = true;

  # Enable the GNOME Desktop Environment.
  # services.xserver.displayManager.gdm.enable = true;
  # services.xserver.desktopManager.gnome.enable = true;

  # Configure keymap in X11
  # services.xserver.xkb = {
  #   layout = "us";
  #   variant = "";
  # };

  # Enable CUPS to print documents.
  # services.printing.enable = true;

  # Enable sound with pipewire.
  # services.pulseaudio.enable = false;
  security.rtkit.enable = true;
  # services.pipewire = {
  #   enable = true;
  #   alsa.enable = true;
  #   alsa.support32Bit = true;
  #   pulse.enable = true;
  #   # If you want to use JACK applications, uncomment this
  #   #jack.enable = true;
  #
  #   # use the example session manager (no others are packaged yet so this is enabled by default,
  #   # no need to redefine it in your config for now)
  #   #media-session.enable = true;
  # };

  # Enable touchpad support (enabled default in most desktopManager).
  # services.xserver.libinput.enable = true;

  hardware.bluetooth.enable = true;

  # https://wiki.nixos.org/w/index.php?title=Prometheus
  services.prometheus = {
    enable = true;
    exporters.node = {
      enable = true;
      port = 9000;
      enabledCollectors = [ "systemd" ];
      extraFlags = [
        "--collector.ethtool"
        "--collector.softirqs"
        "--collector.tcpstat"
        "--collector.wifi"
        "--collector.drm"
      ];
    };
    scrapeConfigs = [
      {
        job_name = "node";
        static_configs = [
          {
            targets = [
              "${config.services.prometheus.exporters.node.listenAddress}:${toString config.services.prometheus.exporters.node.port}"
            ];
          }
        ];
      }
    ];
  };
  services.grafana = {
    enable = true;
    settings = {
      server = {
        http_addr = "127.0.0.1";
        http_port = 3000;
        enable_gzip = true;
      };
      security = {
        secret_key = "SW2YcwTIb9zpOOhoPsMm";
      };
      "auth.anonymous" = {
        enabled = true;
        org_role = "Admin";
      };
    };
    provision = {
      enable = true;
      datasources.settings.datasources = [
        {
          name = "Prometheus";
          type = "prometheus";
          url = "http://${config.services.prometheus.listenAddress}:${toString config.services.prometheus.port}";
          isDefault = true;
          editable = false;
        }
        {
          name = "Pyroscope";
          type = "grafana-pyroscope-datasource";
          url = "http://127.0.0.1:4040";
          editable = false;
        }
      ];
    };
  };

  services.nixseparatedebuginfod2.enable = true;

  services.pyroscope = {
    enable = true;
    extraFlags = [
      "-target=all"
      "-auth.multitenancy-enabled=false"
      "-memberlist.bind-addr=127.0.0.1"
      "-memberlist.advertise-addr=127.0.0.1"
      "-ingester.lifecycler.addr=127.0.0.1"
      "-ingester.lifecycler.interface=lo"
      "-distributor.ring.instance-addr=127.0.0.1"
      "-compactor.ring.instance-addr=127.0.0.1"
      "-query-scheduler.ring.instance-addr=127.0.0.1"
      "-store-gateway.sharding-ring.instance-addr=127.0.0.1"
      "-overrides-exporter.ring.instance-addr=127.0.0.1"
      "-query-frontend.instance-addr=127.0.0.1"
      "-segment-writer.lifecycler.addr=127.0.0.1"
      "-segment-writer.lifecycler.interface=lo"
    ];

    settings = {
      # 3. Disable the ring replication logic
      ingester.lifecycler.ring = {
        kvstore.store = "inmemory";
        replication_factor = 1;
      };
      distributor.ring.kvstore.store = "inmemory";

      server = {
        http_listen_address = "127.0.0.1";
        http_listen_port = 4040;
        grpc_listen_address = "127.0.0.1";
        grpc_listen_port = 9095;
      };

      storage.backend = "filesystem";

      symbolizer.debuginfod_url = "http://127.0.0.1:${toString config.services.nixseparatedebuginfod2.port}";
      limits.symbolizer.enabled = true;
    };
  };

  systemd.services.pyroscope.environment.PYROSCOPE_V2 = "1";

  services.alloy = {
    enable = true;
    # Required for eBPF profiling
    # systemd.services.alloy.serviceConfig.User = "root";

    configPath = pkgs.writeText "alloy-config.alloy" ''
      discovery.process "all" {
        discover_config {
          cgroup_path = true
        }
      }

      discovery.relabel "processes" {
        targets = discovery.process.all.targets

        rule {
          target_label = "service_name"
          replacement  = "desktop"
        }

        rule {
          source_labels = ["__meta_process_cgroup_path"]
          target_label  = "cgroup_path"
        }

        rule {
          source_labels = ["__meta_process_exe"]
          regex         = ".*/([^/]+)"
          target_label  = "process_name"
          replacement   = "$1"
        }
      }

      pyroscope.ebpf "default" {
        targets    = discovery.relabel.processes.output
        forward_to = [pyroscope.write.local.receiver]
        demangle   = "full"
      }

      pyroscope.write "local" {
        endpoint {
          url = "http://127.0.0.1:4040"
        }
      }
    '';
  };

  # Alloy needs this for the eBPF component to work
  systemd.services.alloy.serviceConfig.User = "root";

  services.udev.extraRules = ''
    SUBSYSTEM=="drm", KERNEL=="card1", ATTR{card1-HDMI-A-1/status}=="connected", TAG+="seat", ENV{ID_SEAT}="seat1"
  '';

  services.logind.settings.Login.NAutoVTs = 6;

  # https://wiki.nixos.org/wiki/Jellyfin
  services.jellyfin.enable = true;
  services.jellyfin.user = "josh";

  systemd.oomd = {
    enable = true;
    enableRootSlice = true;
    enableSystemSlice = true;
    enableUserSlices = true;
  };

  security.polkit.enable = true;
  # xdg.portal.enable = true;
  # xdg.portal.wlr.enable = true;
  services.udisks2.enable = true;

  # Define a user account. Don't forget to set a password with ‘passwd’.
  users.users.josh = {
    isNormalUser = true;
    description = "Josh Heinrichs";
    extraGroups = [
      "networkmanager"
      "wheel"
      "video"
    ];
    packages = with pkgs; [
      #  thunderbird
    ];
  };

  hardware.graphics.enable = true;

  # Allow unfree packages
  nixpkgs.config.allowUnfree = true;

  # List packages installed in system profile. To search, run:
  # $ nix search wget
  environment.systemPackages = with pkgs; [
    #  vim # Do not forget to add an editor to edit configuration.nix! The Nano editor is also installed by default.
    #  wget
    wgnord
  ];

  # Some programs need SUID wrappers, can be configured further or are
  # started in user sessions.
  # programs.mtr.enable = true;
  # programs.gnupg.agent = {
  #   enable = true;
  #   enableSSHSupport = true;
  # };

  # List services that you want to enable:

  # Enable the OpenSSH daemon.
  # services.openssh.enable = true;

  # Open ports in the firewall.
  # networking.firewall.allowedTCPPorts = [ ... ];
  # networking.firewall.allowedUDPPorts = [ ... ];
  # Or disable the firewall altogether.
  # networking.firewall.enable = false;

  # This value determines the NixOS release from which the default
  # settings for stateful data, like file locations and database versions
  # on your system were taken. It‘s perfectly fine and recommended to leave
  # this value at the release version of the first install of this system.
  # Before changing this value read the documentation for this option
  # (e.g. man configuration.nix or on https://nixos.org/nixos/options.html).
  system.stateVersion = "25.05"; # Did you read the comment?
}
