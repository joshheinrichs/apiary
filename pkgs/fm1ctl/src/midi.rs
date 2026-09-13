//! ALSA rawmidi link to the device. All of the I/O in this crate lives here.
//!
//! rawmidi rather than the sequencer: the protocol is a `F0`..`F7` byte stream
//! either way, and rawmidi hands it over without the sequencer's event
//! chunking in the middle.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use alsa::rawmidi::Rawmidi;
use alsa::{Direction, card};
use anyhow::{Context, Result, bail};

use crate::codec::{self, Packet};

#[derive(Debug, Clone)]
pub struct Port {
    pub card: i32,
    pub device: i32,
    pub sub: i32,
    pub card_name: String,
    pub name: String,
}

impl Port {
    pub fn hw(&self) -> String {
        format!("hw:{},{},{}", self.card, self.device, self.sub)
    }
}

/// Every rawmidi playback port on the system, in card order.
pub fn ports() -> Result<Vec<Port>> {
    let mut found = Vec::new();

    for card in card::Iter::new() {
        let card = card.context("enumerating sound cards")?;
        let index = card.get_index();
        let card_name = card.get_name().unwrap_or_else(|_| format!("card {index}"));

        let ctl = alsa::Ctl::from_card(&card, false)
            .with_context(|| format!("opening control interface for card {index}"))?;

        for info in alsa::rawmidi::Iter::new(&ctl) {
            let info = info.context("enumerating rawmidi devices")?;
            if info.get_stream() != Direction::Playback {
                continue;
            }
            found.push(Port {
                card: index,
                device: info.get_device(),
                sub: info.get_subdevice(),
                card_name: card_name.clone(),
                name: info.get_subdevice_name().unwrap_or_default(),
            });
        }
    }

    Ok(found)
}

/// "No data yet" on a non-blocking rawmidi read. ALSA reports its errnos
/// negated, and the crate passes them through as raw OS errors rather than
/// mapping them, so `ErrorKind::WouldBlock` alone does not catch it.
fn would_block(e: &std::io::Error) -> bool {
    const EAGAIN: i32 = 11;
    e.kind() == std::io::ErrorKind::WouldBlock
        || matches!(e.raw_os_error(), Some(n) if n.abs() == EAGAIN)
}

/// A bidirectional rawmidi connection, plus whatever inbound bytes have
/// arrived but not yet been consumed as a message.
pub struct Link {
    out: Rawmidi,
    inp: Rawmidi,
    pending: VecDeque<u8>,
}

impl Link {
    pub fn open(hw: &str) -> Result<Self> {
        let out = Rawmidi::new(hw, Direction::Playback, true)
            .with_context(|| format!("opening {hw} for output"))?;
        let inp = Rawmidi::new(hw, Direction::Capture, true)
            .with_context(|| format!("opening {hw} for input"))?;
        Ok(Self {
            out,
            inp,
            pending: VecDeque::new(),
        })
    }

    /// Send bytes exactly as given -- already a complete MIDI message.
    pub fn send_raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.out
            .io()
            .write_all(bytes)
            .context("writing to MIDI out")
    }

    pub fn send(&mut self, packet: &Packet) -> Result<()> {
        self.send_raw(&codec::encode_sysex(&packet.to_bytes()))
    }

    /// Wait for one SysEx message and parse it as a packet. Messages that are
    /// not ours are skipped, not failed on -- anything else on the port would
    /// otherwise abort a flash.
    pub fn recv(&mut self, timeout: Duration) -> Result<Packet> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(body) = self.take_sysex() {
                let Ok(bytes) = codec::decode_sysex(&body) else {
                    continue;
                };
                match Packet::parse(&bytes) {
                    Ok(packet) => return Ok(packet),
                    Err(_) => continue,
                }
            }
            if Instant::now() >= deadline {
                bail!("no reply from the device within {timeout:?}");
            }
            self.fill()?;
        }
    }

    /// Drop anything already buffered, so a reply can't be confused with
    /// chatter that arrived before the request went out.
    pub fn drain(&mut self) -> Result<()> {
        self.fill()?;
        self.pending.clear();
        Ok(())
    }

    fn fill(&mut self) -> Result<()> {
        let mut buf = [0u8; 4096];
        match self.inp.io().read(&mut buf) {
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(n) => self.pending.extend(&buf[..n]),
            Err(e) if would_block(&e) => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) => return Err(e).context("reading from MIDI in"),
        }
        Ok(())
    }

    /// Pull the next complete `F0`..`F7` message out of the buffer, dropping
    /// whatever precedes it. Realtime bytes may legally interleave with a
    /// SysEx, so they are stripped rather than treated as the end of one.
    fn take_sysex(&mut self) -> Option<Vec<u8>> {
        let start = self.pending.iter().position(|b| *b == 0xF0)?;
        let end = self
            .pending
            .iter()
            .skip(start + 1)
            .position(|b| *b == 0xF7)?;

        let body: Vec<u8> = self
            .pending
            .iter()
            .skip(start + 1)
            .take(end)
            .copied()
            .filter(|b| *b < 0xF8)
            .collect();

        self.pending.drain(..start + end + 2);
        Some(body)
    }
}
