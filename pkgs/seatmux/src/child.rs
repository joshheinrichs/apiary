//! Spawning a seat's compositor.
//!
//! Two things must be true before `exec`: the seat's socket exists, and the
//! lease fd is inherited. Neither can be arranged afterwards — a lease cannot be
//! handed to a running process, which is why the lease is built first and the
//! child is spawned around it.

use anyhow::{Context, Result};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};

use crate::config::Seat;

/// Baked at build time, same as scoper does it.
const SYSTEMD_RUN: &str = match option_env!("SYSTEMD_RUN") {
    Some(path) => path,
    None => "systemd-run",
};

pub struct Spawned {
    pub process: Child,
    /// Held for the child's lifetime. Dropping it revokes the lease, which is
    /// what makes a restart self-cleaning.
    pub lease: OwnedFd,
}

/// Launch a seat's compositor with its lease, socket and audio targets.
pub fn spawn(seat: &Seat, lease: OwnedFd, socket: &Path) -> Result<Spawned> {
    let (program, args) = seat
        .command
        .split_first()
        .context("seat command is empty")?;

    // Run each compositor as a transient scope in its own slice, so systemd owns
    // the hierarchy and anything launched from a seat -- by scoper, fuzzel or
    // anything else that derives its slice from /proc/self/cgroup -- nests under
    // that seat automatically.
    //
    // `--scope` forks from here rather than from the user manager, so both the
    // inherited lease fd and this environment survive.
    let mut command = Command::new(SYSTEMD_RUN);
    command.args([
        "--user",
        "--scope",
        "--quiet",
        &format!("--slice=seatmux-{}.slice", seat.name),
        &format!("--unit=seatmux-{}-compositor", seat.name),
        "--",
    ]);
    command.arg(program).args(args);

    // The seat's own environment goes on first: everything seatmux sets below is
    // load-bearing for the lease, and a seat must not be able to override it.
    command.envs(&seat.env);

    // wlr_backend_autocreate picks a nested Wayland or X11 backend when these
    // are set, and never reaches the DRM path. A stale value inherited from an
    // earlier session is enough to leave the monitors untouched with no error.
    command.env_remove("WAYLAND_DISPLAY");
    command.env_remove("DISPLAY");

    // A seat with a display but no keyboard is still worth having: the TV shows
    // a picture, and its keyboard is routed in by hotplug whenever it is
    // switched on. Without this wlroots aborts outright when a seat starts with
    // no input devices.
    command.env("WLR_LIBINPUT_NO_DEVICES", "1");

    command.env("LIBSEAT_BACKEND", "seatd");
    command.env("SEATD_SOCK", socket);
    // wlroots reads this and skips GPU enumeration entirely. Without it a child
    // opens every card on the system and fights seatmux for the leased one.
    command.env("WLR_DRM_LEASE_FD", lease.as_raw_fd().to_string());

    if let Some(sink) = &seat.sink {
        command.env("PULSE_SINK", sink);
    }
    if let Some(source) = &seat.source {
        command.env("PULSE_SOURCE", source);
    }

    // PULSE_SINK/PULSE_SOURCE only reach PulseAudio clients; a native PipeWire
    // client (Kodi 21's sink, for one) never looks at them. PIPEWIRE_PROPS is
    // read by libpipewire itself, so the same targets go out again here.
    //
    // The target is in the environment rather than left to a session-manager
    // rule on the seat tag: a rule only applies once the session manager has
    // reloaded, and routing a seat's own audio should not wait on that. The tag
    // rides along for rules that need the direction a single target cannot
    // express -- a seat's source, for one.
    let mut props = format!("{{ seatmux.seat = \"{}\"", seat.name);
    if let Some(sink) = &seat.sink {
        props.push_str(&format!(
            " target.object = \"{sink}\" node.dont-fallback = true"
        ));
    }
    props.push_str(" }");
    command.env("PIPEWIRE_PROPS", props);

    // Rust sets CLOEXEC on everything it opens, so the lease would vanish across
    // exec unless it is cleared in the child between fork and exec.
    let raw = lease.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            let flags = nix::libc::fcntl(raw, nix::libc::F_GETFD);
            if flags < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if nix::libc::fcntl(raw, nix::libc::F_SETFD, flags & !nix::libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let process = command
        .spawn()
        .with_context(|| format!("spawning seat '{}': {program}", seat.name))?;

    Ok(Spawned { process, lease })
}
