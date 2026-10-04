{ pkgs }:
let
  inherit (pkgs) lib;

  # Panels use the default datasource (Prometheus).
  rate = metric: "rate(${metric}[$__rate_interval])";

  # hwmon readings labelled by sensor name (Tctl, junction, Composite, ...),
  # selected by chip name rather than PCI path.
  hwmon =
    metric: chipName:
    ''${metric} * on(chip, sensor) group_left(label) node_hwmon_sensor_label * on(chip) group_left(chip_name) node_hwmon_chip_names{chip_name="${chipName}"}'';

  # The discrete GPU: the only amdgpu chip that reports power.
  gpu = metric: "(${hwmon metric "amdgpu"}) and on(chip) node_hwmon_power_average_watt";

  # NVMe names (nvme0, nvme1) swap between boots; tag series with the drive
  # model instead. `key` is the joining label and `rewrite` maps nvmeN to it.
  nvmeModel =
    expr: key: rewrite:
    ''${expr} * on(${key}) group_left(model) label_replace(node_nvme_info, "${key}", "${rewrite}", "device", "(.+)")'';

  # PSI "some": the share of time at least one task waited on the resource.
  pressure =
    resource:
    stat {
      title = "${resource} pressure";
      expr = rate "node_pressure_${lib.toLower resource}_waiting_seconds_total";
      unit = "percentunit";
      warn = 0.1;
      crit = 0.3;
    };

  steps = warn: crit: [
    {
      color = "green";
      value = null;
    }
    {
      color = "yellow";
      value = warn;
    }
    {
      color = "red";
      value = crit;
    }
  ];

  stat =
    {
      title,
      expr,
      unit,
      warn,
      crit,
    }:
    gridPos: {
      inherit title gridPos;
      type = "stat";
      targets = [ { inherit expr; } ];
      options.colorMode = "background";
      fieldConfig.defaults = {
        inherit unit;
        thresholds = {
          mode = "absolute";
          steps = steps warn crit;
        };
      };
    };

  graph =
    {
      title,
      unit,
      targets,
      stack ? false,
    }:
    gridPos: {
      inherit title gridPos;
      type = "timeseries";
      targets = map (t: {
        inherit (t) expr;
        legendFormat = t.legend or "";
      }) targets;
      fieldConfig.defaults = {
        inherit unit;
        custom = lib.optionalAttrs stack {
          stacking.mode = "normal";
          fillOpacity = 30;
        };
      };
    };

  # Rows of glanceable stats, then collapsed rows of detail graphs.
  dashboard =
    {
      uid,
      title,
      tiles,
      rows,
    }:
    let
      tileRow =
        j: ts:
        lib.imap0 (
          i: s:
          s {
            x = 24 / builtins.length ts * i;
            y = 4 * j;
            w = 24 / builtins.length ts;
            h = 4;
          }
        ) ts;
      top = 4 * builtins.length tiles;
      row = k: r: {
        type = "row";
        title = r.title;
        collapsed = true;
        gridPos = {
          x = 0;
          y = top + k;
          w = 24;
          h = 1;
        };
        panels = lib.imap0 (
          i: g:
          g {
            x = 12 * (i - 2 * (i / 2));
            y = top + 1 + k + 8 * (i / 2);
            w = 12;
            h = 8;
          }
        ) r.panels;
      };
    in
    {
      name = "${uid}.json";
      path = pkgs.writeText "${uid}.json" (
        builtins.toJSON {
          inherit uid title;
          time = {
            from = "now-3h";
            to = "now";
          };
          panels = lib.concatLists (lib.imap0 tileRow tiles) ++ lib.imap0 row rows;
        }
      );
    };
in
pkgs.linkFarm "grafana-dashboards" [
  (dashboard {
    uid = "health";
    title = "System Health";
    tiles = [
      [
        (pressure "CPU")
        (pressure "Memory")
        (pressure "IO")
      ]
      [
        (stat {
          title = "CPU";
          expr = "1 - avg(${rate ''node_cpu_seconds_total{mode="idle"}''})";
          unit = "percentunit";
          warn = 0.8;
          crit = 0.95;
        })
        (stat {
          title = "Memory";
          expr = "1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes";
          unit = "percentunit";
          warn = 0.8;
          crit = 0.9;
        })
        (stat {
          title = "Swap";
          expr = "1 - node_memory_SwapFree_bytes / node_memory_SwapTotal_bytes";
          unit = "percentunit";
          warn = 0.5;
          crit = 0.8;
        })
        (stat {
          title = "CPU temp";
          expr = "max(${hwmon "node_hwmon_temp_celsius" "k10temp"})";
          unit = "celsius";
          warn = 80;
          crit = 90;
        })
        (stat {
          title = "GPU";
          expr = ''node_drm_gpu_busy_percent{card="card1"} / 100'';
          unit = "percentunit";
          warn = 0.8;
          crit = 0.95;
        })
        (stat {
          title = "GPU temp";
          expr = "max(${gpu "node_hwmon_temp_celsius"})";
          unit = "celsius";
          warn = 90;
          crit = 105;
        })
        (stat {
          title = "Power";
          expr = "sum(${rate ''node_rapl_joules_total{rapl_zone="package"}''}) + sum(${gpu "node_hwmon_power_average_watt"})";
          unit = "watt";
          warn = 300;
          crit = 450;
        })
        (stat {
          title = "Disk /";
          expr = ''1 - node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"}'';
          unit = "percentunit";
          warn = 0.8;
          crit = 0.9;
        })
      ]
    ];
    rows = [
      {
        title = "CPU";
        panels = [
          (graph {
            title = "Usage by mode";
            unit = "percentunit";
            stack = true;
            targets = [
              {
                expr = ''sum by (mode) (${rate ''node_cpu_seconds_total{mode!="idle"}''}) / scalar(count(node_cpu_seconds_total{mode="idle"}))'';
                legend = "{{mode}}";
              }
            ];
          })
          (graph {
            title = "CPU pressure";
            unit = "percentunit";
            targets = [
              {
                expr = rate "node_pressure_cpu_waiting_seconds_total";
                legend = "some";
              }
            ];
          })
          (graph {
            title = "Temperature";
            unit = "celsius";
            targets = [
              {
                expr = hwmon "node_hwmon_temp_celsius" "k10temp";
                legend = "{{label}}";
              }
            ];
          })
          (graph {
            title = "Load";
            unit = "short";
            targets = [
              {
                expr = "node_load1";
                legend = "1m";
              }
              {
                expr = "node_load5";
                legend = "5m";
              }
              {
                expr = "node_load15";
                legend = "15m";
              }
            ];
          })
        ];
      }
      {
        title = "Memory";
        panels = [
          (graph {
            title = "Usage";
            unit = "bytes";
            targets = [
              {
                expr = "node_memory_MemTotal_bytes - node_memory_MemAvailable_bytes";
                legend = "used";
              }
              {
                expr = "node_memory_Cached_bytes + node_memory_Buffers_bytes";
                legend = "cache";
              }
              {
                expr = "node_memory_SwapTotal_bytes - node_memory_SwapFree_bytes";
                legend = "swap";
              }
            ];
          })
          (graph {
            title = "Memory pressure";
            unit = "percentunit";
            targets = [
              {
                expr = rate "node_pressure_memory_waiting_seconds_total";
                legend = "some";
              }
              {
                expr = rate "node_pressure_memory_stalled_seconds_total";
                legend = "full";
              }
            ];
          })
          (graph {
            title = "Swap traffic";
            unit = "Bps";
            targets = [
              {
                expr = "${rate "node_vmstat_pswpin"} * 4096";
                legend = "in";
              }
              {
                expr = "${rate "node_vmstat_pswpout"} * 4096";
                legend = "out";
              }
            ];
          })
        ];
      }
      {
        title = "GPU";
        panels = [
          (graph {
            title = "Busy";
            unit = "percent";
            targets = [ { expr = ''node_drm_gpu_busy_percent{card="card1"}''; } ];
          })
          (graph {
            title = "Memory";
            unit = "bytes";
            targets = [
              {
                expr = ''node_drm_memory_vram_used_bytes{card="card1"}'';
                legend = "vram";
              }
              {
                expr = ''node_drm_memory_gtt_used_bytes{card="card1"}'';
                legend = "gtt";
              }
            ];
          })
          (graph {
            title = "Temperature";
            unit = "celsius";
            targets = [
              {
                expr = gpu "node_hwmon_temp_celsius";
                legend = "{{label}}";
              }
            ];
          })
        ];
      }
      {
        title = "Disk";
        panels = [
          (graph {
            title = "Throughput";
            unit = "Bps";
            targets = [
              {
                expr = nvmeModel (rate ''node_disk_read_bytes_total{device=~"nvme.n."}'') "device" "\${1}n1";
                legend = "{{model}} read";
              }
              {
                expr = nvmeModel (rate ''node_disk_written_bytes_total{device=~"nvme.n."}'') "device" "\${1}n1";
                legend = "{{model}} write";
              }
            ];
          })
          (graph {
            title = "IO pressure";
            unit = "percentunit";
            targets = [
              {
                expr = rate "node_pressure_io_waiting_seconds_total";
                legend = "some";
              }
              {
                expr = rate "node_pressure_io_stalled_seconds_total";
                legend = "full";
              }
            ];
          })
          (graph {
            title = "NVMe temperature";
            unit = "celsius";
            targets = [
              {
                expr = nvmeModel "(${hwmon "node_hwmon_temp_celsius" "nvme"})" "chip" "nvme_$1";
                legend = "{{model}} {{label}}";
              }
            ];
          })
        ];
      }
      {
        title = "Power";
        panels = [
          (graph {
            title = "Draw";
            unit = "watt";
            stack = true;
            targets = [
              {
                expr = rate ''node_rapl_joules_total{rapl_zone="package"}'';
                legend = "cpu";
              }
              {
                expr = gpu "node_hwmon_power_average_watt";
                legend = "gpu";
              }
            ];
          })
        ];
      }
      {
        title = "Network";
        panels = [
          (graph {
            title = "Throughput";
            unit = "Bps";
            targets = [
              {
                expr = rate ''node_network_receive_bytes_total{device!="lo"}'';
                legend = "{{device}} rx";
              }
              {
                expr = rate ''node_network_transmit_bytes_total{device!="lo"}'';
                legend = "{{device}} tx";
              }
            ];
          })
        ];
      }
    ];
  })
]
