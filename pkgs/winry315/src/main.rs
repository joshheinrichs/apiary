// Winry315 macropad daemon. The pad reports raw key and encoder events and
// holds no policy; everything about what the pad *does* lives here. The wire
// protocol is in DESIGN.md.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// Keyed on vid:pid rather than the product string, which our firmware changes
// from the vendor's.
const HID_ID: &str = "0003:0000F1F1:00000315";
// Vendor-defined usage page 0xFF60 -- the one interface of three carrying raw HID.
const RAW_USAGE_PAGE: [u8; 3] = [0x06, 0x60, 0xff];

const REPORT_SIZE: usize = 32;

const EVT_KEY: u8 = 0x01;
const EVT_ENCODER: u8 = 0x02;
const CMD_PING: u8 = 0x04;
const CMD_FRAME: u8 = 0x05;

/// Overrides that fit after the opcode and wash colour, at 4 bytes each.
const FRAME_MAX_OVERRIDES: usize = (REPORT_SIZE - 5) / 4;
const _: () = assert!(
    MODE_SLOTS.len() <= FRAME_MAX_OVERRIDES,
    "a frame carries every mode indicator, so the bottom row has to fit in one report"
);

const COLOUR_STEP: i16 = 8;
// The pad gives up on us after 2s of silence, so idle still has to talk.
const HEARTBEAT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PadEvent {
    Key { index: u8, pressed: bool },
    Encoder { index: u8, delta: i8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Colour,
}

/// The bottom row of keys selects the mode. One table drives both which key
/// picks what and which LED shows it, so the two can't drift apart. Only
/// colour exists so far; the rest of the row is waiting on the modes in
/// INTENT.md.
const MODE_SLOTS: [(u8, u8, Mode); 1] = [(10, 8, Mode::Colour)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct State {
    mode: Mode,
    rgb: [u8; 3],
}

impl Mode {
    fn tint(self) -> [u8; 3] {
        match self {
            Mode::Colour => [255, 255, 255],
        }
    }
}

fn mode_for_key(index: u8) -> Option<Mode> {
    MODE_SLOTS
        .iter()
        .find(|(key, _, _)| *key == index)
        .map(|(_, _, mode)| *mode)
}

fn parse_event(report: &[u8]) -> Option<PadEvent> {
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

fn adjust_channel(rgb: [u8; 3], index: u8, delta: i8) -> [u8; 3] {
    let Some(current) = rgb.get(index as usize) else {
        return rgb;
    };
    let next = (*current as i16 + delta as i16 * COLOUR_STEP).clamp(0, 255) as u8;
    let mut adjusted = rgb;
    adjusted[index as usize] = next;
    adjusted
}

fn apply(state: State, event: PadEvent) -> State {
    match event {
        // Mode keys select on press; their release is not interesting.
        PadEvent::Key {
            index,
            pressed: true,
        } => match mode_for_key(index) {
            Some(mode) => State { mode, ..state },
            None => state,
        },
        PadEvent::Key { .. } => state,
        PadEvent::Encoder { index, delta } => match state.mode {
            Mode::Colour => State {
                rgb: adjust_channel(state.rgb, index, delta),
                ..state
            },
        },
    }
}

fn scale(rgb: [u8; 3], numerator: u16, denominator: u16) -> [u8; 3] {
    rgb.map(|c| (c as u16 * numerator / denominator) as u8)
}

/// The complete desired LED state as one report: an ambient wash, plus the
/// bottom row showing each mode in its own colour with the active one at full
/// brightness. One report rather than a wash followed by patches, because the
/// pad renders between reports and would flicker the patched LEDs.
fn paint(state: State) -> [u8; REPORT_SIZE] {
    let wash = match state.mode {
        Mode::Colour => state.rgb,
    };
    let mut msg = [0u8; REPORT_SIZE];
    msg[0] = CMD_FRAME;
    msg[1..4].copy_from_slice(&wash);
    msg[4] = MODE_SLOTS.len() as u8;
    for (slot, (_, led, mode)) in MODE_SLOTS.iter().enumerate() {
        let rgb = if *mode == state.mode {
            mode.tint()
        } else {
            scale(mode.tint(), 1, 5)
        };
        let at = 5 + slot * 4;
        msg[at] = *led;
        msg[at + 1..at + 4].copy_from_slice(&rgb);
    }
    msg
}

fn find_pad() -> Result<String> {
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

/// hidraw wants a leading report number and QMK's raw HID reports are
/// unnumbered, so every write is 0x00 followed by the payload.
fn frame(payload: [u8; REPORT_SIZE]) -> [u8; REPORT_SIZE + 1] {
    let mut framed = [0u8; REPORT_SIZE + 1];
    framed[1..].copy_from_slice(&payload);
    framed
}

fn send(pad: &mut fs::File, payload: [u8; REPORT_SIZE]) -> Result<()> {
    pad.write_all(&frame(payload)).context("writing to pad")
}

fn main() -> Result<()> {
    let path = find_pad()?;
    eprintln!("winry315: using {path}");

    let mut pad = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {path}"))?;
    let mut reader = pad.try_clone()?;

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; REPORT_SIZE];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf).is_err() {
                break;
            }
        }
    });

    let mut state = State {
        mode: Mode::Colour,
        rgb: [0, 0, 0],
    };
    send(&mut pad, paint(state))?;

    loop {
        let report = match rx.recv_timeout(HEARTBEAT) {
            Ok(report) => report,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut ping = [0u8; REPORT_SIZE];
                ping[0] = CMD_PING;
                send(&mut pad, ping)?;
                continue;
            }
            // The reader thread only stops when the pad stops answering.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("winry315: pad disconnected");
                return Ok(());
            }
        };
        let Some(event) = parse_event(&report) else {
            continue;
        };

        let next = apply(state, event);
        if next != state {
            state = next;
            eprintln!("winry315: {state:?}");
            send(&mut pad, paint(state))?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colour(rgb: [u8; 3]) -> State {
        State {
            mode: Mode::Colour,
            rgb,
        }
    }

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
        let framed = frame(paint(colour([1, 2, 3])));
        assert_eq!(framed.len(), REPORT_SIZE + 1);
        assert_eq!(&framed[..5], &[0x00, CMD_FRAME, 1, 2, 3]);
    }

    #[test]
    fn channels_clamp_rather_than_wrap() {
        assert_eq!(adjust_channel([250, 0, 0], 0, 1), [255, 0, 0]);
        assert_eq!(adjust_channel([4, 0, 0], 0, -1), [0, 0, 0]);
        // The pad has three knobs; a fourth index is not ours to handle.
        assert_eq!(adjust_channel([1, 2, 3], 9, 1), [1, 2, 3]);
    }

    #[test]
    fn each_knob_drives_its_own_channel() {
        assert_eq!(
            apply(colour([0, 0, 0]), PadEvent::Encoder { index: 1, delta: 1 }).rgb,
            [0, COLOUR_STEP as u8, 0]
        );
        assert_eq!(
            apply(colour([0, 0, 0]), PadEvent::Encoder { index: 2, delta: 2 }).rgb,
            [0, 0, 2 * COLOUR_STEP as u8]
        );
    }

    #[test]
    fn bottom_row_selects_modes_and_other_keys_do_not() {
        for (key, _, mode) in MODE_SLOTS {
            let next = apply(
                colour([0, 0, 0]),
                PadEvent::Key {
                    index: key,
                    pressed: true,
                },
            );
            assert_eq!(next.mode, mode, "key {key} should select {mode:?}");
        }
        // Key 14 is the reserved end of the bottom row, not a mode.
        assert_eq!(mode_for_key(14), None);
        // A knob click changes nothing while only colour mode exists.
        assert_eq!(
            apply(colour([1, 2, 3]), PadEvent::Key { index: 15, pressed: true }),
            colour([1, 2, 3])
        );
    }

    #[test]
    fn paint_is_one_report_carrying_wash_and_every_indicator() {
        let frame = paint(colour([9, 8, 7]));
        assert_eq!(&frame[..4], &[CMD_FRAME, 9, 8, 7]);
        assert_eq!(frame[4] as usize, MODE_SLOTS.len());
        // Slot 0 is colour mode's key, which is the active one here.
        assert_eq!(frame[5], 8);
        assert_eq!(&frame[6..9], &Mode::Colour.tint());
    }
}
