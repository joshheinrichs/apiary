//! seatmux — split one logind session into several independent seats.
//!
//! seatmux holds the machine's only session and hands each child a complete
//! seat: a DRM lease for its connectors, input devices it is allowed to open,
//! and its audio targets. It never composites and is not in any frame path.

mod card;
mod child;
mod config;
mod control;
mod device;
mod lease;
mod proto;
mod server;

use anyhow::{Context, Result, bail};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::cell::Cell;
use std::collections::HashSet;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::card::Card;
use crate::config::{Config, Seat};
use crate::control::{Command, Control};
use crate::lease::{Resources, plan_leases, ready_seats};
use crate::proto::Event;
use crate::server::{Client, Listener};

const USAGE: &str = "usage: seatmux start <config.toml> [card] | seatmux status | seatmux stop";
const DEFAULT_CARD: &str = "/dev/dri/card1";

/// How long a compositor gets to leave on its own after SIGTERM before the
/// stop turns into SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// How often to re-read connector state and reap children. Fast enough that
/// switching the TV on feels immediate, cheap enough to run forever.
const TICK: Duration = Duration::from_millis(500);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(8);

struct SeatRuntime {
    config: Seat,
    listener: Listener,
    client: Option<Client>,
    child: Option<child::Spawned>,
    backoff: Duration,
    retry_at: Instant,
}

impl SeatRuntime {
    fn running(&self) -> bool {
        self.child.is_some()
    }
}

/// What the command line asked for. The subcommand is always required: taking
/// the session is not something to do by accident from a bare `seatmux`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Invocation {
    Start { config: String, card: String },
    Control(Command),
}

fn parse_args(args: &[String]) -> Result<Invocation> {
    let (first, rest) = args.split_first().context(USAGE)?;

    if let Some(command) = Command::parse(first) {
        if !rest.is_empty() {
            bail!("{USAGE}");
        }
        return Ok(Invocation::Control(command));
    }

    if first != "start" {
        bail!("{USAGE}");
    }

    let (config, tail) = rest.split_first().context(USAGE)?;
    if tail.len() > 1 {
        bail!("{USAGE}");
    }
    Ok(Invocation::Start {
        config: config.clone(),
        card: tail
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_CARD.to_string()),
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args)? {
        Invocation::Control(command) => {
            print!("{}", control::send(&runtime_dir()?, command)?);
            Ok(())
        }
        Invocation::Start { config, card } => start(config, card),
    }
}

fn start(config_path: String, card_path: String) -> Result<()> {
    let text =
        std::fs::read_to_string(&config_path).with_context(|| format!("reading {config_path}"))?;
    let config = Config::parse(&text)?;

    log("config parsed, taking control of the session");

    // Enable/Disable arrive on a libseat callback, so park them somewhere the
    // loop can pick them up rather than acting inside the callback.
    let activation: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    let signal = activation.clone();
    let mut seat = libseat::Seat::open(move |_, event| {
        signal.set(Some(matches!(event, libseat::SeatEvent::Enable)));
    })
    .map_err(|e| anyhow::anyhow!("taking control of the logind session: {e}"))?;

    log("session taken, opening the card");

    // DRM master arrives with the fd; seatmux never calls drmSetMaster itself.
    let card_device = seat
        .open_device(&Path::new(&card_path))
        .map_err(|e| anyhow::anyhow!("opening {card_path}: {e}"))?;
    let card = Card::new(card_device)?;

    log("card opened, binding seat sockets");

    let runtime_dir = runtime_dir()?;
    let mut seats: Vec<SeatRuntime> = config
        .seats
        .into_iter()
        .map(|config| {
            let listener = Listener::bind(&runtime_dir, &config.name)?;
            Ok(SeatRuntime {
                config,
                listener,
                client: None,
                child: None,
                backoff: BACKOFF_MIN,
                retry_at: Instant::now(),
            })
        })
        .collect::<Result<_>>()?;

    let control = Control::bind(&runtime_dir)?;

    // Captured once: libinput enumerates by this name, and borrowing the seat
    // mutably later would conflict with passing it around.
    let udev_seat = seat.name().to_string();

    log(&format!(
        "ready: libseat seat {udev_seat}, card {card_path}, seats: {}",
        seats
            .iter()
            .map(|s| s.config.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let mut active = true;
    let mut last_report = String::new();
    loop {
        wait(&mut seat, &mut seats, &control)?;

        seat.dispatch(0)
            .map_err(|e| anyhow::anyhow!("libseat dispatch: {e}"))?;

        if let Some(enabled) = activation.take() {
            active = enabled;
            relay_activation(&mut seats, enabled);
        }

        if let Some((mut stream, command)) = control.accept()? {
            use std::io::Write;
            match command {
                Command::Status => {
                    let _ = write!(stream, "{}", status(&seats, &card_path, &udev_seat, active));
                }
                Command::Stop => {
                    let _ = writeln!(stream, "stopping {} seat(s)", seats.len());
                    drop(stream);
                    return shutdown(&mut seats, &mut seat);
                }
            }
        }

        accept_clients(&mut seats)?;
        pump_clients(&mut seats, &mut seat, &udev_seat, active)?;
        reap(&mut seats, &mut seat);

        if active {
            start_ready_seats(&card, &mut seats, &mut last_report)?;
        }
    }
}

/// Block until something needs attention, or the tick expires so connector
/// state and child exits get looked at.
fn wait(seat: &mut libseat::Seat, seats: &mut [SeatRuntime], control: &Control) -> Result<()> {
    let seat_fd = seat
        .get_fd()
        .map_err(|e| anyhow::anyhow!("libseat fd: {e}"))?;

    let mut fds = vec![
        PollFd::new(seat_fd, PollFlags::POLLIN),
        PollFd::new(control.as_fd(), PollFlags::POLLIN),
    ];
    for runtime in seats.iter() {
        fds.push(PollFd::new(
            runtime.listener.listener.as_fd(),
            PollFlags::POLLIN,
        ));
    }
    // Borrowed separately so the client borrows do not overlap the listeners.
    let clients: Vec<_> = seats.iter().filter_map(|s| s.client.as_ref()).collect();
    for client in &clients {
        fds.push(PollFd::new(client.as_fd(), PollFlags::POLLIN));
    }

    let timeout = PollTimeout::try_from(TICK.as_millis() as u16).unwrap_or(PollTimeout::MAX);
    match poll(&mut fds, timeout) {
        Ok(_) => Ok(()),
        Err(nix::errno::Errno::EINTR) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn accept_clients(seats: &mut [SeatRuntime]) -> Result<()> {
    for runtime in seats.iter_mut() {
        if runtime.client.is_some() {
            continue;
        }
        match runtime.listener.listener.accept() {
            Ok((stream, _)) => runtime.client = Some(Client::new(stream)?),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn pump_clients(
    seats: &mut [SeatRuntime],
    seat: &mut libseat::Seat,
    udev_seat: &str,
    active: bool,
) -> Result<()> {
    for runtime in seats.iter_mut() {
        let Some(client) = runtime.client.as_mut() else {
            continue;
        };
        let alive = match client.pump(&runtime.config, seat, udev_seat, active) {
            Ok(alive) => alive,
            Err(e) => {
                log(&format!("seat '{}': {e:#}", runtime.config.name));
                false
            }
        };
        if !alive {
            client.release_all(seat);
            runtime.client = None;
        }
    }
    Ok(())
}

fn relay_activation(seats: &mut [SeatRuntime], enabled: bool) {
    let event = if enabled {
        Event::EnableSeat
    } else {
        Event::DisableSeat
    };
    for runtime in seats.iter_mut() {
        if let Some(client) = runtime.client.as_mut() {
            let _ = client.send_event(event);
        }
    }
}

/// What `seatmux status` prints. One line for the session, one per seat.
fn status(seats: &[SeatRuntime], card: &str, udev_seat: &str, active: bool) -> String {
    let mut out = format!(
        "card {card}, libseat seat {udev_seat}, session {}\n",
        if active { "active" } else { "inactive" }
    );

    for runtime in seats {
        let state = match runtime.child.as_ref() {
            Some(spawned) => format!("running pid {}", spawned.process.id()),
            None => {
                let wait = runtime.retry_at.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    "stopped".to_string()
                } else {
                    format!("stopped, retry in {:.1}s", wait.as_secs_f32())
                }
            }
        };
        out.push_str(&format!(
            "  {:8} {state}, connectors {}, compositor {}\n",
            runtime.config.name,
            runtime.config.connectors.join(" "),
            if runtime.client.is_some() {
                "attached"
            } else {
                "not attached"
            },
        ));
    }
    out
}

/// Stop every compositor and leave. Waiting for them matters: a lease is
/// released by the child exiting, and the leases have to be gone before seatmux
/// drops the card.
fn shutdown(seats: &mut [SeatRuntime], seat: &mut libseat::Seat) -> Result<()> {
    signal_children(seats, Signal::SIGTERM);

    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        reap(seats, seat);
        if seats.iter().all(|runtime| !runtime.running()) {
            log("all seats stopped");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    log("seats still running after the grace period, killing");
    signal_children(seats, Signal::SIGKILL);
    reap(seats, seat);
    Ok(())
}

fn signal_children(seats: &mut [SeatRuntime], sig: Signal) {
    for runtime in seats.iter() {
        let Some(spawned) = runtime.child.as_ref() else {
            continue;
        };
        let pid = Pid::from_raw(spawned.process.id() as i32);
        log(&format!(
            "sending {sig} to seat '{}' (pid {pid})",
            runtime.config.name
        ));
        let _ = signal::kill(pid, sig);
    }
}

/// A child that has exited takes its lease with it, so the objects return to
/// seatmux and the seat can be rebuilt from scratch.
fn reap(seats: &mut [SeatRuntime], seat: &mut libseat::Seat) {
    for runtime in seats.iter_mut() {
        let Some(spawned) = runtime.child.as_mut() else {
            continue;
        };
        match spawned.process.try_wait() {
            Ok(Some(status)) => {
                log(&format!("seat '{}' exited: {status}", runtime.config.name));
                if let Some(client) = runtime.client.as_mut() {
                    client.release_all(seat);
                }
                runtime.client = None;
                runtime.child = None;
                runtime.retry_at = Instant::now() + runtime.backoff;
                runtime.backoff = (runtime.backoff * 2).min(BACKOFF_MAX);
            }
            Ok(None) => {}
            Err(e) => log(&format!("seat '{}': {e}", runtime.config.name)),
        }
    }
}

/// Build leases for seats whose connectors are present and that are not already
/// running, then spawn them.
///
/// Leases are lazy on purpose: a seat whose display is off simply has no
/// compositor until the connector appears.
fn start_ready_seats(
    card: &Card<libseat::Device>,
    seats: &mut [SeatRuntime],
    last_report: &mut String,
) -> Result<()> {
    let now = Instant::now();
    let wanted: Vec<usize> = seats
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.running() && now >= s.retry_at)
        .map(|(i, _)| i)
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }

    // Probe: after boot an untouched connector reports Unknown, which would
    // silently look like "nothing plugged in" forever. A forced probe reads
    // EDID over DDC, which is not free, so say when it is slow enough to
    // matter, since it runs on every tick until a seat starts.
    let probe_started = Instant::now();
    let resources = card.resources(true)?;
    if probe_started.elapsed() > Duration::from_millis(250) {
        log(&format!(
            "connector probe took {:?}",
            probe_started.elapsed()
        ));
    }
    // Objects already out on a lease must not be planned twice; the kernel
    // would refuse, and a running seat would lose its scanout.
    let taken = leased_objects(seats);
    let free = without(&resources, &taken);

    let candidates: Vec<Seat> = wanted.iter().map(|i| seats[*i].config.clone()).collect();
    let ready = ready_seats(&free, &candidates);
    if ready.is_empty() {
        // Report what is being waited on, once per change. A seat that never
        // becomes ready is otherwise indistinguishable from a hang.
        let seen: Vec<String> = free
            .connectors
            .iter()
            .map(|c| {
                format!(
                    "{}={}",
                    c.name,
                    if c.connected { "connected" } else { "no" }
                )
            })
            .collect();
        let report = format!(
            "waiting for {}; connectors: {}",
            candidates
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            seen.join(" ")
        );
        if *last_report != report {
            log(&report);
            *last_report = report;
        }
        return Ok(());
    }
    last_report.clear();

    let plans = match plan_leases(&free, &ready) {
        Ok(plans) => plans,
        Err(e) => {
            log(&format!("cannot plan leases: {e:#}"));
            return Ok(());
        }
    };

    for plan in plans {
        let Some(index) = seats.iter().position(|s| s.config.name == plan.seat) else {
            continue;
        };
        let lease = match card.lease(&plan) {
            Ok(lease) => lease,
            Err(e) => {
                log(&format!("seat '{}': {e:#}", plan.seat));
                seats[index].retry_at = Instant::now() + seats[index].backoff;
                seats[index].backoff = (seats[index].backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };

        let socket = seats[index].listener.path.clone();
        match child::spawn(&seats[index].config, lease.fd, &socket) {
            Ok(spawned) => {
                log(&format!(
                    "seat '{}' started on {:?}",
                    plan.seat, plan.connectors
                ));
                seats[index].child = Some(spawned);
                seats[index].backoff = BACKOFF_MIN;
            }
            Err(e) => {
                log(&format!("seat '{}': {e:#}", plan.seat));
                seats[index].retry_at = Instant::now() + seats[index].backoff;
                seats[index].backoff = (seats[index].backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
    Ok(())
}

fn leased_objects(seats: &[SeatRuntime]) -> HashSet<u32> {
    let mut taken = HashSet::new();
    for runtime in seats.iter() {
        let Some(spawned) = runtime.child.as_ref() else {
            continue;
        };
        if let Ok(resources) = drm::control::get_lease(&spawned.lease) {
            taken.extend(resources.crtcs.iter().map(|h| u32::from(*h)));
            taken.extend(resources.planes.iter().map(|h| u32::from(*h)));
            taken.extend(resources.connectors.iter().map(|h| u32::from(*h)));
        }
    }
    taken
}

fn without(resources: &Resources, taken: &HashSet<u32>) -> Resources {
    Resources {
        crtcs: resources
            .crtcs
            .iter()
            .filter(|c| !taken.contains(&c.id))
            .cloned()
            .collect(),
        planes: resources
            .planes
            .iter()
            .filter(|p| !taken.contains(&p.id))
            .cloned()
            .collect(),
        connectors: resources
            .connectors
            .iter()
            .filter(|c| !taken.contains(&c.id))
            .cloned()
            .collect(),
    }
}

fn runtime_dir() -> Result<PathBuf> {
    let Ok(base) = std::env::var("XDG_RUNTIME_DIR") else {
        bail!("XDG_RUNTIME_DIR is not set");
    };
    Ok(PathBuf::from(base).join("seatmux"))
}

/// Unbuffered so a line is on its way out before the next thing can hang.
/// seatmux runs from a TTY that goes dark the moment the session is taken, so
/// these only ever get read out of the journal.
fn log(message: &str) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "seatmux: {message}");
    let _ = err.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn starting_takes_a_config_and_defaults_the_card() {
        assert_eq!(
            parse_args(&args(&["start", "seatmux.toml"])).unwrap(),
            Invocation::Start {
                config: "seatmux.toml".into(),
                card: DEFAULT_CARD.into()
            }
        );
    }

    #[test]
    fn the_card_is_optional_and_overridable() {
        let Invocation::Start { card, .. } =
            parse_args(&args(&["start", "seatmux.toml", "/dev/dri/card0"])).unwrap()
        else {
            panic!("expected a start");
        };
        assert_eq!(card, "/dev/dri/card0");
    }

    #[test]
    fn control_words_are_commands_rather_than_config_paths() {
        assert_eq!(
            parse_args(&args(&["status"])).unwrap(),
            Invocation::Control(Command::Status)
        );
        assert_eq!(
            parse_args(&args(&["stop"])).unwrap(),
            Invocation::Control(Command::Stop)
        );
    }

    /// The session is taken only when it is asked for by name. A bare `seatmux`,
    /// or a config path on its own, must not start anything.
    #[test]
    fn nothing_starts_without_the_subcommand() {
        for words in [
            vec![],
            vec!["seatmux.toml"],
            vec!["seatmux.toml", "/dev/dri/card0"],
        ] {
            assert!(
                parse_args(&args(&words)).is_err(),
                "{words:?} should not start seatmux"
            );
        }
    }

    #[test]
    fn too_much_to_do_is_refused() {
        for words in [
            vec!["stop", "now"],
            vec!["start", "a.toml", "card", "extra"],
            vec!["start"],
        ] {
            assert!(
                parse_args(&args(&words)).is_err(),
                "{words:?} should not parse"
            );
        }
    }
}
