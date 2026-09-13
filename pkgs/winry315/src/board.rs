//! What the pad is actually showing, against what the modes want it to show.
//!
//! Desired versus actual: the board holds the picture in flight and the last
//! thing written to the wire, and every tick closes some of the gap. It knows
//! nothing about modes -- a [`Frame`] is the whole of what it is told.

use crate::audio::{self, Levels};
use crate::grid::{self, Cells, LED_COUNT};

/// What the mode layer hands the board each tick.
pub struct Frame {
    /// The picture wanted. Colours ease toward it.
    pub cells: Cells,
    /// Sound to scale the eased picture by, for a mode that reacts to any.
    /// Never eased -- a hit has to land on the frame it happens, or it stops
    /// reading as a hit.
    pub levels: Option<Levels>,
}

#[derive(Clone)]
pub struct Board {
    /// How far the ease has got. The *unmodulated* picture: levels scale what
    /// leaves here, and must never be fed back in, or a quiet passage would
    /// drag the colours toward black and they would have to climb back.
    shown: Cells,
    /// The last frame the pad was given, so a settled board can say nothing.
    sent: Cells,
}

impl Board {
    pub const fn new() -> Self {
        Board {
            shown: [[0; 3]; LED_COUNT],
            sent: [[0; 3]; LED_COUNT],
        }
    }

    /// Fold one frame in, and say what the wire needs -- `None` when the pad is
    /// already showing it.
    pub fn reconcile(self, frame: Frame) -> (Board, Option<Cells>) {
        let shown = grid::ease_cells(&self.shown, &frame.cells);
        let lit = match frame.levels {
            Some(levels) => audio::modulate(&shown, levels),
            None => shown,
        };
        (Board { shown, sent: lit }, (lit != self.sent).then_some(lit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{BANDS, SILENCE};

    fn plain(cells: Cells) -> Frame {
        Frame { cells, levels: None }
    }

    fn loud(cells: Cells) -> Frame {
        Frame { cells, levels: Some(Levels { left: [1.0; BANDS], right: [1.0; BANDS] }) }
    }

    #[test]
    fn a_settled_board_says_nothing() {
        // Frames go out on change only, so an idle pad is quiet on the wire and
        // the heartbeat is the only traffic left.
        let target = [[200u8; 3]; LED_COUNT];
        let mut board = Board::new();
        let mut updates = 0;
        for _ in 0..128 {
            let (next, update) = board.reconcile(plain(target));
            board = next;
            updates += usize::from(update.is_some());
        }
        assert!(updates > 1, "the ease never moved");
        assert!(updates < 128, "a settled board kept talking: {updates} updates");

        // And once settled it is showing exactly what was asked for.
        assert_eq!(board.sent, target);
    }

    #[test]
    fn colours_ease_and_levels_do_not() {
        let target = [[200u8; 3]; LED_COUNT];

        // Easing: one frame gets part of the way, never the whole way.
        let (eased, update) = Board::new().reconcile(plain(target));
        assert_ne!(update, Some(target), "colour arrived in one frame");
        assert!(eased.shown[0][0] > 0, "colour did not move at all");

        // Levels: full scale passes the eased picture through untouched, and
        // silence lands on the very next frame rather than fading.
        let (_, lit) = eased.clone().reconcile(loud(target));
        let (_, dark) = eased.reconcile(Frame { cells: target, levels: Some(SILENCE) });
        assert_eq!(dark, Some([[0u8; 3]; LED_COUNT]), "silence was eased");
        assert!(lit.unwrap()[0][0] > 0, "full level darkened the picture");
    }

    #[test]
    fn a_quiet_passage_does_not_drag_the_ease_backwards() {
        // The bug this guards: feeding the modulated frame back in as the eased
        // picture. The cover would then decay toward black through every quiet
        // moment and have to climb out again on the next beat.
        let target = [[200u8; 3]; LED_COUNT];
        let mut board = Board::new();
        for _ in 0..64 {
            board = board.reconcile(loud(target)).0;
        }
        assert_eq!(board.shown, target, "the ease did not settle");

        // Silence blacks the pad, but the picture underneath is untouched.
        let (after, dark) = board.reconcile(Frame { cells: target, levels: Some(SILENCE) });
        assert_eq!(dark, Some([[0u8; 3]; LED_COUNT]));
        assert_eq!(after.shown, target, "silence ate the picture");
        // So the next beat is instantly back at full, with nothing to climb.
        assert_eq!(after.reconcile(loud(target)).1, Some(target));
    }
}
