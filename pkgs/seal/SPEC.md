# seal spec

## Overview

seal is a thin wrapper around `bwrap` (bubblewrap) for NixOS. It provides two binaries:

- **`seal`** — runtime: takes flags + a command, builds bwrap args, and execs into the sandbox.
- **`seal-generator`** — build-time tool: takes the same flags + a package path, and emits a wrapper script (and patched `.desktop` files) that call `seal` with the baked-in flags.

Both binaries are compiled with `BWRAP`, `XDG_DBUS_PROXY`, and `PASTA` baked in as store paths. The generator also bakes in the path to the `seal` runtime.

---

## Flags

### Feature groups

| Flag | Implies |
|------|---------|
| `--gui` | `--wayland`, `--pulse`, `--pipewire` |
| `--audio` | `--pulse`, `--pipewire` |
| `--wayland` | — |
| `--pulse` | — |
| `--pipewire` | — |
| `--gpu` | — |
| `--camera` | — |

### Network

`--net=POLICY` sets what the sandbox can reach. Default `none`: loopback only.

| Policy | Netns | Reaches |
|--------|-------|---------|
| `none` | private, no link | nothing |
| `internet`, `lan`, `host`, or a comma-separated set | private, bridged by pasta | the listed zones |
| `all` | private, bridged by pasta | everything, no firewall |
| `shared` | the host's | everything, including binding host ports |

Zones: `host` is this machine's loopback; `lan` is private, CGNAT, link-local and multicast ranges (IPv4 and IPv6); `internet` is everything else. DNS is allowed whenever `lan` or `internet` is. `all` and `shared` can't be combined with zones.

`--publish=tcp:SPEC` / `--publish=udp:SPEC` forwards a host port in, SPEC in `pasta -t`/`-u` syntax (`8384`, `127.0.0.1/8384`, `8384:9000`). Pasta policies only.

`--pasta-mac=ADDR` sets the sandbox interface's MAC.

### Home

`--persist-home=NAME` — bind-mounts `$XDG_DATA_HOME/seal/NAME/home` as the sandbox home. Without it, home is an empty ephemeral directory.

`--share-tmp=NAME` — bind-mounts `$XDG_RUNTIME_DIR/seal/NAME` as the sandbox `/tmp` instead of an isolated tmpfs. Use this when multiple instances of the same sandboxed app need to share `/tmp` (e.g. for Electron's singleton socket). `$XDG_RUNTIME_DIR` is user-owned (mode 0700) and cleared on logout.

### DBus filtering

`--dbus-talk=NAME` and `--dbus-own=NAME` spawn an `xdg-dbus-proxy` with `--filter`. If either is given, a proxy is started before bwrap and its socket is bind-mounted into the sandbox.

### Environment

`--set-env=KEY=VALUE` — set an env var inside the sandbox.  
`--fwd-env=KEY` — forward a host env var into the sandbox.

### Mounts

`--ro-bind=HOST:DEST` — read-only bind mount.  
`--rw-bind=HOST:DEST` — read-write bind mount.  
`--tmpfs=PATH` — tmpfs at PATH.  
`--device=PATH` — pass a device node through (`--dev-bind-try`), e.g. `/dev/ntsync`.

### Other

`--hostname=NAME` (default: `bubble`)  
`--new-session` — pass `--new-session` to bwrap (calls `setsid()`).  
`--keep-env` — inherit the full host environment instead of starting clean (default: `--clearenv`).  
`--bwrap=PATH` — override the bwrap binary (hidden flag, for testing).

---

## Sandbox construction

The bwrap invocation is built in this order:

### Base filesystem

```
--proc /proc
--dev /dev
--tmpfs /tmp
```

### Home

Persistent: `--bind $XDG_DATA_HOME/seal/NAME $HOME`  
Ephemeral: `--dir $HOME`

### /etc

```
--tmpfs /etc
--file <fd> /etc/passwd      # the current user, real uid/gid
--file <fd> /etc/group       # the current user's primary group, real gid
--file <fd> /etc/hostname    # contains the --hostname value
--file <fd> /run/host/container-manager   # "seal": the container interface marker; SDL then watches /dev/input instead of udev
--ro-bind /etc/localtime /etc/localtime   # if it exists on host
```

### Isolation

```
--die-with-parent
--unshare-all
--share-net          # --net=shared, or any pasta policy (see Pasta network)
```

### Network files (any policy but none)

```
--ro-bind /etc/hosts /etc/hosts
--ro-bind /etc/nsswitch.conf /etc/nsswitch.conf
--ro-bind /etc/resolv.conf /etc/resolv.conf     # --net=shared
--file <fd> /etc/resolv.conf                    # pasta: "nameserver 169.254.1.1"
--ro-bind /etc/ssl /etc/ssl
--setenv TZ $TZ      # if TZ is set on host
```

### Pasta network (`all` and zone sets)

seal builds the network before bwrap runs, then runs bwrap inside it with `--share-net`:

1. Fork a helper, which stays in the host netns (pasta's outbound sockets live there).
2. seal unshares a user + net namespace and maps its own uid/gid into it.
3. The helper loads the zone firewall, then attaches pasta, then reports back. Firewall first, so the link never comes up unfiltered.
4. The helper signals ready (see [Services](#services)). Any failure in 2–3 aborts the sandbox.

The payload ends up in bwrap's user namespace, nested below the one that owns the netns, so it has no capabilities over the firewall. Verified: `nft flush ruleset` is refused inside, including from a further nested userns.

The firewall is an nftables `output` chain in table `inet seal`, policy = the `internet` verdict: loopback and established traffic pass, then DNS to the forwarder, the host address, and the LAN ranges are each accepted or dropped per zone. It's loaded from a forked child that `setns`es into the namespaces and calls libnftables in-process — exec'ing `nft` would drop the capabilities setns grants, since the uid isn't root there.

pasta runs `--config-net --host-lo-to-ns-lo -T none -U none --dns-forward 169.254.1.1`, plus `--map-host-loopback 169.254.1.2` when the host is reachable or `--no-map-gw` otherwise, plus the `--publish` forwards (`-t none -u none` when there are none). It configures the netns, daemonizes, and quits when the netns goes away; the foreground exit is the readiness signal.

Things that cost time to learn:

- pasta's default maps the **gateway address to the host's loopback**, so without `--no-map-gw` a sandbox reaches every `127.0.0.1` service via the gateway IP.
- `-T`/`-U` default to `auto`, which mirrors every host listener into the sandbox and steals those ports.
- Don't drive pasta or nft off bwrap's `--info-fd` child PID: bwrap reports it right after `clone()`, before writing the uid map, and later moves into a nested userns for devpts. Both race.
- pasta closes inherited fds at startup, so namespaces have to be passed as `/proc/<pid>/ns/*` paths.

### UTS + env baseline

```
--hostname <name>
--clearenv              # skipped if --keep-env
--setenv HOME $HOME
--setenv TERM $TERM     # if set on host
--setenv LANG $LANG     # if set on host
--setenv TZ   $TZ       # if set on host
```

PATH is not set automatically. Use `--set-env=PATH=...` to set it explicitly.

### XDG_RUNTIME_DIR (if wayland/pulse/pipewire/dbus)

```
--setenv XDG_RUNTIME_DIR $XDG_RUNTIME_DIR
--dir $XDG_RUNTIME_DIR
```

### GPU (if --gpu)

```
--dev-bind /dev/dri /dev/dri
--ro-bind /sys/dev/char /sys/dev/char
--ro-bind /run/opengl-driver /run/opengl-driver       # if exists
--ro-bind /run/opengl-driver-32 /run/opengl-driver-32 # if exists
--ro-bind /sys/devices/pci.../<gpu> /sys/devices/pci.../<gpu>
          # one entry per PCI device with a drm/ subdirectory,
          # paths canonicalized from /sys/bus/pci/devices symlinks
          # to their real /sys/devices/pci... locations (required for VA-API)
```

### Wayland (if --wayland or --gui)

```
--ro-bind $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY
--setenv WAYLAND_DISPLAY $WAYLAND_DISPLAY
--setenv XDG_SESSION_TYPE $XDG_SESSION_TYPE   # if set
```

### PulseAudio (if --pulse or --audio or --gui)

```
--bind-try /run/pulse /run/pulse
--bind-try $XDG_RUNTIME_DIR/pulse $XDG_RUNTIME_DIR/pulse
--setenv PULSE_SERVER $PULSE_SERVER   # if set
```

### PipeWire (if --pipewire or --audio or --gui)

`--pipewire` binds the host's PipeWire; `--audio`/`--gui` get a restricted proxy service instead (see [Services](#services)):

```
--bind-try $RUN/pipewire-0 $XDG_RUNTIME_DIR/pipewire-0      # proxy
--bind-try /run/pipewire /run/pipewire                      # --pipewire
--bind-try $XDG_RUNTIME_DIR/pipewire-0 $XDG_RUNTIME_DIR/pipewire-0
```

### GUI extras (if --gui)

```
--ro-bind-try /etc/fonts /etc/fonts
--tmpfs $HOME/.config/dconf
--ro-bind-try $HOME/.config/dconf $HOME/.config/dconf
--setenv XDG_DATA_DIRS <resolved>
          # host XDG_DATA_DIRS with symlinks canonicalized;
          # non-store directories are explicitly bound
--setenv XCURSOR_THEME / XCURSOR_SIZE / XCURSOR_PATH   # if set
--ro-bind-try <dir> <dir>   # for each dir in XCURSOR_PATH
```

### Camera (if --camera)

```
--dev-bind /dev/videoN /dev/videoN   # for each /dev/video0..63 that exists
```

### DBus proxy (if --dbus-talk or --dbus-own)

```
--ro-bind $RUN/dbus $XDG_RUNTIME_DIR/bus
--setenv DBUS_SESSION_BUS_ADDRESS unix:path=$XDG_RUNTIME_DIR/bus
```

A service (see [Services](#services)): `xdg-dbus-proxy --filter` with the `--talk`/`--own` names, readiness on its `--fd` pipe.

### User-supplied overrides (appended last)

`--ro-bind`, `--rw-bind`, `--tmpfs`, `--set-env`, `--fwd-env` — applied in order after all builtins.

### New session

```
--new-session   # only if --new-session flag given
```

---

## Services

The helpers an app needs start in parallel; the app starts once all are ready.

| Service | Started when | Ready |
|---------|--------------|-------|
| xdg-dbus-proxy | `--dbus-talk`/`--dbus-own` | a byte on its `--fd` pipe |
| pipewire + wireplumber | `--audio`, `--gui` | at spawn: socket-activated, seal binds the listening socket and passes it as fd 3 with `LISTEN_FDS=1` |
| network | `--net=all` or zones | a byte from the helper once the firewall and pasta are up |

The network service is spawned last, because seal itself moves into the sandbox netns there and anything spawned later would follow it.

seal waits with one `poll()` over every ready fd plus each service's pidfd. EOF on a ready fd, or a service exiting before it's ready, fails the sandbox: everything started is killed and the app never runs.

seal stays the parent of the app (bwrap, or cage running bwrap), waits for it, kills the services, and exits with the app's code (128+signal if it was killed).

**Nothing outlives seal.** Every service and the app get `PR_SET_PDEATHSIG=SIGKILL`, plus a `getppid()` check for seal dying between fork and prctl. PDEATHSIG fires when the parent *thread* exits; seal is single-threaded, so that is when seal exits.

**Per-sandbox run dir.** `$RUN = $XDG_RUNTIME_DIR/seal-<pid>/` (0700) holds the dbus socket, the pipewire sockets (pipewire's `XDG_RUNTIME_DIR` points here) and cage's runtime dir. It's removed whole on exit, so no service's own cleanup matters — a SIGKILLed pipewire leaves `-manager` sockets and `.lock` files that seal never needs to know about. Only a SIGKILL of seal itself leaves the dir behind — files only, no processes — until logout clears `/run/user`.

---

## Generator

### Usage

```
seal-generator [flags] <source-pkg> <output-dir>
```

Accepts all the same flags as `seal`, plus:

`--bin=NAME` — only wrap the named binary (may be repeated; default: all executables).  
`--ro-bind-file=FILE` — file containing paths to bind read-only, one per line. Each path is bound to itself (`--ro-bind PATH PATH`). Baked into the wrapper at build time — no runtime file reads. Use with `closureInfo` to restrict the sandbox to only the paths the app needs rather than the entire store.

### Output

For each executable in `<source-pkg>/bin/`:

```sh
#!/bin/sh
exec seal [flags] [--ro-bind=PATH:PATH ...] -- /nix/store/.../bin/<exe> "$@"
```

`.desktop` files have their `Exec=` and `TryExec=` lines rewritten to point at the wrapped binary. Icons are symlinked.
