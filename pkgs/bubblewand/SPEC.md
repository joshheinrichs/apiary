# bubblewand spec

## Overview

bubblewand is a thin wrapper around `bwrap` (bubblewrap) for NixOS. It provides two binaries:

- **`bubblewand`** — runtime: takes flags + a command, builds bwrap args, and execs into the sandbox.
- **`bubblewand-generator`** — build-time tool: takes the same flags + a package path, and emits a wrapper script (and patched `.desktop` files) that call `bubblewand` with the baked-in flags.

Both binaries are compiled with `BWRAP`, `XDG_DBUS_PROXY`, and `PASTA` baked in as store paths. The generator also bakes in the path to the `bubblewand` runtime.

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
| `--network` | — |
| `--pasta` | — |
| `--camera` | — |

`--network` and `--pasta` are mutually exclusive. `--network` shares the host network namespace; `--pasta` keeps the netns unshared and bridges it to the host via `pasta` (see [Pasta networking](#pasta-networking) below).

### Home

`--persist-home=NAME` — bind-mounts `$XDG_DATA_HOME/bubblewand/NAME/home` as the sandbox home. Without it, home is an empty ephemeral directory.

`--share-tmp=NAME` — bind-mounts `$XDG_RUNTIME_DIR/bubblewand/NAME` as the sandbox `/tmp` instead of an isolated tmpfs. Use this when multiple instances of the same sandboxed app need to share `/tmp` (e.g. for Electron's singleton socket). `$XDG_RUNTIME_DIR` is user-owned (mode 0700) and cleared on logout.

### DBus filtering

`--dbus-talk=NAME` and `--dbus-own=NAME` spawn an `xdg-dbus-proxy` with `--filter`. If either is given, a proxy is started before bwrap and its socket is bind-mounted into the sandbox.

### Pasta networking

`--pasta` keeps the sandbox netns unshared (no `--share-net` to bwrap) and attaches a userspace network bridge (`pasta`, from passt) to it. Rootless: needs no `CAP_NET_ADMIN`.

`--pasta-tcp=SPEC` and `--pasta-udp=SPEC` are repeatable port-forwarding specs passed verbatim to `pasta -t SPEC` / `pasta -u SPEC`. See `pasta(1)` for SPEC syntax. Examples: `8384` (forward host port 8384 → guest 8384), `127.0.0.1/8384` (bind only host's loopback), `8384:9000` (host 8384 → guest 9000). When neither flag is given, pasta runs with `-t none -u none` (outbound-only).

### Environment

`--set-env=KEY=VALUE` — set an env var inside the sandbox.  
`--fwd-env=KEY` — forward a host env var into the sandbox.

### Mounts

`--ro-bind=HOST:DEST` — read-only bind mount.  
`--rw-bind=HOST:DEST` — read-write bind mount.  
`--tmpfs=PATH` — tmpfs at PATH.

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

Persistent: `--bind $XDG_DATA_HOME/bubblewand/NAME $HOME`  
Ephemeral: `--dir $HOME`

### /etc

```
--tmpfs /etc
--file <fd> /etc/passwd      # minimal: root + current user
--file <fd> /etc/group       # minimal: root + current user's primary group
--file <fd> /etc/hostname    # contains the --hostname value
--ro-bind /etc/localtime /etc/localtime   # if it exists on host
```

### Isolation

```
--die-with-parent
--unshare-all
--share-net          # only if --network
```

### Network files (only if --network or --pasta)

```
--ro-bind /etc/hosts /etc/hosts
--ro-bind /etc/nsswitch.conf /etc/nsswitch.conf
--ro-bind /etc/resolv.conf /etc/resolv.conf
--ro-bind /etc/ssl /etc/ssl
--setenv TZ $TZ      # if TZ is set on host
```

### Pasta orchestration (only if --pasta)

bwrap is told to communicate setup with a sibling orchestrator process via two pipes:

```
--info-fd <N>    # bwrap writes JSON {"child-pid": ...} once namespaces are set up
--block-fd <M>   # bwrap blocks on this fd before exec'ing the payload
```

The orchestration sequence:

1. Before forking the dbus proxy / before exec, bubblewand creates `info_pipe` (orchestrator reads, bwrap writes) and `block_pipe` (orchestrator writes, bwrap reads).
2. bubblewand forks the **pasta orchestrator** child. The orchestrator-side ends are kept open in the child; the bwrap-side ends have `FD_CLOEXEC` cleared in the parent so they survive `exec` into bwrap.
3. bubblewand `exec`s into bwrap with `--info-fd` / `--block-fd` referring to the bwrap-side fd numbers.
4. bwrap creates namespaces, writes `{"child-pid": N}` to `info_pipe`, blocks on `block_pipe`.
5. Orchestrator reads the PID and runs `pasta --quiet --config-net --host-lo-to-ns-lo -T none -U none --userns /proc/<PID>/ns/user --netns /proc/<PID>/ns/net [-t SPEC|none] [-u SPEC|none]`, then waits for it to exit.

   - `--host-lo-to-ns-lo`: translates host-loopback forwards to the sandbox's loopback so apps that bind `127.0.0.1` inside stay reachable via the host loopback forward.
   - `-T none -U none`: disables pasta's default namespace→host forwarding (`-T auto -U auto`). The defaults would create transparent in-namespace listeners for every host-bound port, which both leaks host services into the sandbox and steals ports the sandboxed app may want (e.g. syncthing's TCP/UDP 22000 conflicts with a host syncthing's matching binds).
6. Pasta sets up the tap interface synchronously and daemonizes (default behavior — no `--foreground`); the spawned process exits with status 0 once setup is complete. That exit is the readiness signal — no pid file polling, no race.
7. Orchestrator writes one byte to `block_pipe` to unblock bwrap, then exits. The detached pasta daemon stays alive until the netns is torn down (default behavior; no `--no-netns-quit`).
8. bwrap unblocks and `exec`s the payload, which runs in the unshared netns bridged by pasta.

`--userns` is passed because bwrap's netns is owned by bwrap's user namespace, and pasta needs `CAP_SYS_ADMIN` in that user namespace to enter the netns. Without it, pasta fails with "Couldn't switch to pasta namespaces: Operation not permitted".

If pasta cannot be spawned or its setup fails, the orchestrator still writes the unblock byte so bwrap doesn't hang — the sandbox runs with no network connectivity, with an error on stderr.

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

```
--bind-try /run/pipewire /run/pipewire
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

xdg-dbus-proxy is started before bwrap with `--filter` and the specified `--talk`/`--own` names. A `socketpair(AF_UNIX, SOCK_STREAM)` is used: the proxy writes a zero byte to its end when ready; bubblewand reads that byte before proceeding, ensuring the proxy is accepting connections before bwrap starts.

The socket appears at `$XDG_RUNTIME_DIR/bubblewand-dbus.sock` and is bound into the sandbox:

```
--ro-bind $XDG_RUNTIME_DIR/bubblewand-dbus.sock $XDG_RUNTIME_DIR/bus
--setenv DBUS_SESSION_BUS_ADDRESS unix:path=$XDG_RUNTIME_DIR/bus
```

The proxy's lifetime is tied to bwrap: bubblewand keeps the parent end of the socketpair open across `exec` into bwrap, so when bwrap exits the socket closes and the proxy sees `POLLHUP` and exits.

### User-supplied overrides (appended last)

`--ro-bind`, `--rw-bind`, `--tmpfs`, `--set-env`, `--fwd-env` — applied in order after all builtins.

### New session

```
--new-session   # only if --new-session flag given
```

---

## Generator

### Usage

```
bubblewand-generator [flags] <source-pkg> <output-dir>
```

Accepts all the same flags as `bubblewand`, plus:

`--bin=NAME` — only wrap the named binary (may be repeated; default: all executables).  
`--ro-bind-file=FILE` — file containing paths to bind read-only, one per line. Each path is bound to itself (`--ro-bind PATH PATH`). Baked into the wrapper at build time — no runtime file reads. Use with `closureInfo` to restrict the sandbox to only the paths the app needs rather than the entire store.

### Output

For each executable in `<source-pkg>/bin/`:

```sh
#!/bin/sh
exec bubblewand [flags] [--ro-bind=PATH:PATH ...] -- /nix/store/.../bin/<exe> "$@"
```

`.desktop` files have their `Exec=` and `TryExec=` lines rewritten to point at the wrapped binary. Icons are symlinked.
