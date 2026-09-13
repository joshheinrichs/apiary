//! Colour mode: the three knobs are red, green and blue, and the pad shows
//! what they add up to.

use crate::grid::{Cells, LED_COUNT};
use crate::pad::PadEvent;

/// What this mode's indicator shows.
pub const TINT: [u8; 3] = [255, 255, 255];

/// 32 steps across a channel: fine enough to land on the colour you meant,
/// coarse enough to cross the whole range in about a turn and a half.
const COLOUR_STEP: i16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    rgb: [u8; 3],
}

impl State {
    pub const fn new() -> Self {
        State { rgb: [0, 0, 0] }
    }

    pub fn apply(self, event: PadEvent) -> Self {
        match event {
            PadEvent::Encoder { index, delta } => State {
                rgb: adjust_channel(self.rgb, index, delta),
            },
            PadEvent::Key { .. } => self,
        }
    }

    pub fn cells(&self) -> Cells {
        [self.rgb; LED_COUNT]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_clamp_rather_than_wrap() {
        assert_eq!(adjust_channel([250, 0, 0], 0, 1), [255, 0, 0]);
        assert_eq!(adjust_channel([4, 0, 0], 0, -1), [0, 0, 0]);
        // The pad has three knobs; a fourth index is not ours to handle.
        assert_eq!(adjust_channel([1, 2, 3], 9, 1), [1, 2, 3]);
    }

    #[test]
    fn each_knob_drives_its_own_channel() {
        let turn = |index, delta| State::new().apply(PadEvent::Encoder { index, delta }).rgb;
        assert_eq!(turn(1, 1), [0, COLOUR_STEP as u8, 0]);
        assert_eq!(turn(2, 2), [0, 0, 2 * COLOUR_STEP as u8]);
    }

    #[test]
    fn keys_do_nothing_here() {
        let state = State { rgb: [1, 2, 3] };
        assert_eq!(state.apply(PadEvent::Key { index: 4, pressed: true }), state);
    }

    #[test]
    fn the_whole_pad_is_the_colour() {
        assert_eq!(State { rgb: [9, 8, 7] }.cells(), [[9, 8, 7]; LED_COUNT]);
    }
}
