# seatmux — design

`seatmux` holds the machine's one logind session and hands out complete seats
from it. Each seat is a DRM lease, a set of input device fds, an audio target,
and a supervised child compositor. It never composites, never sees an input
event, and is not in any frame path.

```
tty1 login → wm → seatmux            (logind session, DRM master on card1)
                   ├── sway "desk"   lease {DP-1, DP-2}   + its input fds
                   └── sway "tv"     lease {HDMI-A-2}     + its input fds
```

The children are siblings, not parent and child. Either can die and respawn
without the other noticing, because neither owns the other's resources.

## Why leases

The alternative — one compositor nesting another — was tried and abandoned (see
`desktop-home/INTENT.md`). Nesting makes the parent own all input forever, so
every grab, popup and shortcut-inhibitor becomes a special case, and the child
is a Wayland client the parent has to positively identify. Leasing pushes the
split into DRM, where the kernel enforces it: each child is a real DRM master
over its own connectors, scanning out directly with no copies.

The other alternative — two logind seats — needs a second GPU, because a logind
seat owns whole DRM devices and cannot be given individual connectors.

## Resource acquisition

seatmux is the **only** process that talks to logind. This is forced, not
chosen: `TakeControl` is exclusive per session.

```c
// systemd/src/login/logind-session.c, session_set_controller()
if (s->controller && !force)
        return -EBUSY;
```

and `force` requires uid 0. So exactly one process in the session can be a
libseat controller. Everything else has to get its devices from that process.

seatmux therefore:

- takes control of the session and opens `card1` through libseat — DRM master
  arrives with the fd, since logind sets master on device open
- opens input devices on request from children, and passes the fds on
- relays session activate/deactivate to every child

## DRM leases

A lease is built from a connector, a CRTC, and that CRTC's planes. `card1` has
four CRTCs and eleven planes against three live connectors, so there is room for
both seats plus a spare.

Leases are created **lazily**, when a seat's connectors are first present. The
TV is off more often than on, and whether a *disconnected* connector can be
leased is unverified — building lazily works under either answer and is needed
anyway for power cycles. Once created, a lease is held for the child's lifetime;
sway handles the output appearing and disappearing inside it.

A child's lease is destroyed and rebuilt when that child restarts. Leases are
children of seatmux's master fd, so losing the session revokes all of them at
once — which is why the activation relay below matters.

## seatd protocol server

Children get input through libseat's `seatd` backend, which seatmux implements.
This means **no wlroots patching for input at all** — the client already ships.

Each seat gets its own socket; the child is launched with
`LIBSEAT_BACKEND=seatd` and `SEATD_SOCK` pointing at it, so the connection
identifies the seat and no in-band handshake is needed.

The wire protocol is small — a 4-byte header (`opcode: u16, size: u16`) and ten
message types:

| direction | messages |
|---|---|
| client | `OPEN_SEAT` `CLOSE_SEAT` `OPEN_DEVICE` `CLOSE_DEVICE` `DISABLE_SEAT` `SWITCH_SESSION` `PING` |
| server | `SEAT_OPENED` `SEAT_CLOSED` `DEVICE_OPENED` `DEVICE_CLOSED` `DISABLE_SEAT` `ENABLE_SEAT` `PONG` `SESSION_SWITCHED` `SEAT_DISABLED` `ERROR` |

`DEVICE_OPENED` carries a `device_id` in the body and the fd in `SCM_RIGHTS`
auxiliary data. `device_id`s are allocated per connection by seatmux.

`OPEN_DEVICE` is the policy point. seatmux matches the requested path against
that seat's rules and either takes the device from logind and passes the fd
back, or answers `ERROR`. libinput treats a refused open as "device not
available" and skips it, so a child never sees a device it does not own.

`SWITCH_SESSION` from a child is refused — VT is seatmux's, not a child's.

### Verified semantics

Taken from libseat's `backend/seatd.c`, which is the authority on what a server
may send:

- `header.size` counts the body only, and libseat compares it exactly. A
  bodyless response must declare `0`; `SEAT_OPENED` is the sole variable-length
  message and is checked with `>=`.
- Every request gets exactly one response. `read_header()` treats any other
  opcode as a protocol error, so a server must not volunteer messages mid-exchange.
- `DISABLE_SEAT` and `ENABLE_SEAT` are the only exceptions: `read_and_queue()`
  drains and defers them, so they may be pushed at any time.
- `PONG` is special-cased as a liveness reply and never reaches the event queue.
- `ERROR` may substitute for any response. Its `code` is a positive errno the
  client assigns straight to `errno`, so a refused `OPEN_DEVICE` should carry
  `EACCES` rather than a made-up value.
- The `path_len` on `OPEN_DEVICE` includes the trailing NUL.
- **The seat name in `SEAT_OPENED` must be the udev seat, not ours.** libinput
  enumerates with `libinput_udev_assign_seat(li, session->seat)`, and that name
  comes straight from this reply. Every device on the machine is on `seat0`, so
  reporting an invented label yields zero devices and the compositor dies with
  "libinput initialization failed". The per-seat split belongs in `OPEN_DEVICE`
  policy, which is the only place it can be enforced.
- **A seat is disabled until the server says otherwise.** After answering
  `OPEN_SEAT`, an active session must be announced with an `ENABLE_SEAT` event
  or the client blocks forever at "Waiting for a session to become active".
  Relaying only logind's activation *changes* is not enough — a client that
  connects to an already-active session never sees one.

## Input routing

Children still enumerate devices themselves through udev, which is unprivileged
and gives them hotplug for free. Only *opening* goes through seatmux, and that
is where ownership is decided.

Policy is asymmetric on purpose:

- **desk** — everything *except* the TV's devices, so plugging in something new
  at the desk needs no config change
- **tv** — *only* its listed devices

Devices match on any identity udev reports: the evdev name, `ID_PATH`, or
`vendor:product`. `/dev/input/eventN` numbering is never used — it shuffles
across reboots.

Prefer the evdev name. `vendor:product` is actively wrong for anything on a
Logitech Unifying receiver, where udev reports the receiver's USB id for every
device paired to it; `ID_PATH` names a physical port, so it breaks if the
receiver moves. The evdev name survives both, and is what `desktop-devices`
already records.

Because both compositors enumerate everything, the desk asks for the TV's
keyboard on every scan and is refused with `EACCES`, which libinput treats as
"not available". A seat never learns a device it does not own exists.

Rejected: tagging devices `ID_SEAT="seat-tv"` in udev. That writes into a
namespace logind owns, and logind acts on it — the devices drop out of `seat0`,
lose their `uaccess` ACLs, and end up needing an `input` group grant to
compensate. A mechanism whose workaround is caused by the mechanism is the wrong
mechanism.

## Audio

A seat that declares a sink or source sets `PULSE_SINK` / `PULSE_SOURCE` in the
child's environment at spawn, for PulseAudio clients. Every seat also gets
`PIPEWIRE_PROPS`, because native PipeWire clients read neither of those. All of
it is inherited by everything launched under the seat.

Declaring is for seats that differ from the graph default. A seat that names
nothing inherits it, which is how the desk is configured — the same asymmetry as
its device policy: the desk excludes and inherits, the TV includes and declares.

`PIPEWIRE_PROPS` carries the sink as `target.object` directly, not a hint for a
rule to act on: a rule only applies once the session manager has reloaded its
config, and a seat should not lose its audio to that. `PIPEWIRE_NODE` would do
the same job, but props can carry `node.dont-fallback` alongside the target, so
a seat whose sink is missing goes silent instead of leaking onto another seat.

The `seatmux.seat` tag rides along unused by routing. It is there for the one
thing a single target cannot express — direction — so a seat with its own source
can be given a capture rule without another env var.

The rule is: own the context an app is born into, never re-route streams after
the fact. An earlier tool did this per-launch, inferring context from the focused
workspace's output because a single compositor had no better signal. With one
compositor per seat the question is answered by which process started the app, so
the inference — and the tool — are gone.

Both children share one PipeWire daemon, since they run as the same user. That
is fine: it has many sinks, and each seat merely defaults to a different one.

### This is a default, not an isolation boundary

Audio can leak between seats, and the design does not prevent it:

- `PULSE_SINK` sets a *preferred* target. Any client may name a different sink
  through the API, and the user can drag a stream anywhere in `pavucontrol`.
- Both seats share one PipeWire daemon, so every node is visible to every
  client. There is no namespace or permission split.
- `PULSE_SINK` only reaches clients using the PulseAudio API. Native PipeWire
  clients are covered by the `seatmux.seat` tag and a WirePlumber rule, but
  anything that names its own target still wins over both — Kodi 21 did exactly
  that, having auto-selected the default output device.
- Apps that remember a device in their own config — browsers, games, Discord —
  will re-select it regardless of what they inherited.

In practice this is mild: the two sinks are different hardware, so a misrouted
stream is obvious and fixable. If it becomes an annoyance, the enforcement point
is WirePlumber access rules keyed on the seat's cgroup — which exists precisely
because seatmux gives each seat a distinct subtree. Running two PipeWire daemons
would give real isolation but means partitioning the sound hardware between them,
and would break the existing single-graph `mic-filter` and `soundboard` setup.

Worth stating plainly so nobody later assumes the seats are soundproof. They are
not; they are merely aimed correctly at startup.

## Session activation and VT

seatmux holds the VT. When logind deactivates the session, DRM master is dropped
and every lease dies with it. seatmux relays this to children as
`DISABLE_SEAT`, and on reactivation re-creates the leases and sends
`ENABLE_SEAT`. Children must therefore treat lease loss as recoverable rather
than fatal — the same path as a crash restart, so it is written once.

Neither child should bind VT-switch keys.

## Cgroups

Each seat gets its own cgroup, so one cannot starve or OOM the other:

```
user@<uid>.service/seatmux/
├── desk    the desk compositor and everything it spawns
└── tv      the TV compositor, Steam, games
```

**seatmux creates these itself.** Composing with `scoper` was tried and does not
work: `scoper` shells out to `systemd-run`, which asks the systemd *user manager*
to start the process, so the compositor lands in a different process tree and the
DRM lease fd cannot be inherited across that boundary. The lease is not
negotiable, so the cgroup work has to live wherever the fork happens.

No privilege is needed — systemd delegates `user@<uid>.service` to the user. But
seatmux must itself be *inside* that subtree: cgroup v2 only permits moving a
process between cgroups whose common ancestor you control, and a tty login sits
in `session-N.scope` under the root-owned `user-<uid>.slice`. Hence the launcher:

```
systemd-run --user --scope --unit=seatmux --property=Delegate=yes -- seatmux …
```

`--scope` is load-bearing. Unlike plain `systemd-run`, it forks the command from
the caller rather than handing it to the manager, so the lease fds seatmux later
passes its own children still work.

Processes inherit their parent's cgroup, so anything a compositor spawns stays in
its seat. Failing to set any of this up warns and carries on — losing isolation
is bad, being left without a desktop over it is worse.

## Startup

seatmux is the parent of both compositors — that is what makes them siblings
rather than one nesting the other. `wm` execs seatmux, and seatmux spawns nothing
until it owns what a seat needs.

1. Take control of the logind session and open `card1`. DRM master arrives with
   the fd; seatmux never calls `drmSetMaster` itself.
2. Bind one seatd socket per seat, so `SEATD_SOCK` exists before any child could
   connect to it.
3. Per seat, once its connectors are live: build the lease, clear `CLOEXEC` on
   the lease fd, and spawn the child with `WLR_DRM_LEASE_FD` naming that fd
   number, alongside `SEATD_SOCK`, `LIBSEAT_BACKEND=seatd` and the audio targets.
4. The child's wlroots reads `WLR_DRM_LEASE_FD`, skips GPU enumeration entirely,
   and builds its DRM backend on the inherited fd. Its libseat connects back over
   the socket for input devices, which seatmux grants or refuses by policy.

Both ordering constraints are hard. The socket must exist before the child runs,
and the lease fd must be inherited across `exec` — there is no way to hand it
over afterwards.

Because leases are lazy, a seat with no live connector simply has no compositor
yet. If the TV is off at boot there is no TV sway until it is switched on, at
which point the connector appears and the seat starts. That is intended, not a
degraded mode.

The reverse case does *not* stop the child: when the TV powers off mid-session
the lease is held, sway sees an output disappear and later reappear, and the seat
keeps its workspaces across the power cycle.

## Event loop

One thread, one `poll()` over the libseat fd, each seat's listening socket, and
each connected client, with a 500ms tick.

The tick does the work that would otherwise need a udev monitor and a signal
handler: re-read connector state to notice a display appearing, and
`try_wait()` each child to notice one exiting. Both are cheap — a handful of
ioctls — and half a second is imperceptible when switching on a TV. Trading
`calloop`, `udev` monitoring and `signalfd` for a periodic poll keeps the whole
daemon single-threaded and linear.

libseat's enable/disable arrives on a callback, which parks the change in a
`Cell` for the loop to pick up rather than acting inside the callback, so
activation is handled on the same path as everything else.

## Supervision

A child that exits has its lease torn down and rebuilt, then is respawned with
exponential backoff. seatmux exiting takes both children with it, since their
leases derive from its master fd.

## Child environment

| variable | purpose |
|---|---|
| `WLR_DRM_LEASE_FD` | inherited lease fd; makes wlroots skip GPU enumeration entirely |
| `LIBSEAT_BACKEND=seatd` | route device opens to seatmux |
| `SEATD_SOCK` | this seat's socket |
| `PULSE_SINK` / `PULSE_SOURCE` | audio targets |
| `WLR_LIBINPUT_NO_DEVICES=1` | start even with no input yet; devices arrive by hotplug |
| `WAYLAND_DISPLAY` / `DISPLAY` | *removed* — either one makes wlroots nest instead of using DRM |
| a seat's own `env` | whatever the config declares |

A seat's `env` is applied *before* everything above, so the table wins: a seat
cannot unset the lease fd or the seatd socket by declaring them.

`WLR_DRM_LEASE_FD` is the one patched behaviour, and it lives in `pkgs/wlroots`
(68 lines). Without it a child calls `wlr_session_find_gpus()` and opens *every*
card — the running sway currently holds master on both GPUs — and would fight
seatmux for `card1`.

## Config

```toml
[[seat]]
name       = "desk"
connectors = ["DP-2", "DP-1"]
exclude    = ["Logitech K400 Plus"]
sink       = "alsa_output.usb-Native_Instruments_Komplete_Audio_6_…"
source     = "mic-filter"
command    = ["…/sway", "-d"]

[seat.env]
TZ        = "America/Regina"
GTK_THEME = "Adwaita:dark"

[[seat]]
name       = "tv"
connectors = ["HDMI-A-2"]
include    = ["Logitech K400 Plus"]
sink       = "alsa_output.pci-0000_03_00.1.hdmi-stereo"
command    = ["…/sway", "-d", "-c", "…/sway-tv.conf"]
```

Devices match on any identity udev reports — the evdev name, `ID_PATH`, or
`vendor:product`. Prefer the **evdev name**: it survives replugging, and it is
the only one that distinguishes devices sharing a Logitech Unifying receiver.

The asymmetry is deliberate. The desk excludes, so new hardware at the desk needs
no config change; the TV includes, so nothing drifts onto it by accident. The TV
seat has no `source` — there is no microphone at the couch.

Commands are the compositor directly, with no `scoper` wrapper: see *Cgroups*.

`env` is per seat and optional. It exists because session settings — timezone,
GTK theme, editor — used to ride on the `wm` launcher, which seatmux bypasses by
forking compositors itself. Both seats currently declare the same values; the
point of the table is that they need not.

Connector names, device names and sink names are instance specifics and live in
`desktop-home`, fed from `desktop-devices` like the rest of the machine's
identity. The package itself stays generic.

## This machine

```
card1  0000:03:00.0  4 CRTCs [432 437 442 447]  11 planes
  DP-1      449  LG Electronics LG ULTRAFINE 112NTFA27619   3840x2160@60
  DP-2      459  Ancor Communications Inc ASUS MG279 0x00025AF7  2560x1440@144
  HDMI-A-1  466  disconnected
  HDMI-A-2  473  Technical Concepts Ltd 55R617CA Unknown   3840x2160@60 (tv)
card0  0000:76:00.0  iGPU, unused by this design
```

The TV prefers 4K30 in its EDID, so the mode is set explicitly. It also needs
its HDMI port on the *Standard* bandwidth mode; in the other one the panel
reports no connection at 4K60 while the kernel still reads EDID over DDC, which
looks exactly like a software fault and is not one.

## Things that cost time to find

Ordered roughly by how long each one hid.

- **`DRM_CLIENT_CAP_UNIVERSAL_PLANES` must be set before enumerating planes.**
  Without it the kernel reports only *overlay* planes, so every CRTC looks like
  it has no primary and no lease can be built. `Card::new` sets it, along with
  `Atomic`, before anything reads the card.
- **`systemd-run` breaks fd inheritance.** It asks the systemd *user manager* to
  start the process, which lands in a different process tree — a lease fd cannot
  follow. Compositors must be forked by seatmux directly. This is why cgroup
  placement could not be delegated to `scoper`.
- **`WAYLAND_DISPLAY` or `DISPLAY` in a child's environment silently defeats the
  whole design.** `wlr_backend_autocreate` picks a nested Wayland or X11 backend
  and never reaches DRM, with no error. `child::spawn` clears both.
- **Connectors read `Unknown` until probed.** After boot nothing may have probed
  yet, and an unprobed connector is indistinguishable from a disconnected one —
  so every seat looks absent forever. `resources(probe: true)` forces it while
  any seat is waiting to start.
- **A seat with no input devices aborts the compositor.** wlroots treats zero
  devices as fatal unless `WLR_LIBINPUT_NO_DEVICES=1`. A display with no keyboard
  is still worth having, and the keyboard arrives later by hotplug.
- **`vendor:product` is useless for anything on a Logitech Unifying receiver.**
  udev reports the *receiver's* USB id, identical for every device paired to it —
  the couch keyboard and the desk mouse read the same `046d:c52b`. udev exposes
  no `NAME` for these nodes either (`ID_MODEL` is `USB_Receiver`). The evdev name
  from sysfs is the only thing that tells them apart, and it survives moving the
  receiver between ports, which `ID_PATH` does not.
- **`/dev/input/event*` is `root:input 0660` with no ACL entries**, and the user
  is not in `input`. Compositors reach input only because logind opens it for
  them — which is why children cannot use `LIBSEAT_BACKEND=builtin`, and why
  seatmux must pass the fds. Force `LIBSEAT_BACKEND=logind` for seatmux itself:
  falling through to `builtin` leaves every seat with no keyboard or mouse.
- **cgroup v2 only allows moving a process between cgroups whose common ancestor
  you control.** A tty login sits in `session-N.scope` under the root-owned
  `user-<uid>.slice`, so seatmux must be launched inside the delegated
  `user@<uid>.service` subtree to create per-seat cgroups.
- **wlroots never calls `drmSetMaster`.** The only master call in the backend is
  a `drmDropMaster` in `wlr_drm_backend_get_non_master_fd()`. logind takes master
  when it opens the device, so a lessee needs nothing suppressed.
- **`wlr_drm_backend_create()` derives `drm->name` from
  `drmGetDeviceNameFromFd2(dev->fd)`**, which resolves to the underlying node on
  a lease fd — so the client-fd path needs no special casing.
- **sway opens every GPU it finds**, not just the one it displays on. A lessee
  must skip enumeration entirely or it fights seatmux for the leased card.
- **DRM render nodes are `0666`; card nodes are `0660`.** Rendering was never the
  restricted part — only KMS.

## Debugging

Taking control of the session puts the VT into graphics mode and turns off
kernel keyboard handling. A seatmux that runs but starts nothing therefore
leaves a blank, unresponsive console — Ctrl+C never becomes a signal, and the
only way out is a hard reset. Console stderr is worthless there, so the launcher
pipes everything through `systemd-cat`:

```
journalctl -t seatmux -b        # this boot
journalctl -t seatmux -b -1     # after a hard reset
```

Both compositors run with `-d`, so their wlroots output lands under the same
tag. The line confirming the shim is working:

```
Using leased DRM fd 9, skipping GPU enumeration
```

seatmux reports what it is waiting for whenever no seat is ready, once per
change, because a seat that never becomes ready is otherwise indistinguishable
from a hang.

## Control socket

`seatmux status` and `seatmux stop` connect to `$XDG_RUNTIME_DIR/seatmux/control.sock`,
which is bound alongside the seat sockets and joins the same `poll()`. It is
deliberately not the seatd socket: that one speaks libseat's protocol to child
compositors, where an unexpected message desynchronises the client.

The protocol is one word in, one report out, connection closed. The client
shuts down its write side so the server can read to EOF, which is also why an
unrecognised word is answered and dropped rather than left waiting.

`stop` sends SIGTERM to every compositor and waits up to five seconds for them,
escalating to SIGKILL. Waiting is the point: a lease is released by the child
exiting, so seatmux must not drop the card first.

The subcommand is never optional. A bare `seatmux`, or a config path on its own,
prints usage and takes nothing: seizing the session is worth having to ask for
by name.

## Status

Running. Both seats come up, each driving its own connectors on one GPU, with
input routed by policy, per-seat cgroups, and hotplug handled on both sides.

Known-good on this machine: the desk on DP-1 + DP-2, the TV on HDMI-A-2 at
4K60, the K400 reaching only the TV and the MX Master only the desk, and the TV
surviving repeated power cycles — the connector drops, the CRTC is de-allocated,
and a modeset is requested when it returns.

Not yet exercised: lease revoke and reissue across a VT switch, and TV audio
over HDMI while the display side is leased (the codec is a separate PCI
function, `0000:03:00.1`, so it should be indifferent to who holds DRM master,
but ELD population comes from the display driver).

The TV needs its HDMI port set to the *Standard* bandwidth mode; at 4K60 in the
other mode the panel reports no connection while the kernel still reads EDID
over DDC, which reads as a software fault and is not one.
