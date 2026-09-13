//! Winry315 macropad daemon. The pad reports raw key and encoder events and
//! holds no policy; everything about what the pad *does* lives in `modes`.
//! The wire protocol is in DESIGN.md.

mod audio;
mod board;
mod grid;
mod modes;
mod pad;

use anyhow::Result;
use grid::LED_COUNT;
use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How often the renderer advances an ease. Fast enough to look continuous,
/// slow enough that a settled pad costs nothing.
pub const TICK_MS: f32 = 30.0;
pub const TICK: Duration = Duration::from_millis(TICK_MS as u64);

fn main() -> Result<()> {
    let (pad, mut reader) = pad::open()?;
    pad.beat();

    let (tx, rx) = mpsc::channel();
    let reports = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; pad::REPORT_SIZE];
        while let Ok(n) = reader.read(&mut buf) {
            let Some(event) = (n > 0).then(|| pad::parse_event(&buf)).flatten() else {
                if n == 0 {
                    break;
                }
                continue;
            };
            if reports.send(modes::Input::Pad(event)).is_err() {
                break;
            }
        }
    });

    let effects = modes::Effects::new(tx);
    let mut sources = modes::Sources::new();
    let mut state = modes::State::new();
    let mut board = board::Board::new();
    pad.show(&[[0; 3]; LED_COUNT])?;

    loop {
        // Fold everything that arrives before the next frame is due, then draw
        // once. Draining rather than drawing per input keeps the renderer's
        // clock fixed, so how fast a knob is turned cannot change how fast
        // anything eases or decays.
        let deadline = Instant::now() + TICK;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(input) => {
                    let (next, action) = modes::absorb(state, input);
                    if next.active != state.active {
                        eprintln!("winry315: {:?}", next.active);
                    }
                    state = next;
                    if let Some(action) = action {
                        effects.run(action);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    eprintln!("winry315: pad disconnected");
                    return Ok(());
                }
            }
        }

        sources = sources.tick(&state);
        let (next, update) = board.reconcile(modes::frame(&state, &sources));
        board = next;
        if let Some(cells) = update {
            pad.show(&cells)?;
        }
    }
}
