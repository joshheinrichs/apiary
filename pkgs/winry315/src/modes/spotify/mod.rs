//! Spotify mode: the knobs drive the player and the pad shows the album
//! flashing along to it.

mod art;
mod mpris;

use crate::audio::{Levels, Meter, SILENCE};
use crate::grid::{Cells, GRID, GRID_W, LED_COUNT, UNDERGLOW_LEFT, UNDERGLOW_RIGHT};
use crate::pad::{
    ENCODER_CENTRE, ENCODER_LEFT, ENCODER_RIGHT, KEY_ENCODER_CENTRE, KEY_ENCODER_LEFT,
    KEY_ENCODER_RIGHT, PadEvent,
};
use art::Album;
use mpris::Player;
use std::thread;
use std::time::Duration;

/// Spotify green: this mode's indicator, and what the pad shows until a cover
/// lands. No part of the pad ever waits on the network.
pub const TINT: [u8; 3] = [30, 215, 96];

/// Seek per detent. Coarse on purpose: the knob is for finding a spot in a
/// track, not for frame-accurate scrubbing.
const SEEK_STEP_US: i64 = 5_000_000;
/// Volume per detent, against MPRIS's 0.0 - 1.0 scale.
const VOLUME_STEP: f64 = 0.05;
/// How often to look for a track change. Slow on purpose -- this is identity,
/// not motion.
const ART_POLL: Duration = Duration::from_secs(2);
/// The node to listen to. Spotify's own output, not the sink everything
/// shares, so a notification does not flash the pad.
const AUDIO_SOURCE: &str = "spotify";

/// What this mode's watcher has to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Album(Option<Album>),
    Playing(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    PlayPause,
    NextTrack,
    PrevTrack,
    Seek(i64),
    VolumeStep(i8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// The album, once its art has been fetched and distilled.
    album: Option<Album>,
    /// Whether the player is running. Levels mean nothing when it is not.
    playing: bool,
}

impl State {
    pub const fn new() -> Self {
        State { album: None, playing: false }
    }

    pub fn absorb(self, input: Input) -> Self {
        match input {
            Input::Album(album) => State { album, ..self },
            Input::Playing(playing) => State { playing, ..self },
        }
    }

    /// Both side knobs scrub, because whichever hand is nearer should work.
    /// Direction comes from the rotation, so the knobs' identity is only
    /// meaningful on their clicks, where it picks previous vs next.
    pub fn apply(self, event: PadEvent) -> (Self, Option<Action>) {
        let action = match event {
            PadEvent::Encoder { index: ENCODER_CENTRE, delta } => Some(Action::VolumeStep(delta)),
            PadEvent::Encoder { index: ENCODER_LEFT | ENCODER_RIGHT, delta } => {
                Some(Action::Seek(delta as i64 * SEEK_STEP_US))
            }
            PadEvent::Key { index: KEY_ENCODER_CENTRE, pressed: true } => Some(Action::PlayPause),
            PadEvent::Key { index: KEY_ENCODER_LEFT, pressed: true } => Some(Action::PrevTrack),
            PadEvent::Key { index: KEY_ENCODER_RIGHT, pressed: true } => Some(Action::NextTrack),
            _ => None,
        };
        (self, action)
    }

    /// The cover across the whole grid, with the sides carrying on from the
    /// outer column of whichever row each one sits beside.
    pub fn cells(&self) -> Cells {
        let Some(album) = self.album else {
            return [TINT; LED_COUNT];
        };
        let mut cells = [[0u8; 3]; LED_COUNT];
        for (column, rows) in GRID.iter().enumerate() {
            for (row, leds) in rows.iter().enumerate() {
                for led in *leds {
                    cells[*led as usize] = album.grid[row * GRID_W + column];
                }
            }
        }
        for (led, row) in UNDERGLOW_LEFT {
            cells[led as usize] = album.grid[row * GRID_W];
        }
        for (led, row) in UNDERGLOW_RIGHT {
            cells[led as usize] = album.grid[row * GRID_W + GRID_W - 1];
        }
        cells
    }
}

/// The music, as levels this mode scales its picture by.
pub struct Sources {
    meter: Meter,
    heard: Levels,
}

impl Sources {
    pub fn new() -> Self {
        Sources { meter: Meter::watch(AUDIO_SOURCE), heard: SILENCE }
    }

    pub fn tick(self, state: &State) -> Self {
        Sources { heard: self.meter.tick(self.heard, state.playing), ..self }
    }

    pub fn levels(&self) -> Levels {
        self.heard
    }
}

/// Spotify not being up is ordinary, not fatal: the pad keeps working as a
/// colour picker and the failure goes to stderr.
pub struct Effects {
    player: Option<Player>,
}

impl Effects {
    pub fn new() -> Self {
        Effects {
            player: Player::connect()
                .map_err(|e| eprintln!("winry315: no session bus, Spotify mode inert: {e:#}"))
                .ok(),
        }
    }

    pub fn run(&self, action: Action) {
        let Some(player) = &self.player else {
            eprintln!("winry315: {action:?} dropped, no session bus");
            return;
        };
        let done = match action {
            Action::PlayPause => player.play_pause(),
            Action::NextTrack => player.next(),
            Action::PrevTrack => player.previous(),
            Action::Seek(offset) => player.seek(offset),
            Action::VolumeStep(detents) => player
                .volume()
                .and_then(|at| player.set_volume((at + detents as f64 * VOLUME_STEP).clamp(0.0, 1.0))),
        };
        if let Err(e) = done {
            eprintln!("winry315: {action:?} failed: {e:#}");
        }
    }
}

/// Watch what the player is doing: which album, and whether it is running at
/// all. Every step here is allowed to fail -- no network, no player, art that
/// decodes to nothing -- and the pad just carries on.
pub fn watch(sink: impl Fn(Input) -> bool + Send + 'static) {
    thread::spawn(move || {
        let Ok(player) = Player::connect() else {
            return;
        };
        let mut current = None;
        let mut was_playing = false;
        loop {
            thread::sleep(ART_POLL);

            let now_playing = player.playing();
            if now_playing != was_playing {
                was_playing = now_playing;
                if !sink(Input::Playing(now_playing)) {
                    return;
                }
            }

            let seen = player.art_url().unwrap_or(None);
            if seen == current {
                continue;
            }
            current = seen;
            let album = current.as_ref().and_then(|url| {
                art::load_art(url)
                    .and_then(|jpeg| art::album_of_art(&jpeg))
                    .map_err(|e| eprintln!("winry315: album art: {e:#}"))
                    .ok()
            });
            eprintln!("winry315: album {}", if album.is_some() { "loaded" } else { "none" });
            if !sink(Input::Album(album)) {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> State {
        State::new().absorb(Input::Playing(true))
    }

    #[test]
    fn every_knob_and_click_is_mapped() {
        let act = |e| playing().apply(e).1;

        // Both side knobs scrub, and direction comes from the rotation.
        for index in [ENCODER_LEFT, ENCODER_RIGHT] {
            assert_eq!(act(PadEvent::Encoder { index, delta: 1 }), Some(Action::Seek(SEEK_STEP_US)));
            assert_eq!(act(PadEvent::Encoder { index, delta: -1 }), Some(Action::Seek(-SEEK_STEP_US)));
        }
        assert_eq!(
            act(PadEvent::Encoder { index: ENCODER_CENTRE, delta: -2 }),
            Some(Action::VolumeStep(-2))
        );

        // Clicks are where the side knobs' identity matters.
        let click = |index| PadEvent::Key { index, pressed: true };
        assert_eq!(act(click(KEY_ENCODER_LEFT)), Some(Action::PrevTrack));
        assert_eq!(act(click(KEY_ENCODER_CENTRE)), Some(Action::PlayPause));
        assert_eq!(act(click(KEY_ENCODER_RIGHT)), Some(Action::NextTrack));

        // Releases must not double-fire.
        assert_eq!(act(PadEvent::Key { index: KEY_ENCODER_CENTRE, pressed: false }), None);
    }

    #[test]
    fn the_album_lands_on_the_grid_and_carries_on_down_the_sides() {
        let grid = std::array::from_fn(|i| [i as u8 * 4, 0, 0]);
        let cells = playing().absorb(Input::Album(Some(Album { grid }))).cells();

        // GRID[col][row]; row 0 is the bottom, and columns alternate direction,
        // so cell (col 1, row 2) is LED 11 -- not a constant stride from LED 6.
        assert_eq!(cells[11], grid[2 * GRID_W + 1]);
        assert_eq!(cells[6], grid[2 * GRID_W]);
        // The sides take the outer cell of the row they sit beside.
        for (led, row) in UNDERGLOW_LEFT {
            assert_eq!(cells[led as usize], grid[row * GRID_W], "LED {led}");
        }
        for (led, row) in UNDERGLOW_RIGHT {
            assert_eq!(cells[led as usize], grid[row * GRID_W + GRID_W - 1], "LED {led}");
        }
    }

    #[test]
    fn no_cover_means_the_mode_shows_its_own_colour() {
        // The network is allowed to be slow or absent; the pad is not.
        assert_eq!(State::new().cells(), [TINT; LED_COUNT]);
    }

    #[test]
    fn whether_the_player_runs_changes_the_levels_not_the_picture() {
        // A capture stream whose target is missing falls back to the default
        // source -- verified: a nonsense target still captured live audio. So
        // `playing` gates the levels (see Meter::tick), and only the levels.
        let grid = std::array::from_fn(|_| [200u8; 3]);
        let stopped = State::new().absorb(Input::Album(Some(Album { grid })));
        let running = stopped.absorb(Input::Playing(true));
        assert_eq!(stopped.cells(), running.cells());
        assert!(!stopped.playing && running.playing);
    }
}
