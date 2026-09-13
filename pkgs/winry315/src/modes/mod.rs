//! The modes, and the one table that knows they exist.
//!
//! A mode owns its own state, its own background threads, its own effects and
//! its own picture. Nothing here is shared between two of them, and no mode
//! can see another. Adding one is a row in [`SLOTS`] and a module; the
//! compiler names every other place that has to grow, because every dispatch
//! below is an exhaustive match on [`Mode`].

pub mod colour;
pub mod spotify;

use crate::audio::Levels;
use crate::board::Frame;
use crate::grid::{Cells, led_for_key};
use crate::pad::PadEvent;
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Colour,
    Spotify,
}

/// Where a mode lives on the pad: the key that selects it and the colour it
/// shows there. The indicator LED is derived from the key rather than stated,
/// so the two cannot drift apart.
pub struct Slot {
    pub key: u8,
    pub tint: [u8; 3],
    pub mode: Mode,
}

/// The bottom row of keys picks the mode, one key per mode, no cycling. The
/// rest of the row is waiting on the modes in INTENT.md.
pub const SLOTS: [Slot; 2] = [
    Slot { key: 10, tint: colour::TINT, mode: Mode::Colour },
    Slot { key: 11, tint: spotify::TINT, mode: Mode::Spotify },
];

/// Everything a mode's background threads have to say. The pad speaks through
/// the same channel, so the event loop has exactly one thing to wait on.
pub enum Input {
    Pad(PadEvent),
    Spotify(spotify::Input),
}

/// What an event asks the host to do beyond repainting the pad. Modes stay
/// pure and hand their effects back for [`Effects`] to run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Spotify(spotify::Action),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub active: Mode,
    colour: colour::State,
    spotify: spotify::State,
}

impl State {
    pub const fn new() -> Self {
        State {
            active: Mode::Colour,
            colour: colour::State::new(),
            spotify: spotify::State::new(),
        }
    }
}

/// Fold one input into the state, and say what the host should do about it.
pub fn absorb(state: State, input: Input) -> (State, Option<Action>) {
    match input {
        Input::Pad(event) => pressed(state, event),
        Input::Spotify(input) => (
            State { spotify: state.spotify.absorb(input), ..state },
            None,
        ),
    }
}

/// Mode keys select on press, whatever mode is active; their release is not
/// interesting. Everything else belongs to the mode you are in.
fn pressed(state: State, event: PadEvent) -> (State, Option<Action>) {
    if let PadEvent::Key { index, pressed: true } = event
        && let Some(slot) = SLOTS.iter().find(|slot| slot.key == index)
    {
        return (State { active: slot.mode, ..state }, None);
    }
    match state.active {
        Mode::Colour => (State { colour: state.colour.apply(event), ..state }, None),
        Mode::Spotify => {
            let (spotify, action) = state.spotify.apply(event);
            (State { spotify, ..state }, action.map(Action::Spotify))
        }
    }
}

/// Everything the active mode wants of the board this tick. The one place a
/// mode's state and its live sources become a picture.
pub fn frame(state: &State, sources: &Sources) -> Frame {
    Frame {
        cells: cells(state),
        levels: levels(state, sources),
    }
}

/// The picture the active mode wants, with the indicators composited last --
/// knowing which mode you are in beats seeing every pixel of whatever is
/// underneath. Colours ease toward this.
fn cells(state: &State) -> Cells {
    let mut cells = match state.active {
        Mode::Colour => state.colour.cells(),
        Mode::Spotify => state.spotify.cells(),
    };
    for slot in &SLOTS {
        let Some(led) = led_for_key(slot.key) else { continue };
        cells[led as usize] = match slot.mode == state.active {
            true => slot.tint,
            false => dimmed(slot.tint),
        };
    }
    cells
}

/// What the active mode is hearing, for the board to scale the eased picture
/// by. `None` from a mode that does not react to sound.
fn levels(state: &State, sources: &Sources) -> Option<Levels> {
    match state.active {
        Mode::Colour => None,
        Mode::Spotify => Some(sources.spotify.levels()),
    }
}

/// An indicator for a mode you are not in.
fn dimmed(tint: [u8; 3]) -> [u8; 3] {
    tint.map(|c| c / 5)
}

/// The live handles a mode draws from -- latest-value slots its own threads
/// publish into. Beside `State` rather than in it: a frame must read the
/// newest value, never fold a queue of stale ones.
pub struct Sources {
    spotify: spotify::Sources,
}

impl Sources {
    pub fn new() -> Self {
        Sources {
            spotify: spotify::Sources::new(),
        }
    }

    /// Advance everything that decays on the renderer's clock -- every mode's,
    /// not just the active one, so switching back does not reveal a frozen
    /// frame from minutes ago.
    pub fn tick(self, state: &State) -> Self {
        Sources {
            spotify: self.spotify.tick(&state.spotify),
        }
    }
}

/// Where a mode's effects actually happen, and what starts every mode's
/// background threads. Each mode is handed only the means to announce its own
/// kind of input, so no mode can reach another.
pub struct Effects {
    spotify: spotify::Effects,
}

impl Effects {
    pub fn new(tx: mpsc::Sender<Input>) -> Self {
        spotify::watch(move |input| tx.send(Input::Spotify(input)).is_ok());

        Effects { spotify: spotify::Effects::new() }
    }

    pub fn run(&self, action: Action) {
        match action {
            Action::Spotify(action) => self.spotify.run(action),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::LED_COUNT;

    #[test]
    fn every_slot_is_its_own_key_and_mode() {
        // Two modes sharing a key is the drift this table exists to prevent;
        // sharing an LED is now impossible, because the LED is the key's.
        for (a, first) in SLOTS.iter().enumerate() {
            for second in &SLOTS[a + 1..] {
                assert_ne!(first.key, second.key, "two modes on key {}", first.key);
                assert_ne!(first.mode, second.mode, "{:?} listed twice", first.mode);
            }
            let led = led_for_key(first.key).expect("a mode key is on the grid");
            assert!((led as usize) < LED_COUNT, "LED {led} does not exist");
        }
    }

    #[test]
    fn the_bottom_row_selects_modes_and_other_keys_do_not() {
        for slot in &SLOTS {
            let state = absorb(State::new(), Input::Pad(PadEvent::Key { index: slot.key, pressed: true })).0;
            assert_eq!(state.active, slot.mode, "key {} should select {:?}", slot.key, slot.mode);
        }
        // Key 14 is the reserved end of the bottom row, not a mode.
        let state = absorb(State::new(), Input::Pad(PadEvent::Key { index: 14, pressed: true })).0;
        assert_eq!(state.active, Mode::Colour);
    }

    #[test]
    fn selecting_a_mode_leaves_every_other_mode_untouched() {
        // Independence, asserted: turning a knob in colour mode and then
        // switching away must not disturb what colour mode was holding.
        let picked = absorb(State::new(), Input::Pad(PadEvent::Encoder { index: 0, delta: 3 })).0;
        let switched = absorb(picked, Input::Pad(PadEvent::Key { index: 11, pressed: true })).0;
        assert_eq!(switched.active, Mode::Spotify);
        assert_eq!(switched.colour, picked.colour, "colour mode lost its state");

        let back = absorb(switched, Input::Pad(PadEvent::Key { index: 10, pressed: true })).0;
        assert_eq!(back, picked, "coming back must restore exactly what was left");
    }

    #[test]
    fn a_modes_input_never_reaches_another_mode() {
        // Spotify's watcher speaking while colour mode is active changes
        // Spotify and nothing else.
        let state = State::new();
        let after = absorb(state, Input::Spotify(spotify::Input::Playing(true))).0;
        assert_eq!(after.active, Mode::Colour);
        assert_eq!(after.colour, state.colour);
        assert_ne!(after.spotify, state.spotify);
    }

    #[test]
    fn the_pad_is_washed_with_the_colour_and_still_shows_its_indicators() {
        let state = absorb(State::new(), Input::Pad(PadEvent::Encoder { index: 0, delta: 1 })).0;
        let cells = cells(&state);
        // LED 21 is outside the key grid, so it carries the plain wash.
        assert_eq!(cells[21], state.colour.cells()[21]);
        // Indicators sit on top: colour mode active, spotify dimmed.
        assert_eq!(cells[led_for_key(10).unwrap() as usize], colour::TINT);
        assert_eq!(cells[led_for_key(11).unwrap() as usize], dimmed(spotify::TINT));
    }

    #[test]
    fn an_inactive_mode_never_paints() {
        // Colour mode is active, so nothing Spotify holds may reach the pad
        // beyond its own indicator.
        let state = State { spotify: spotify::State::new().absorb(spotify::Input::Playing(true)), ..State::new() };
        assert_eq!(cells(&state), cells(&State::new()));
    }
}
