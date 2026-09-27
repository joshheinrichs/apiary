//! The pad's physical shape: which LED is where, and how colours move.
//!
//! Nothing here knows about any mode. Modes draw into a `Cells` and this
//! module is the only thing that knows what a cell means on the board.

/// Every LED the pad has. The 15 key LEDs are the grid; the rest ring it.
pub const LED_COUNT: usize = 27;

/// The whole pad, one colour per LED, in LED index order. What every mode
/// paints into and what the transport sends.
pub type Cells = [[u8; 3]; LED_COUNT];

/// The pad as one grid, `GRID[column][row]`, bottom row first. The top row is
/// the six knob LEDs, which sit directly above the keys -- there are six of
/// them over five columns, and by position both LED 3 and LED 2 fall nearest
/// the middle, so that cell holds two. Everything else holds one.
pub const GRID: [[&[u8]; 4]; 5] = [
    [&[8], &[7], &[6], &[5]],
    [&[9], &[10], &[11], &[4]],
    [&[14], &[13], &[12], &[3, 2]],
    [&[15], &[16], &[17], &[1]],
    [&[20], &[19], &[18], &[0]],
];
/// The grid measures itself; nothing else gets to disagree about its shape.
pub const GRID_W: usize = GRID.len();
pub const GRID_H: usize = GRID[0].len();

/// The three rows of keys, below the knob row.
pub const KEY_ROWS: usize = 3;
/// Every key the pad has, in reading order.
pub const KEY_COUNT: usize = KEY_ROWS * GRID_W;

/// The LED under a key. Keys are numbered in reading order from the top left
/// and the grid counts rows upward from the bottom, so the two disagree by a
/// flip; deriving it is what stops a hand-written table drifting.
pub fn led_for_key(key: u8) -> Option<u8> {
    if key as usize >= KEY_COUNT {
        return None;
    }
    let row = KEY_ROWS - 1 - key as usize / GRID_W;
    Some(GRID[key as usize % GRID_W][row][0])
}

/// Underglow: one column down each side, paired with the grid row it sits level
/// with. They are not evenly spaced, so the row is stated rather than derived --
/// the lowest is level with the bottom keys, the middle with the top keys, and
/// the highest with the knobs. The left column is the left channel and the
/// right column the right, so the sides are a level meter per ear.
pub const UNDERGLOW_LEFT: [(u8, usize); 3] = [(22, 0), (23, 2), (24, 3)];
pub const UNDERGLOW_RIGHT: [(u8, usize); 3] = [(21, 0), (26, 2), (25, 3)];

/// Ease one channel toward its target. Colours move; the caller decides what
/// does not.
fn ease(current: u8, target: u8) -> u8 {
    let delta = target as i16 - current as i16;
    // A quarter of the gap per tick, but never zero: integer division stalls
    // at a gap of 2 or 3 and would leave the colour permanently just short.
    let step = match delta {
        0 => return target,
        d if d > 0 => (d / 4).max(1),
        d => (d / 4).min(-1),
    };
    (current as i16 + step) as u8
}

/// Move the shown picture a quarter of the way toward what a mode wants.
/// Colours ease; levels do not, so anything driven by audio is applied after
/// this rather than folded into it.
pub fn ease_cells(current: &Cells, target: &Cells) -> Cells {
    std::array::from_fn(|led| std::array::from_fn(|c| ease(current[led][c], target[led][c])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_table_matches_the_board() {
        // From QMK's winry315.c: LED positions in mm from the PCB centre, y up.
        // Anything that disagrees with this puts the picture on its head.
        let position = |led: u8| -> (i32, i32) {
            match led {
                0 => (35, 36),
                1 => (21, 36),
                2 => (8, 34),
                3 => (-8, 34),
                4 => (-21, 36),
                5 => (-35, 36),
                6 => (-38, 5),
                7 => (-38, -14),
                8 => (-38, -33),
                9 => (-19, -33),
                10 => (-19, -14),
                11 => (-19, 5),
                12 => (0, 5),
                13 => (0, -14),
                14 => (0, -33),
                15 => (19, -33),
                16 => (19, -14),
                17 => (19, 5),
                18 => (38, 5),
                19 => (38, -14),
                20 => (38, -33),
                other => panic!("LED {other} is not on the grid"),
            }
        };
        let mut previous_x = i32::MIN;
        for (column, rows) in GRID.iter().enumerate() {
            // Rows climb: row 0 is the lowest, and the knob row is above them.
            let ys: Vec<i32> = rows.iter().map(|leds| position(leds[0]).1).collect();
            assert!(
                ys.windows(2).all(|p| p[0] < p[1]),
                "column {column} runs down: {ys:?}"
            );
            // Columns run left to right, and a cell's LEDs share its column.
            let x = position(rows[0][0]).0;
            assert!(x > previous_x, "column {column} is out of order");
            previous_x = x;
            for leds in rows.iter() {
                let spread: Vec<i32> = leds.iter().map(|l| position(*l).0).collect();
                let width = spread.iter().max().unwrap() - spread.iter().min().unwrap();
                assert!(width <= 16, "column {column} cell straddles: {spread:?}");
            }
        }
    }

    #[test]
    fn every_led_has_a_job_and_no_led_has_two() {
        // Each of the 27 belongs to exactly one group: keys, knobs or sides.
        let mut seen = std::collections::BTreeSet::new();
        let groups = GRID
            .iter()
            .flatten()
            .copied()
            .flatten()
            .chain(UNDERGLOW_LEFT.iter().map(|(led, _)| led))
            .chain(UNDERGLOW_RIGHT.iter().map(|(led, _)| led));
        for led in groups {
            assert!(seen.insert(*led), "LED {led} is claimed twice");
            assert!((*led as usize) < LED_COUNT, "LED {led} does not exist");
        }
        assert_eq!(seen.len(), LED_COUNT, "some LEDs are unclaimed: {seen:?}");
    }

    #[test]
    fn every_key_has_its_own_led_and_reading_order_is_the_right_way_up() {
        let leds: Vec<u8> = (0..KEY_COUNT as u8)
            .map(|k| led_for_key(k).unwrap())
            .collect();
        assert_eq!(leds.len(), KEY_COUNT);
        assert_eq!(
            leds.iter().collect::<std::collections::BTreeSet<_>>().len(),
            KEY_COUNT,
            "two keys share an LED: {leds:?}"
        );
        // Key 0 is top left and key 14 bottom right, so the first key's LED
        // sits above the last row's and the flip has not been lost.
        assert_eq!(led_for_key(0), Some(GRID[0][KEY_ROWS - 1][0]));
        assert_eq!(led_for_key(14), Some(GRID[GRID_W - 1][0][0]));
        // The knob switches are keys 15..17 and are not on the grid at all.
        assert_eq!(led_for_key(KEY_COUNT as u8), None);
    }

    #[test]
    fn easing_closes_the_gap_and_then_settles_exactly() {
        let from = [[0u8; 3]; LED_COUNT];
        let to = [[200u8; 3]; LED_COUNT];
        let mut at = from;
        for _ in 0..64 {
            at = ease_cells(&at, &to);
        }
        assert_eq!(at, to, "an ease must land on its target, not near it");
        // And it must actually move on the first step.
        assert_ne!(ease_cells(&from, &to), from);
    }
}
