//! Flashes M-Vave FM-1 firmware from Linux, where the vendor ships a Windows
//! and macOS tool only.
//!
//! `ports` and `info` are read-only and prove the wire format against real
//! hardware without writing anything. `firmware flash` is the irreversible
//! one: the board has no recovery buttons or debug pads, so every check that
//! can refuse runs before the first byte goes out.

mod codec;
mod firmware;
mod midi;
mod ota;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};

use codec::{IDENTIFY_REPLY_LEN, Identity, Packet, TYPE_IDENTIFY};

const IDENTIFY_TIMEOUT: Duration = Duration::from_millis(1000);
const IDENTIFY_ATTEMPTS: usize = 3;

#[derive(Parser)]
#[command(about = "Talk to an M-Vave FM-1 over USB MIDI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List rawmidi ports.
    Ports,
    /// Ask the device what it is and compare it against the bundled firmware.
    Info {
        /// ALSA rawmidi port, e.g. hw:4,0,0. Guessed from the port list if
        /// omitted.
        #[arg(long)]
        port: Option<String>,
    },
    /// Work with firmware images.
    Firmware {
        #[command(subcommand)]
        command: Option<FirmwareCommand>,
    },
}

#[derive(Subcommand)]
enum FirmwareCommand {
    /// List stored images. The default.
    List,
    /// Download the current release from the vendor and store it.
    Fetch,
    /// Write an image to the device. Irreversible.
    Flash {
        /// ALSA rawmidi port. Guessed from the port list if omitted.
        #[arg(long)]
        port: Option<String>,
        /// Image to write. The newest stored one if omitted.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Allow writing an older version than the device runs.
        #[arg(long)]
        allow_downgrade: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Ports => ports(),
        Command::Info { port } => info(port),
        Command::Firmware { command } => match command.unwrap_or(FirmwareCommand::List) {
            FirmwareCommand::List => list(),
            FirmwareCommand::Fetch => fetch(),
            FirmwareCommand::Flash {
                port,
                file,
                allow_downgrade,
                yes,
            } => flash(port, file, allow_downgrade, yes),
        },
    }
}

fn ports() -> Result<()> {
    let ports = midi::ports()?;
    if ports.is_empty() {
        eprintln!("no rawmidi ports found");
        return Ok(());
    }
    for port in &ports {
        println!("{:<12} {} -- {}", port.hw(), port.card_name, port.name);
    }
    Ok(())
}

fn fetch() -> Result<()> {
    let (image, published) = firmware::fetch()?;
    let (path, is_new) = firmware::store(&image)?;
    println!("{:<14}{}", "downloaded", image.label());
    println!("{:<14}{} bytes", "size", image.bytes.len());
    println!("{:<14}{}", "sha256", image.sha256);
    if let Some(published) = published {
        println!("{:<14}{published}", "published");
    }
    println!(
        "{:<14}{} ({})",
        "stored",
        path.display(),
        if is_new { "new" } else { "already had it" }
    );
    Ok(())
}

fn list() -> Result<()> {
    let stored = firmware::stored()?;
    if stored.is_empty() {
        eprintln!("no firmware stored in {}", firmware::dir()?.display());
        eprintln!("run `fm1ctl firmware fetch` to download the current release");
        return Ok(());
    }
    for (path, image) in &stored {
        println!(
            "{:<12} {:>8} bytes  {}  {}",
            image.label(),
            image.bytes.len(),
            &image.sha256[..16],
            path.display()
        );
    }
    Ok(())
}

fn flash(
    port: Option<String>,
    file: Option<PathBuf>,
    allow_downgrade: bool,
    yes: bool,
) -> Result<()> {
    let image = match file {
        Some(path) => {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            firmware::Image::parse(bytes)
                .with_context(|| format!("{} is not a usable firmware container", path.display()))?
        }
        None => {
            let (path, image) = firmware::best()?.context(
                "no firmware stored -- run `fm1ctl firmware fetch` to download the current release",
            )?;
            eprintln!("using {}", path.display());
            image
        }
    };

    let hw = resolve_port(port)?;
    let mut link = midi::Link::open(&hw)?;
    let id = identify(&mut link)?;

    // Everything that could refuse has to refuse before the first write.
    ensure!(
        id.name == image.name,
        "device says it is a {} but the image is for a {} -- refusing",
        id.name,
        image.name
    );
    ensure!(
        id.version != image.version,
        "device already runs v{} -- nothing to do",
        id.version
    );
    if image.version < id.version && !allow_downgrade {
        bail!(
            "image is v{} but the device runs v{} -- pass --allow-downgrade to go backwards",
            image.version,
            id.version
        );
    }

    eprintln!();
    eprintln!("  device    {} v{} on {hw}", id.name, id.version);
    eprintln!(
        "  writing   {} ({} bytes)",
        image.label(),
        image.bytes.len()
    );
    eprintln!("  sha256    {}", image.sha256);
    eprintln!();
    eprintln!("This cannot be undone, and the board has no recovery button. Do not");
    eprintln!("unplug it or let the machine sleep until it reports success.");
    eprintln!();
    if !yes {
        confirm()?;
    }

    ota::run(&hw, &image.bytes)
}

/// Typing the whole word, because y/n is too easy to do by reflex.
fn confirm() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal -- pass --yes to flash without confirmation");
    }
    eprint!("Type 'flash' to continue: ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading confirmation")?;
    ensure!(answer.trim() == "flash", "cancelled");
    Ok(())
}

/// Anything whose card or port name looks like the device. The FM-1 shows up
/// as "FM-1 MIDI 1" in normal mode and "USB-Midi" once it is in OTA mode, so
/// neither name alone is enough to go on.
pub fn guess_port(ports: &[midi::Port]) -> Option<&midi::Port> {
    let looks_right = |p: &&midi::Port| {
        let haystack = format!("{} {}", p.card_name, p.name).to_lowercase();
        ["fm-1", "fm1", "usb-midi", "usb midi"]
            .iter()
            .any(|needle| haystack.contains(needle))
    };
    ports.iter().find(looks_right)
}

fn resolve_port(port: Option<String>) -> Result<String> {
    if let Some(hw) = port {
        return Ok(hw);
    }
    let ports = midi::ports()?;
    let guess = guess_port(&ports)
        .context("could not guess which port the FM-1 is on; run `fm1ctl ports` and pass --port")?;
    eprintln!("using {} ({})", guess.hw(), guess.name);
    Ok(guess.hw())
}

/// Query until the device answers, and return what it says it is.
fn identify(link: &mut midi::Link) -> Result<Identity> {
    let query = Packet::query(TYPE_IDENTIFY);

    for attempt in 1..=IDENTIFY_ATTEMPTS {
        eprintln!("identify attempt {attempt}/{IDENTIFY_ATTEMPTS}...");
        link.drain()?;
        link.send(&query)?;

        match link.recv(IDENTIFY_TIMEOUT) {
            Ok(reply) => return interpret(&reply),
            Err(e) => eprintln!("  {e}"),
        }
    }

    bail!("device did not identify itself after {IDENTIFY_ATTEMPTS} attempts");
}

fn interpret(reply: &Packet) -> Result<Identity> {
    if reply.kind != TYPE_IDENTIFY {
        eprintln!(
            "note: reply type is {:#04x}, expected {TYPE_IDENTIFY:#04x}",
            reply.kind
        );
    }
    let total = 7 + reply.payload.len();
    if total != IDENTIFY_REPLY_LEN {
        eprintln!("note: packet is {total} bytes, vendor tool expects {IDENTIFY_REPLY_LEN}");
    }
    Identity::parse(&reply.payload).context("reading an identity out of the reply")
}

fn info(port: Option<String>) -> Result<()> {
    let hw = resolve_port(port)?;
    let mut link = midi::Link::open(&hw)?;
    let id = identify(&mut link)?;

    println!("device        {}", id.name);
    println!("firmware      v{}", id.version);
    match firmware::best()? {
        Some((_, image)) => {
            println!("available     {}", image.label());
            println!("              {}", verdict(id.version, image.version));
        }
        None => println!("available     nothing (`fm1ctl firmware fetch`)"),
    }
    Ok(())
}

/// The vendor tool refuses to flash a version the device already runs, so the
/// interesting states are "behind" and "ahead", not just "different".
fn verdict(device: u32, available: u32) -> &'static str {
    match device.cmp(&available) {
        std::cmp::Ordering::Equal => "up to date",
        std::cmp::Ordering::Less => "update available",
        std::cmp::Ordering::Greater => "device is newer than the stored firmware",
    }
}
