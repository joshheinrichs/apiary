//! The wire: finding the pad, reading its events, painting its LEDs.
//!
//! Everything here is protocol and transport. No mode is visible from this
//! module and no policy lives in it.

use crate::grid::{Cells, LED_COUNT};
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// Keyed on vid:pid rather than the product string, which our firmware changes
// from the vendor's.
const HID_ID: &str = "0003:0000F1F1:00000315";
// Vendor-defined usage page 0xFF60 -- the one interface of three carrying raw HID.
const RAW_USAGE_PAGE: [u8; 3] = [0x06, 0x60, 0xff];

pub const REPORT_SIZE: usize = 32;

const EVT_KEY: u8 = 0x01;
const EVT_ENCODER: u8 = 0x02;
const CMD_PING: u8 = 0x04;
const CMD_LEDS: u8 = 0x02;

// The pad gives up on us after 2s of silence, so idle still has to talk.
const HEARTBEAT: Duration = Duration::from_millis(500);

/// How bright the pad runs, after gamma. These LEDs are painfully bright at
/// full scale; the picture is built at full range and only scaled on the way
/// to the wire.
const BRIGHTNESS: f32 = 0.25;
/// sRGB's encoding gamma. Colours arrive encoded for a screen and these LEDs
/// are linear, so it has to be undone on the way out.
const GAMMA: f32 = 2.2;
/// Colours that fit after the opcode, offset and count, at three bytes each.
const LEDS_PER_REPORT: usize = (REPORT_SIZE - 3) / 3;
const REPORTS_PER_FRAME: usize = LED_COUNT.div_ceil(LEDS_PER_REPORT);

/// Encoder switches, renumbered by the firmware to line up with the encoder
/// indices (see DESIGN.md).
pub const KEY_ENCODER_LEFT: u8 = 15;
pub const KEY_ENCODER_CENTRE: u8 = 16;
pub const KEY_ENCODER_RIGHT: u8 = 17;
pub const ENCODER_LEFT: u8 = 0;
pub const ENCODER_CENTRE: u8 = 1;
pub const ENCODER_RIGHT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadEvent {
    Key { index: u8, pressed: bool },
    Encoder { index: u8, delta: i8 },
}

pub fn parse_event(report: &[u8]) -> Option<PadEvent> {
    match *report.first()? {
        EVT_KEY => Some(PadEvent::Key {
            index: *report.get(1)?,
            pressed: *report.get(2)? == 1,
        }),
        EVT_ENCODER => Some(PadEvent::Encoder {
            index: *report.get(1)?,
            delta: *report.get(2)? as i8,
        }),
        _ => None,
    }
}

/// Convert a colour to what the LED should actually emit, and dim it.
///
/// Colours are sRGB, which is gamma encoded; an LED's brightness is linear in
/// its duty cycle. Sending sRGB values straight through lifts every dark
/// channel about tenfold -- a saturated red's 40 emits 15.7% of full light
/// instead of 1.6% -- which drags every colour toward white and is why they
/// looked pale. Undoing the encoding restores the saturation the picture had.
///
/// Rounds rather than truncates: at this brightness truncation puts every dark
/// colour at zero.
fn dim(channel: u8) -> u8 {
    let linear = (channel as f32 / 255.0).powf(GAMMA);
    (linear * 255.0 * BRIGHTNESS).round() as u8
}

/// The pad, at full colour depth, as the handful of reports it takes. Three
/// bytes per LED means three reports rather than one, which is the whole cost
/// of never having to think about which colours are representable.
fn led_reports(cells: &Cells) -> [[u8; REPORT_SIZE]; REPORTS_PER_FRAME] {
    std::array::from_fn(|chunk| {
        let offset = chunk * LEDS_PER_REPORT;
        let run = &cells[offset..(offset + LEDS_PER_REPORT).min(LED_COUNT)];
        let mut msg = [0u8; REPORT_SIZE];
        msg[0] = CMD_LEDS;
        msg[1] = offset as u8;
        msg[2] = run.len() as u8;
        for (i, rgb) in run.iter().enumerate() {
            msg[3 + i * 3..6 + i * 3].copy_from_slice(&rgb.map(dim));
        }
        msg
    })
}

/// hidraw wants a leading report number and QMK's raw HID reports are
/// unnumbered, so every write is 0x00 followed by the payload.
fn frame(payload: [u8; REPORT_SIZE]) -> [u8; REPORT_SIZE + 1] {
    let mut framed = [0u8; REPORT_SIZE + 1];
    framed[1..].copy_from_slice(&payload);
    framed
}

fn find() -> Result<String> {
    for entry in fs::read_dir("/sys/class/hidraw")?.flatten() {
        let device = entry.path().join("device");
        if !fs::read_to_string(device.join("uevent"))
            .unwrap_or_default()
            .contains(HID_ID)
        {
            continue;
        }
        if fs::read(device.join("report_descriptor"))
            .unwrap_or_default()
            .starts_with(&RAW_USAGE_PAGE)
        {
            return Ok(format!("/dev/{}", entry.file_name().to_string_lossy()));
        }
    }
    Err(anyhow!("winry315 raw HID interface not found"))
}

/// The write end of the pad. Liveness and pixels are separate concerns on
/// separate schedules, so this is shared: the heartbeat thread proves the
/// daemon is alive, the event loop says what to draw.
#[derive(Clone)]
pub struct Pad(Arc<Mutex<fs::File>>);

/// Open the pad, yielding the write end and the read end of the same device.
pub fn open() -> Result<(Pad, fs::File)> {
    let path = find()?;
    eprintln!("winry315: using {path}");
    let pad = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {path}"))?;
    let reader = pad.try_clone()?;
    Ok((Pad(Arc::new(Mutex::new(pad))), reader))
}

impl Pad {
    fn send(&self, payload: [u8; REPORT_SIZE]) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow!("pad writer poisoned"))?
            .write_all(&frame(payload))
            .context("writing to pad")
    }

    /// Paint the whole pad.
    pub fn show(&self, cells: &Cells) -> Result<()> {
        for report in led_reports(cells) {
            self.send(report)?;
        }
        Ok(())
    }

    /// The pad gives up on a silent host after 2s. Pinging on its own thread
    /// means no amount of work in the event loop -- a long knob sweep, a slow
    /// D-Bus call -- can starve it.
    pub fn beat(&self) {
        let pad = self.clone();
        thread::spawn(move || {
            let mut ping = [0u8; REPORT_SIZE];
            ping[0] = CMD_PING;
            loop {
                thread::sleep(HEARTBEAT);
                if pad.send(ping).is_err() {
                    return;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_and_encoder_reports() {
        assert_eq!(
            parse_event(&[EVT_KEY, 7, 1]),
            Some(PadEvent::Key {
                index: 7,
                pressed: true
            })
        );
        // Deltas are signed: 0xff is one detent anticlockwise.
        assert_eq!(
            parse_event(&[EVT_ENCODER, 2, 0xff]),
            Some(PadEvent::Encoder {
                index: 2,
                delta: -1
            })
        );
        assert_eq!(parse_event(&[0x7f, 0, 0]), None);
        assert_eq!(parse_event(&[]), None);
    }

    #[test]
    fn framing_prepends_the_report_number() {
        let framed = frame(led_reports(&[[1, 2, 3]; LED_COUNT])[0]);
        assert_eq!(framed.len(), REPORT_SIZE + 1);
        assert_eq!(&framed[..3], &[0x00, CMD_LEDS, 0]);
    }

    #[test]
    fn every_led_is_sent_exactly_once_and_dimmed_on_the_way() {
        let cells: Cells = std::array::from_fn(|i| [i as u8, 255 - i as u8, 128]);
        let reports = led_reports(&cells);

        let mut seen = vec![];
        for msg in &reports {
            assert_eq!(msg[0], CMD_LEDS);
            let (offset, count) = (msg[1] as usize, msg[2] as usize);
            assert_eq!(offset, seen.len(), "runs must be contiguous");
            for i in 0..count {
                seen.push([msg[3 + i * 3], msg[4 + i * 3], msg[5 + i * 3]]);
            }
        }
        assert_eq!(seen.len(), LED_COUNT, "every LED exactly once");
        // Each colour arrives scaled, and nothing else happens to it.
        for (sent, wanted) in seen.iter().zip(cells.iter()) {
            assert_eq!(*sent, wanted.map(dim));
        }
        // Full scale lands on the brightness we chose, and black stays black.
        assert_eq!(dim(255), (255.0 * BRIGHTNESS).round() as u8);
        assert_eq!(dim(0), 0);
    }
}
