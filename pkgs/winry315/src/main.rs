// Winry315 macropad daemon. The pad reports raw key and encoder events and
// holds no policy; everything about what the pad *does* lives here. The wire
// protocol is in DESIGN.md.

use anyhow::{Context, Result, anyhow};
use pipewire as pw;
use zbus::blocking::Connection;
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

// Keyed on vid:pid rather than the product string, which our firmware changes
// from the vendor's.
const HID_ID: &str = "0003:0000F1F1:00000315";
// Vendor-defined usage page 0xFF60 -- the one interface of three carrying raw HID.
const RAW_USAGE_PAGE: [u8; 3] = [0x06, 0x60, 0xff];

const REPORT_SIZE: usize = 32;

const EVT_KEY: u8 = 0x01;
const EVT_ENCODER: u8 = 0x02;
const CMD_PING: u8 = 0x04;
const CMD_LEDS: u8 = 0x02;


/// 32 steps across a channel: fine enough to land on the colour you meant,
/// coarse enough to cross the whole range in about a turn and a half.
const COLOUR_STEP: i16 = 8;
// The pad gives up on us after 2s of silence, so idle still has to talk.
const HEARTBEAT: Duration = Duration::from_millis(500);
/// How often the renderer advances an ease. Fast enough to look continuous,
/// slow enough that a settled pad costs nothing.
const TICK_MS: f32 = 30.0;
const TICK: Duration = Duration::from_millis(TICK_MS as u64);
/// Levels older than this are not news any more: the music has stopped.
const AUDIO_TIMEOUT: Duration = Duration::from_millis(200);

/// Seek per detent. Coarse on purpose: the knob is for finding a spot in a
/// track, not for frame-accurate scrubbing.
const SEEK_STEP_US: i64 = 5_000_000;
/// Volume per detent, against MPRIS's 0.0 - 1.0 scale.
const VOLUME_STEP: f64 = 0.05;

/// Encoder switches, renumbered by the firmware to line up with the encoder
/// indices (see DESIGN.md).
const KEY_ENCODER_LEFT: u8 = 15;
const KEY_ENCODER_CENTRE: u8 = 16;
const KEY_ENCODER_RIGHT: u8 = 17;
const ENCODER_LEFT: u8 = 0;
const ENCODER_CENTRE: u8 = 1;
const ENCODER_RIGHT: u8 = 2;

/// Everything that can change what the pad should show, on one channel: the pad
/// speaks for itself, the art watcher speaks for the album.
enum Input {
    Report([u8; REPORT_SIZE]),
    Album(Option<Album>),
    Playing(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PadEvent {
    Key { index: u8, pressed: bool },
    Encoder { index: u8, delta: i8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Colour,
    Spotify,
}

/// The bottom row of keys selects the mode. One table drives both which key
/// picks what and which LED shows it, so the two can't drift apart. Only
/// colour exists so far; the rest of the row is waiting on the modes in
/// INTENT.md.
const MODE_SLOTS: [(u8, u8, Mode); 2] = [(10, 8, Mode::Colour), (11, 9, Mode::Spotify)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct State {
    mode: Mode,
    rgb: [u8; 3],
    /// The album, once its art has been fetched and distilled.
    album: Option<Album>,
    /// Whether the player is running. Levels mean nothing when it is not.
    playing: bool,
}

/// What one album looks like: a colour for the edges and a cell per key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Album {
    grid: [[u8; 3]; GRID_W * GRID_H],
}

impl Mode {
    fn tint(self) -> [u8; 3] {
        match self {
            Mode::Colour => [255, 255, 255],
            // Spotify green.
            Mode::Spotify => [30, 215, 96],
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
/// What an event asks the host to do beyond repainting the pad. Spotify mode is
/// the first mode whose effects leave the pad, so `apply` yields an action for
/// `main` to run rather than performing it here.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    PlayPause,
    NextTrack,
    PrevTrack,
    Seek(i64),
    VolumeStep(i8),
}

/// Spotify mode: both side knobs scrub, because whichever hand is nearer should
/// work. Direction comes from the rotation, so the knobs' identity is only
/// meaningful on their clicks, where it picks previous vs next.
fn spotify_action(event: PadEvent) -> Option<Action> {
    match event {
        PadEvent::Encoder {
            index: ENCODER_CENTRE,
            delta,
        } => Some(Action::VolumeStep(delta)),
        PadEvent::Encoder {
            index: ENCODER_LEFT | ENCODER_RIGHT,
            delta,
        } => Some(Action::Seek(delta as i64 * SEEK_STEP_US)),
        PadEvent::Key {
            index: KEY_ENCODER_CENTRE,
            pressed: true,
        } => Some(Action::PlayPause),
        PadEvent::Key {
            index: KEY_ENCODER_LEFT,
            pressed: true,
        } => Some(Action::PrevTrack),
        PadEvent::Key {
            index: KEY_ENCODER_RIGHT,
            pressed: true,
        } => Some(Action::NextTrack),
        _ => None,
    }
}

fn apply(state: State, event: PadEvent) -> (State, Option<Action>) {
    match event {
        // Mode keys select on press; their release is not interesting.
        PadEvent::Key {
            index,
            pressed: true,
        } if mode_for_key(index).is_some() => {
            let mode = mode_for_key(index).expect("guarded above");
            (State { mode, ..state }, None)
        }
        _ => match state.mode {
            Mode::Colour => match event {
                PadEvent::Encoder { index, delta } => (
                    State {
                        rgb: adjust_channel(state.rgb, index, delta),
                        ..state
                    },
                    None,
                ),
                PadEvent::Key { .. } => (state, None),
            },
            Mode::Spotify => (state, spotify_action(event)),
        },
    }
}


/// An indicator for a mode you are not in.
fn dimmed(tint: [u8; 3]) -> [u8; 3] {
    tint.map(|c| c / 5)
}

/// Every LED the pad has. The 15 key LEDs are the grid; the rest ring it.
const LED_COUNT: usize = 27;
/// How bright the pad runs, after gamma. These LEDs are painfully bright at
/// look right at about a tenth of it; the picture is built at full range and
/// only scaled on the way to the wire.
const BRIGHTNESS: f32 = 0.25;
/// sRGB's encoding gamma. Colours arrive encoded for a screen and these LEDs
/// are linear, so it has to be undone on the way out.
const GAMMA: f32 = 2.2;
/// Colours that fit after the opcode, offset and count, at three bytes each.
const LEDS_PER_REPORT: usize = (REPORT_SIZE - 3) / 3;
const REPORTS_PER_FRAME: usize = LED_COUNT.div_ceil(LEDS_PER_REPORT);

/// The pad, at full colour depth, as the handful of reports it takes. Three
/// bytes per LED means three reports rather than one, which is the whole cost
/// of never having to think about which colours are representable.
fn led_reports(cells: &[[u8; 3]; LED_COUNT]) -> [[u8; REPORT_SIZE]; REPORTS_PER_FRAME] {
    std::array::from_fn(|chunk| {
        let offset = chunk * LEDS_PER_REPORT;
        let run = &cells[offset..(offset + LEDS_PER_REPORT).min(LED_COUNT)];
        let mut msg = [0u8; REPORT_SIZE];
        msg[0] = CMD_LEDS;
        msg[1] = offset as u8;
        msg[2] = run.len() as u8;
        for (i, rgb) in run.iter().enumerate() {
            let dimmed = rgb.map(dim);
            msg[3 + i * 3..6 + i * 3].copy_from_slice(&dimmed);
        }
        msg
    })
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

/// What the pad should show. The host composites everything -- wash, album,
/// mode indicators -- because it owns the picture and the pad owns nothing.
fn desired_cells(state: State) -> [[u8; 3]; LED_COUNT] {
    let mut cells;
    match (state.mode, state.album) {
        // The cover across the whole grid, and the sides carrying on from the
        // outer column of whichever row each one sits beside.
        (Mode::Spotify, Some(album)) => {
            cells = [[0u8; 3]; LED_COUNT];
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
        }
        (mode, _) => {
            let wash = match mode {
                Mode::Colour => state.rgb,
                Mode::Spotify => Mode::Spotify.tint(),
            };
            cells = [wash; LED_COUNT];
        }
    }
    // Indicators go on last: knowing which mode you are in beats seeing every
    // pixel of the cover.
    for (_, led, mode) in MODE_SLOTS {
        cells[led as usize] = if mode == state.mode {
            mode.tint()
        } else {
            dimmed(mode.tint())
        };
    }
    cells
}

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

fn ease_cells(
    current: &[[u8; 3]; LED_COUNT],
    target: &[[u8; 3]; LED_COUNT],
) -> [[u8; 3]; LED_COUNT] {
    std::array::from_fn(|led| std::array::from_fn(|c| ease(current[led][c], target[led][c])))
}


/// Scale the picture by what the music is doing. Frequency runs up the pad and
/// stereo runs across it:
///
///     level = (left + right) / 2                  how loud this band is
///     tilt  = (right - left) * 60dB / FULL_TILT   which way it leans, -1..1
///     gain(x, y) = level[y] * (1 + tilt[y] * x)   for x across -1..1
///
/// The tilt is amplified on purpose. Both channels of real music carry nearly
/// the same energy, so the raw difference between them moves a column by a few
/// percent and the gradient is invisible; scaling by how many decibels count as
/// "hard panned" turns that into something you can see.
///
/// Applied after easing and never eased itself -- a hit has to land on the
/// frame it happens, or it stops reading as a hit.
fn modulate(cells: &[[u8; 3]; LED_COUNT], levels: Levels) -> [[u8; 3]; LED_COUNT] {
    // Which way the row leans, as a share of its own energy: -1 all left, +1
    // all right. A ratio rather than a difference, so a quiet band leaning hard
    // tilts as far as a loud one.
    let tilt = |row: usize| {
        let (left, right) = (levels.left[row], levels.right[row]);
        match left + right > f32::EPSILON {
            true => (right - left) / (right + left),
            false => 0.0,
        }
    };
    let level = |row: usize| (levels.left[row] + levels.right[row]) / 2.0;
    let mut gain = [0.0f32; LED_COUNT];

    for (column, rows) in GRID.iter().enumerate() {
        let across = 2.0 * column as f32 / (GRID_W - 1) as f32 - 1.0;
        for (row, leds) in rows.iter().enumerate() {
            for led in *leds {
                gain[*led as usize] = level(row) * (1.0 + tilt(row) * across);
            }
        }
    }

    // The sides are a meter per channel rather than part of the picture: no
    // tilt, just how loud that ear is in the band it sits beside.
    for (led, row) in UNDERGLOW_LEFT {
        gain[led as usize] = levels.left[row];
    }
    for (led, row) in UNDERGLOW_RIGHT {
        gain[led as usize] = levels.right[row];
    }

    std::array::from_fn(|led| cells[led].map(|c| (c as f32 * gain[led].clamp(0.0, 1.0)) as u8))
}

// ---------------------------------------------------------------------------
// Audio. One band per row of the grid.
// ---------------------------------------------------------------------------

/// One band per *row*: frequency runs up the pad, low at the bottom. Stereo
/// position runs across it. Each axis carries one thing.
const BANDS: usize = GRID_H;
/// The range worth showing. Below this is rumble, above it is mostly air, and
/// pitch is heard logarithmically -- so the bands are spaced by ratio, not by
/// width: each covers the same number of octaves.
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 12_000.0;
/// ~43ms of audio at 48kHz: enough to resolve a kick, short enough not to lag.
const FFT_SIZE: usize = 2048;
/// Quieter than this is silence, and silence is dark.
const FLOOR_DB: f32 = -60.0;
/// How long a level takes to fall by half once the sound stops. A time, not a
/// per-frame fraction: the frame rate follows the sample rate and the FFT size,
/// so a bare fraction would mean something different on every device.
const RELEASE_HALF_LIFE_MS: f32 = 180.0;
/// How long the loudest thing recently heard stays the reference. Without this
/// a fixed dB window maps all ordinary music into the middle of the range: the
/// pad sits at half brightness and nothing punches. Normalising against a
/// recent peak restores the dynamics that make a beat read as a beat.
const GAIN_HALF_LIFE_MS: f32 = 2_500.0;
/// What fraction of a peak survives after this long.
/// The quietest reference worth dividing by, so silence stays dark instead of
/// being amplified into noise.
const GAIN_FLOOR: f32 = 0.15;
/// No sound at all.
const SILENCE: Levels = Levels { left: [0.0; BANDS], right: [0.0; BANDS] };
/// The node to listen to. Spotify's own output, not the sink everything
/// shares, so a notification does not flash the pad.
const AUDIO_SOURCE: &str = "spotify";


/// Instant attack, gradual release.
/// What fraction of a level survives one hop, given how long a hop lasts.
/// What fraction of a value survives after this long, given a half-life. The
/// audio thread runs on PipeWire's quantum and the renderer on its tick, so
/// every envelope is stated as a time and converted to whatever cadence asks.
fn survives(millis: f32, half_life: f32) -> f32 {
    0.5f32.powf(millis / half_life)
}

/// Peak-hold with decay: jump to whatever is louder, sink toward it otherwise.
/// The one shape every envelope here has -- the sound's own fall, and the
/// slower forgetting of how loud it has recently been.
fn hold(previous: f32, now: f32, survives: f32) -> f32 {
    now.max(previous * survives)
}


/// Instant attack, gradual release. Equal rise and fall reads as flicker; the
/// asymmetry is what makes it a pulse.
/// Track how loud the loudest band has been lately, so levels can be read
/// against it. Rises instantly, forgets slowly, never drops below a floor --
/// otherwise silence gets amplified into noise.
///
/// How much a rise counts for. Flux is a change in level, so a kick lifting its
/// band is a small number -- this is what turns that into something you can see.
const FLUX_GAIN: f32 = 6.0;
/// How much steady sound still shows. Pure onset detection goes dark through a
/// sustained chord, which is right for following a beat and wrong for watching
/// music; this keeps some presence under the flashes.
const LEVEL_FLOOR: f32 = 0.35;

/// What got *louder* since the last look. Steady sound cancels; a hit does not.
/// This is spectral flux, the standard way to find an onset: energy that has
/// just arrived, rather than energy that is merely present.
fn onsets(previous: Levels, now: Levels) -> Levels {
    Levels {
        left: std::array::from_fn(|b| (now.left[b] - previous.left[b]).max(0.0)),
        right: std::array::from_fn(|b| (now.right[b] - previous.right[b]).max(0.0)),
    }
}

/// A flash for what just hit, over a floor of what is merely playing.
fn punch(onsets: Levels, level: Levels) -> Levels {
    let mix = |hit: [f32; BANDS], present: [f32; BANDS]| -> [f32; BANDS] {
        std::array::from_fn(|b| {
            (hit[b] * FLUX_GAIN).max(present[b] * LEVEL_FLOOR).clamp(0.0, 1.0)
        })
    };
    Levels {
        left: mix(onsets.left, level.left),
        right: mix(onsets.right, level.right),
    }
}

/// One number for the whole pad, not one per band. A gain per band makes every
/// band use the full range, which is exactly the frequency information the rows
/// are supposed to show: a 50Hz tone came out lighting the bottom two rows
/// equally instead of mostly the bottom one.
fn track_peak(previous: f32, now: Levels, survives: f32) -> f32 {
    let loudest = now
        .left
        .iter()
        .chain(now.right.iter())
        .fold(0.0f32, |most, level| most.max(*level));
    hold(previous, loudest, survives).max(GAIN_FLOOR)
}

/// Scale everything so the loudest band recently heard reaches the top, leaving
/// the balance between bands alone.
fn against_peak(levels: Levels, peak: f32) -> Levels {
    Levels {
        left: levels.left.map(|level| (level / peak).clamp(0.0, 1.0)),
        right: levels.right.map(|level| (level / peak).clamp(0.0, 1.0)),
    }
}

fn follow(previous: Levels, now: Levels, survives: f32) -> Levels {
    Levels {
        left: std::array::from_fn(|b| hold(previous.left[b], now.left[b], survives)),
        right: std::array::from_fn(|b| hold(previous.right[b], now.right[b], survives)),
    }
}

/// One FFT's worth of samples to band levels. Hann window, because a hard edge
/// on the block smears a pure tone across every band.
/// One window of samples to one level per band: each band's *share* of the
/// energy in it, scaled by how loud the window is overall.
///
/// Share, not absolute level. Four bands span nine octaves, so a tone always
/// straddles two -- and in dB that spread is only a few decibels, which reads
/// as two rows equally lit. A share separates them: a 50Hz tone is 74% of the
/// bottom band and 26% of the next, and looks it.
///
/// Hann window, because a hard edge on the block smears a pure tone across
/// every band.
fn spectrum_levels(samples: &[f32; FFT_SIZE], sample_rate: f32) -> [f32; BANDS] {
    let window: [f32; FFT_SIZE] = std::array::from_fn(|i| {
        let phase = std::f32::consts::TAU * i as f32 / FFT_SIZE as f32;
        0.5 * (1.0 - phase.cos())
    });
    let mut planner = realfft::RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut input: Vec<f32> = samples.iter().zip(window).map(|(s, w)| s * w).collect();
    let mut output = fft.make_output_vec();
    if fft.process(&mut input, &mut output).is_err() {
        return [0.0; BANDS];
    }
    // Scale so a full-scale sine reads 0dB. Half the energy of a real signal
    // sits in the negative frequencies this transform does not return, hence
    // the 2; the window throws away the rest, hence dividing by its sum rather
    // than by the sample count. Dividing by the count alone reads 12dB quiet,
    // and the pad can then never reach full brightness.
    let scale = 2.0 / window.iter().sum::<f32>();

    let per_bin = sample_rate / FFT_SIZE as f32;
    let octaves = (HIGH_HZ / LOW_HZ).ln();
    let mut power = [0.0f32; BANDS];
    for (bin, value) in output.iter().enumerate() {
        let hz = bin as f32 * per_bin;
        if !(LOW_HZ..=HIGH_HZ).contains(&hz) {
            continue;
        }
        // Where this bin falls, measured in bands along a log scale. Its energy
        // is split between the two it lies between rather than dropped whole
        // into one -- hard edges make a rising tone jump from row to row.
        let position = (hz / LOW_HZ).ln() / octaves * (BANDS - 1) as f32;
        let lower = position.floor() as usize;
        let toward_upper = position - lower as f32;
        let energy = (value.norm() * scale).powi(2);
        power[lower] += energy * (1.0 - toward_upper);
        if lower + 1 < BANDS {
            power[lower + 1] += energy * toward_upper;
        }
    }

    let total: f32 = power.iter().sum();
    if total <= 0.0 {
        return [0.0; BANDS];
    }
    // How loud it is overall, in dB against the floor; how that loudness is
    // spread is the share of each band.
    let db = 10.0 * total.log10();
    let loudness = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
    power.map(|p| p / total * loudness)
}

/// What the audio thread accumulates between FFTs.
/// What the audio thread accumulates between FFTs, one window per channel.
struct Listener {
    format: pw::spa::param::audio::AudioInfoRaw,
    left: Vec<f32>,
    right: Vec<f32>,
    levels: Levels,
    published: Arc<Mutex<Option<(Levels, Instant)>>>,
    peak: f32,
    last: Levels,
}

/// What the music is doing, per channel.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Levels {
    left: [f32; BANDS],
    right: [f32; BANDS],
}


/// Fold one quantum of interleaved audio in and read the newest window.
///
/// The buffer holds exactly one window: whatever arrives is appended and
/// anything that no longer fits is dropped off the front. Keeping a backlog and
/// working through it in order would put the pad behind the music and it would
/// never catch up -- older audio is not news, so it goes in the bin.
fn absorb(state: &mut Listener, samples: &[u8], channels: usize) {
    let channels = channels.max(1);
    let sample = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().expect("four bytes"));
    let frames = samples.chunks_exact(4 * channels);
    let arrived = frames.len();
    for frame in frames {
        state.left.push(sample(&frame[..4]));
        state.right.push(match channels > 1 {
            true => sample(&frame[4..8]),
            false => sample(&frame[..4]),
        });
    }
    if state.left.len() < FFT_SIZE {
        return;
    }
    let stale = state.left.len() - FFT_SIZE;
    state.left.drain(..stale);
    state.right.drain(..stale);

    let rate = state.format.rate().max(1) as f32;
    let block = |w: &[f32]| -> [f32; FFT_SIZE] { std::array::from_fn(|i| w[i]) };
    let heard = Levels {
        left: spectrum_levels(&block(&state.left), rate),
        right: spectrum_levels(&block(&state.right), rate),
    };

    let since_last = arrived as f32 / rate * 1000.0;

    // Flux is measured on the real energy, *before* the gain below. The gain
    // deliberately makes every band use the whole range, so measuring change
    // after it makes a kick's faint high-frequency click as large a rise as the
    // kick -- every hit then flashes every row and the pad pulses as one.
    let hit = onsets(state.last, heard);
    state.last = heard;

    // Normalise against the loudest this band has been lately, so ordinary
    // music uses the whole range instead of sitting in the middle of it.
    state.peak = track_peak(state.peak, heard, survives(since_last, GAIN_HALF_LIFE_MS));
    let present = against_peak(heard, state.peak);

    // Show what just arrived over a floor of what is merely playing.
    let lit = punch(hit, present);
    state.levels = follow(state.levels, lit, survives(since_last, RELEASE_HALF_LIFE_MS));
    if let Ok(mut out) = state.published.lock() {
        *out = Some((state.levels, Instant::now()));
    }
}

/// Capture Spotify's own output and publish band levels. Targets the player's
/// node, so the pad reacts to music rather than to notification dings.
fn watch_audio(published: Arc<Mutex<Option<(Levels, Instant)>>>) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
    };
    props.insert(*pw::keys::TARGET_OBJECT, AUDIO_SOURCE);

    let stream = pw::stream::StreamBox::new(&core, "winry315", props)?;
    let _listener = stream
        .add_local_listener_with_user_data(Listener {
            format: Default::default(),
            left: Vec::with_capacity(FFT_SIZE * 2),
            right: Vec::with_capacity(FFT_SIZE * 2),
            levels: Levels { left: [0.0; BANDS], right: [0.0; BANDS] },
            peak: GAIN_FLOOR,
            last: SILENCE,
            published,
        })
        .param_changed(|_, state, id, param| {
            let Some(param) = param else { return };
            if id == pw::spa::param::ParamType::Format.as_raw() {
                let _ = state.format.parse(param);
                eprintln!(
                    "winry315: audio {}Hz {}ch",
                    state.format.rate(),
                    state.format.channels()
                );
            }
        })
        .process(|stream, state| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let channels = state.format.channels() as usize;
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };
            let size = data.chunk().size() as usize;
            if let Some(samples) = data.data() {
                absorb(state, &samples[..size.min(samples.len())], channels);
            }
        })
        .register()?;

    let mut audio_info = pw::spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [pw::spa::pod::Pod::from_bytes(&values).context("building format")?];

    stream.connect(
        pw::spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    mainloop.run();
    Ok(())
}

// ---------------------------------------------------------------------------
// Album art -> one colour. Slow and networked, so it runs on its own thread and
// publishes a latest value; the pad never waits for it.
// ---------------------------------------------------------------------------


/// Album art is a few hundred KB of JPEG; anything larger is not album art.
const ART_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// How often to look for a track change. Slow on purpose -- this is identity,
/// not motion.
const ART_POLL: Duration = Duration::from_secs(2);


/// Decode album art and distil it to a single colour. k-means in Oklab, so
/// clustering follows what the eye considers "a different colour" rather than
/// RGB distance.
/// The pad's 5x3 key grid, wired serpentine: columns alternate direction, so
/// there is no constant stride. Row 0 is the bottom.
/// Underglow: one column down each side, paired with the grid row it sits level
/// with. They are not evenly spaced, so the row is stated rather than derived --
/// the lowest is level with the bottom keys, the middle with the top keys, and
/// the highest with the knobs. The left column is the left channel and the
/// right column the right, so the sides are a level meter per ear.
const UNDERGLOW_LEFT: [(u8, usize); 3] = [(22, 0), (23, 2), (24, 3)];
const UNDERGLOW_RIGHT: [(u8, usize); 3] = [(21, 0), (26, 2), (25, 3)];

/// The pad as one grid, `GRID[column][row]`, bottom row first. The top row is
/// the six knob LEDs, which sit directly above the keys -- there are six of
/// them over five columns, and by position both LED 3 and LED 2 fall nearest
/// the middle, so that cell holds two. Everything else holds one.
const GRID: [[&[u8]; 4]; 5] = [
    [&[8], &[7], &[6], &[5]],
    [&[9], &[10], &[11], &[4]],
    [&[14], &[13], &[12], &[3, 2]],
    [&[15], &[16], &[17], &[1]],
    [&[20], &[19], &[18], &[0]],
];
/// The grid measures itself; nothing else gets to disagree about its shape.
const GRID_W: usize = GRID.len();
const GRID_H: usize = GRID[0].len();

/// One cover, distilled: a colour per key, plus one colour for the LEDs around
/// them. Decoded once -- the grid and the ring both come from the same image.
///
/// The grid goes through blurhash rather than a box average: it low-passes in
/// *linear* light, which is where an average turns complementary colours to
/// grey. Gamma correction on the way to the LEDs restores the saturation that
///
/// band-limiting costs.
///
/// The cover is square and the grid is 5:4, so the centre is kept and the top
/// and bottom lost -- covers put their subject in the middle.
fn album_of_art(jpeg: &[u8]) -> Result<Album> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .context("decoding album art")?
        .into_rgb8();

    let (w, h) = img.dimensions();
    let want = GRID_W as f32 / GRID_H as f32;
    let (cw, ch) = match (w as f32 / h as f32) > want {
        true => ((h as f32 * want) as u32, h),
        false => (w, (w as f32 / want) as u32),
    };
    let cropped = image::imageops::crop_imm(&img, (w - cw) / 2, (h - ch) / 2, cw, ch).to_image();
    let rgba = image::DynamicImage::ImageRgb8(cropped).into_rgba8();

    let hash = blurhash::encode(4, 3, rgba.width(), rgba.height(), rgba.as_raw())
        .map_err(|e| anyhow!("blurhash encode: {e:?}"))?;
    let small = blurhash::decode(&hash, GRID_W as u32, GRID_H as u32, 1.0)
        .map_err(|e| anyhow!("blurhash decode: {e:?}"))?;
    // Images count rows downward from the top; the pad counts them upward from
    // the bottom, because row 0 of a column is its lowest LED. Flip, or every
    // cover shows upside down.
    let grid: [[u8; 3]; GRID_W * GRID_H] = std::array::from_fn(|i| {
        let (row, column) = (i / GRID_W, i % GRID_W);
        let pixel = ((GRID_H - 1 - row) * GRID_W + column) * 4;
        [small[pixel], small[pixel + 1], small[pixel + 2]]
    });

    Ok(Album { grid })
}


/// The current track's art URL, if the player is advertising one. Spotify's is
/// a content hash, so it is stable per album -- comparing URLs is enough to
/// avoid refetching for every track on a record.
fn art_url(conn: &Connection) -> Result<Option<String>> {
    let reply = conn.call_method(
        Some(MPRIS_DEST),
        MPRIS_PATH,
        Some(DBUS_PROPS),
        "Get",
        &(MPRIS_PLAYER, "Metadata"),
    )?;
    let body = reply.body();
    // Properties.Get answers with a variant wrapping the dict, so unwrap one
    // layer before the a{sv} is reachable.
    let value: zbus::zvariant::Value = body.deserialize()?;
    let metadata = HashMap::<String, zbus::zvariant::Value>::try_from(value)
        .context("Metadata was not a dict")?;
    match metadata.get("mpris:artUrl") {
        Some(zbus::zvariant::Value::Str(url)) => Ok(Some(url.to_string())),
        _ => Ok(None),
    }
}

/// The art cache lives under `$XDG_CACHE_HOME/winry315-daemon`, keyed by the
/// URL's content hash. Network is the expensive part; re-extracting a colour
/// from a cached JPEG is milliseconds, so the image is what we keep.
fn cache_dir() -> Option<PathBuf> {
    xdg::BaseDirectories::with_prefix("winry315-daemon")
        .ok()?
        .create_cache_directory("art")
        .ok()
}

/// The last path segment of a Spotify art URL is a content hash. Reject
/// anything that is not, rather than letting a URL choose where we write.
fn cache_key(url: &str) -> Option<&str> {
    let key = url.rsplit('/').next()?;
    let sane = !key.is_empty()
        && key.len() <= 64
        && key.chars().all(|c| c.is_ascii_alphanumeric());
    sane.then_some(key)
}

/// Fetch album art, preferring the cache. A cache that cannot be read or
/// written is not an error: it just means going to the network.
fn load_art(url: &str) -> Result<Vec<u8>> {
    let cached = cache_dir().zip(cache_key(url)).map(|(dir, key)| dir.join(key));
    if let Some(path) = &cached
        && let Ok(bytes) = fs::read(path)
    {
        return Ok(bytes);
    }
    let bytes = ureq::get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?
        .body_mut()
        .with_config()
        .limit(ART_MAX_BYTES)
        .read_to_vec()
        .context("reading album art")?;
    if let Some(path) = &cached {
        // Write then rename, so a crash mid-write cannot leave a torn JPEG that
        // would poison the cache for that album forever.
        let part = path.with_extension("part");
        if fs::write(&part, &bytes).and_then(|_| fs::rename(&part, path)).is_err() {
            eprintln!("winry315: could not cache album art");
        }
    }
    Ok(bytes)
}

/// Turn one cover into everything the pad needs from it: the grid, plus one
/// colour for the LEDs outside it.
/// Is Spotify playing? Levels can only be trusted while it is: a capture stream
/// whose target is missing falls back to the default source, so with Spotify
/// closed the pad would otherwise dance to the microphone.
fn playing(conn: &Connection) -> bool {
    let Ok(reply) = conn.call_method(
        Some(MPRIS_DEST),
        MPRIS_PATH,
        Some(DBUS_PROPS),
        "Get",
        &(MPRIS_PLAYER, "PlaybackStatus"),
    ) else {
        return false;
    };
    let body = reply.body();
    matches!(
        body.deserialize::<zbus::zvariant::Value>(),
        Ok(zbus::zvariant::Value::Str(status)) if status == "Playing"
    )
}

/// Watch what the player is doing: which album, and whether it is running at
/// all. Every step here is allowed to fail -- no network, no player, art that
/// decodes to nothing -- and the pad just carries on.
fn watch_art(tx: mpsc::Sender<Input>) {
    let Ok(conn) = Connection::session() else {
        return;
    };
    let mut current = None;
    let mut was_playing = false;
    loop {
        thread::sleep(ART_POLL);

        let now_playing = playing(&conn);
        if now_playing != was_playing {
            was_playing = now_playing;
            if tx.send(Input::Playing(now_playing)).is_err() {
                return;
            }
        }

        let seen = art_url(&conn).unwrap_or(None);
        if seen == current {
            continue;
        }
        current = seen;
        let album = match &current {
            None => None,
            Some(url) => match load_art(url).and_then(|jpeg| album_of_art(&jpeg)) {
                Ok(album) => Some(album),
                Err(e) => {
                    eprintln!("winry315: album art: {e:#}");
                    None
                }
            },
        };
        eprintln!("winry315: album {}", if album.is_some() { "loaded" } else { "none" });
        if tx.send(Input::Album(album)).is_err() {
            return;
        }
    }
}
// ---------------------------------------------------------------------------
// Spotify, over MPRIS. The bus name is owned by bubbled-spotify's dbus proxy,
// not Spotify itself; the proxy's --dbus-own grant is what makes this reachable.
// ---------------------------------------------------------------------------

const MPRIS_DEST: &str = "org.mpris.MediaPlayer2.spotify";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const DBUS_PROPS: &str = "org.freedesktop.DBus.Properties";

fn player_call<B>(conn: &Connection, method: &str, body: &B) -> Result<()>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(MPRIS_DEST), MPRIS_PATH, Some(MPRIS_PLAYER), method, body)
        .with_context(|| format!("MPRIS {method}"))?;
    Ok(())
}


fn volume(conn: &Connection) -> Result<f64> {
    let reply = conn
        .call_method(
            Some(MPRIS_DEST),
            MPRIS_PATH,
            Some(DBUS_PROPS),
            "Get",
            &(MPRIS_PLAYER, "Volume"),
        )
        .context("reading MPRIS Volume")?;
    let body = reply.body();
    let value: zbus::zvariant::Value = body.deserialize()?;
    f64::try_from(value).context("MPRIS Volume was not a double")
}

fn set_volume(conn: &Connection, level: f64) -> Result<()> {
    conn.call_method(
        Some(MPRIS_DEST),
        MPRIS_PATH,
        Some(DBUS_PROPS),
        "Set",
        &(
            MPRIS_PLAYER,
            "Volume",
            zbus::zvariant::Value::F64(level).try_to_owned()?,
        ),
    )
    .context("setting MPRIS Volume")?;
    Ok(())
}

/// Run one action. Spotify not being up is ordinary, not fatal: the pad keeps
/// working and the error goes to stderr.
fn run_action(conn: &Connection, action: Action) -> Result<()> {
    match action {
        Action::PlayPause => player_call(conn, "PlayPause", &()),
        Action::NextTrack => player_call(conn, "Next", &()),
        Action::PrevTrack => player_call(conn, "Previous", &()),
        Action::Seek(offset) => player_call(conn, "Seek", &(offset,)),
        Action::VolumeStep(detents) => {
            let level = (volume(conn)? + detents as f64 * VOLUME_STEP).clamp(0.0, 1.0);
            set_volume(conn, level)
        }
    }
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

fn send(pad: &Mutex<fs::File>, payload: [u8; REPORT_SIZE]) -> Result<()> {
    pad.lock()
        .map_err(|_| anyhow!("pad writer poisoned"))?
        .write_all(&frame(payload))
        .context("writing to pad")
}

fn main() -> Result<()> {
    let path = find_pad()?;
    eprintln!("winry315: using {path}");

    let pad = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {path}"))?;
    let mut reader = pad.try_clone()?;
    // Liveness and pixels are separate concerns on separate schedules, so the
    // write end is shared: the heartbeat thread proves the daemon is alive, the
    // event loop says what to draw.
    let writer = Arc::new(Mutex::new(pad));

    let (tx, rx) = mpsc::channel();
    let pad_tx = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; REPORT_SIZE];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || pad_tx.send(Input::Report(buf)).is_err() {
                break;
            }
        }
    });

    // Album art is networked and slow, so it publishes a latest value like any
    // other input rather than the pad waiting on it.
    thread::spawn(move || watch_art(tx));

    // The pad gives up on a silent host after 2s. Pinging on its own thread
    // means no amount of work in the event loop -- a long knob sweep, a slow
    // D-Bus call -- can starve it.
    let beat = Arc::clone(&writer);
    thread::spawn(move || {
        let mut ping = [0u8; REPORT_SIZE];
        ping[0] = CMD_PING;
        loop {
            thread::sleep(HEARTBEAT);
            if send(&beat, ping).is_err() {
                return;
            }
        }
    });

    // The music scales the picture. A pad with no audio source just does not
    // pulse; nothing else changes.
    let audio = Arc::new(Mutex::new(None));
    let heard = Arc::clone(&audio);
    thread::spawn(move || {
        if let Err(e) = watch_audio(heard) {
            eprintln!("winry315: audio: {e:#}");
        }
    });

    // Spotify mode needs the session bus, but a missing one must not stop the
    // pad from working as a colour picker.
    let bus = Connection::session()
        .map_err(|e| eprintln!("winry315: no session bus, Spotify mode inert: {e}"))
        .ok();

    let mut state = State {
        mode: Mode::Colour,
        rgb: [0, 0, 0],
        album: None,
        playing: false,
    };
    let mut shown = [[0u8; 3]; LED_COUNT];
    let mut sent = shown;
    let mut heard = SILENCE;
    for report in led_reports(&sent) {
        send(&writer, report)?;
    }

    // Fold every input into one desired state, then ease the pad toward it.
    // Liveness is the heartbeat thread's job, not this loop's.
    loop {
        let next = match rx.recv_timeout(TICK) {
            Ok(Input::Album(album)) => State { album, ..state },
            Ok(Input::Playing(playing)) => State { playing, ..state },
            Ok(Input::Report(report)) => match parse_event(&report) {
                None => state,
                Some(event) => {
                    let (next, action) = apply(state, event);
                    if let Some(action) = action {
                        match &bus {
                            Some(conn) => {
                                if let Err(e) = run_action(conn, action) {
                                    eprintln!("winry315: {action:?} failed: {e:#}");
                                }
                            }
                            None => eprintln!("winry315: {action:?} dropped, no session bus"),
                        }
                    }
                    next
                }
            },
            // A tick with nothing to say still advances the ease.
            Err(mpsc::RecvTimeoutError::Timeout) => state,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("winry315: pad disconnected");
                return Ok(());
            }
        };

        if next != state {
            state = next;
            eprintln!("winry315: {:?}", state.mode);
        }

        // Colours ease toward the target; the music scales what comes out and
        // is never eased. Send whatever actually differs from the last frame,
        // so a settled pad with silent music goes quiet on the wire.
        shown = ease_cells(&shown, &desired_cells(state));

        // Decay on this loop's clock, not the audio thread's. When the music
        // stops the capture callback simply stops firing, so levels left in the
        // slot would otherwise stay lit until PipeWire tore the stream down --
        // the pad held its last frame for ten seconds after a pause.
        //
        // And only while the player is actually running: a capture stream whose
        // target is missing silently falls back to the default source, so with
        // Spotify closed these levels are the microphone.
        let fresh = state
            .playing
            .then(|| audio.lock().ok().and_then(|l| *l))
            .flatten()
            .filter(|(_, at)| at.elapsed() < AUDIO_TIMEOUT)
            .map(|(levels, _)| levels);
        heard = follow(heard, fresh.unwrap_or(SILENCE), survives(TICK_MS, RELEASE_HALF_LIFE_MS));

        let lit = match state.mode {
            Mode::Spotify => modulate(&shown, heard),
            _ => shown,
        };
        if lit != sent {
            sent = lit;
            for report in led_reports(&sent) {
                send(&writer, report)?;
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Most tests care only about the state half of the transition.
    fn next_state(state: State, event: PadEvent) -> State {
        apply(state, event).0
    }

    fn colour(rgb: [u8; 3]) -> State {
        State {
            mode: Mode::Colour,
            rgb,
            album: None,
            playing: true,
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
        let framed = frame(led_reports(&desired_cells(colour([1, 2, 3])))[0]);
        assert_eq!(framed.len(), REPORT_SIZE + 1);
        assert_eq!(&framed[..3], &[0x00, CMD_LEDS, 0]);
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
            next_state(colour([0, 0, 0]), PadEvent::Encoder { index: 1, delta: 1 }).rgb,
            [0, COLOUR_STEP as u8, 0]
        );
        assert_eq!(
            next_state(colour([0, 0, 0]), PadEvent::Encoder { index: 2, delta: 2 }).rgb,
            [0, 0, 2 * COLOUR_STEP as u8]
        );
    }

    #[test]
    fn bottom_row_selects_modes_and_other_keys_do_not() {
        for (key, _, mode) in MODE_SLOTS {
            let next = next_state(
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
            next_state(colour([1, 2, 3]), PadEvent::Key { index: 15, pressed: true }),
            colour([1, 2, 3])
        );
    }

    #[test]
    fn spotify_maps_every_knob_and_click() {
        let s = State { mode: Mode::Spotify, rgb: [0, 0, 0], album: None, playing: true };
        let act = |e| apply(s, e).1;

        // Both side knobs scrub, and direction comes from the rotation.
        for index in [ENCODER_LEFT, ENCODER_RIGHT] {
            assert_eq!(
                act(PadEvent::Encoder { index, delta: 1 }),
                Some(Action::Seek(SEEK_STEP_US))
            );
            assert_eq!(
                act(PadEvent::Encoder { index, delta: -1 }),
                Some(Action::Seek(-SEEK_STEP_US))
            );
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

        // Releases must not double-fire, and mode keys stay mode keys.
        assert_eq!(act(PadEvent::Key { index: KEY_ENCODER_CENTRE, pressed: false }), None);
        assert_eq!(apply(s, click(10)).0.mode, Mode::Colour);
    }



    #[test]
    fn cache_key_accepts_content_hashes_and_rejects_paths() {
        assert_eq!(
            cache_key("https://i.scdn.co/image/ab67616d0000b27364096943f064249144d1a972"),
            Some("ab67616d0000b27364096943f064249144d1a972")
        );
        // Only the last segment is ever used, so a key can never contain a
        // separator and the join can never leave the cache directory.
        for url in ["https://evil/../../etc/passwd", "https://x/a/b/c"] {
            assert!(cache_key(url).is_none_or(|k| !k.contains('/')));
        }
        assert_eq!(cache_key("https://evil/.."), None);
        assert_eq!(cache_key("https://evil/a b"), None);
        assert_eq!(cache_key("https://evil/"), None);
        assert_eq!(cache_key(&format!("https://evil/{}", "a".repeat(65))), None);
    }


    #[test]
    fn colour_mode_raises_no_actions() {
        let s = colour([0, 0, 0]);
        assert_eq!(apply(s, PadEvent::Encoder { index: 0, delta: 1 }).1, None);
        assert_eq!(apply(s, PadEvent::Key { index: KEY_ENCODER_CENTRE, pressed: true }).1, None);
    }

    #[test]
    fn the_pad_is_washed_with_the_colour_and_still_shows_its_indicators() {
        let cells = desired_cells(colour([9, 8, 7]));
        // LED 21 is outside the key grid, so it carries the plain wash.
        assert_eq!(cells[21], [9, 8, 7]);
        // Indicators sit on top: colour mode active, spotify dimmed.
        assert_eq!(cells[8], Mode::Colour.tint());
        assert_eq!(cells[9], dimmed(Mode::Spotify.tint()));
    }

    #[test]
    fn the_grid_table_matches_the_board() {
        // From QMK's winry315.c: LED positions in mm from the PCB centre, y up.
        // Anything that disagrees with this puts the picture on its head.
        let position = |led: u8| -> (i32, i32) {
            match led {
                0 => (35, 36), 1 => (21, 36), 2 => (8, 34),
                3 => (-8, 34), 4 => (-21, 36), 5 => (-35, 36),
                6 => (-38, 5), 7 => (-38, -14), 8 => (-38, -33),
                9 => (-19, -33), 10 => (-19, -14), 11 => (-19, 5),
                12 => (0, 5), 13 => (0, -14), 14 => (0, -33),
                15 => (19, -33), 16 => (19, -14), 17 => (19, 5),
                18 => (38, 5), 19 => (38, -14), 20 => (38, -33),
                other => panic!("LED {other} is not on the grid"),
            }
        };
        let mut previous_x = i32::MIN;
        for (column, rows) in GRID.iter().enumerate() {
            // Rows climb: row 0 is the lowest, and the knob row is above them.
            let ys: Vec<i32> = rows.iter().map(|leds| position(leds[0]).1).collect();
            assert!(ys.windows(2).all(|p| p[0] < p[1]), "column {column} runs down: {ys:?}");
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
    fn the_album_lands_on_the_grid_and_carries_on_down_the_sides() {
        let grid = std::array::from_fn(|i| [i as u8 * 4, 0, 0]);
        let state = State {
            mode: Mode::Spotify,
            rgb: [0, 0, 0],
            album: Some(Album { grid }),
            playing: true,
        };
        let cells = desired_cells(state);

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
        // Indicators still win over the cover.
        assert_eq!(cells[9], Mode::Spotify.tint());
    }

    /// A PipeWire quantum at 48kHz, which is how often levels are refreshed.
    const QUANTUM_MS: f32 = 256.0 / 48.0;

    fn tone(hz: f32, rate: f32) -> [f32; FFT_SIZE] {
        std::array::from_fn(|i| (std::f32::consts::TAU * hz * i as f32 / rate).sin())
    }

    #[test]
    fn pitch_climbs_the_pad_smoothly() {
        let rate = 48_000.0;
        let brightest = |hz: f32| {
            let levels = spectrum_levels(&tone(hz, rate), rate);
            (0..BANDS).max_by(|a, b| levels[*a].total_cmp(&levels[*b])).unwrap()
        };
        // Rising pitch never moves down the pad, and spans it end to end.
        let climb: Vec<usize> = [40.0, 200.0, 800.0, 3000.0, 10_000.0]
            .iter()
            .map(|hz| brightest(*hz))
            .collect();
        assert!(climb.windows(2).all(|p| p[0] <= p[1]), "pitch fell: {climb:?}");
        assert_eq!(climb[0], 0, "the lowest tone belongs at the bottom");
        assert_eq!(*climb.last().unwrap(), BANDS - 1, "the highest at the top");

        // And a tone between two bands lights both, rather than snapping to one.
        let between = spectrum_levels(&tone(2000.0, rate), rate);
        let lit = between.iter().filter(|l| **l > 0.0).count();
        assert!(lit >= 2, "a tone between bands lit only {lit}: {between:?}");
    }

    #[test]
    fn a_tone_lights_one_row_and_not_its_neighbour_equally() {
        // The bug: a level in dB put a 50Hz tone at 89 and 82 on two rows --
        // near enough identical -- because a few dB of difference is a few
        // percent of the window. A share of the energy separates them.
        let levels = spectrum_levels(&tone(50.0, 48_000.0), 48_000.0);
        assert!(
            levels[0] > levels[1] * 2.0,
            "50Hz spread across rows as {levels:?}"
        );
    }


    #[test]
    fn silence_is_dark() {
        let levels = spectrum_levels(&[0.0; FFT_SIZE], 48_000.0);
        assert!(levels.iter().all(|l| *l == 0.0), "silence gave {levels:?}");
    }

    fn level(all: f32) -> Levels {
        Levels { left: [all; BANDS], right: [all; BANDS] }
    }

    #[test]
    fn attack_is_instant_and_release_is_not() {
        let step = survives(QUANTUM_MS, RELEASE_HALF_LIFE_MS);
        // A hit lands whole, on the frame it happens.
        assert_eq!(follow(level(0.1), level(0.9), step), level(0.9));
        // Letting go does not: the level falls over many frames.
        let mut now = follow(level(0.9), SILENCE, step);
        assert!(now.left[0] > 0.0 && now.left[0] < 0.9, "fell to {now:?} at once");
        for _ in 0..(2_000.0 / QUANTUM_MS) as usize {
            now = follow(now, SILENCE, step);
        }
        assert!(now.left[0] < 0.01, "never finished falling: {now:?}");
    }

    #[test]
    fn release_is_a_time_not_a_frame_count() {
        // One half-life halves the level, whatever the update cadence -- the
        // audio thread runs on PipeWire's quantum and the renderer on its tick,
        // and the same music has to fade the same way through either.
        for cadence in [QUANTUM_MS, QUANTUM_MS * 4.0, TICK_MS] {
            let mut level = 1.0f32;
            for _ in 0..(RELEASE_HALF_LIFE_MS / cadence).round() as usize {
                level *= survives(cadence, RELEASE_HALF_LIFE_MS);
            }
            assert!((level - 0.5).abs() < 0.03, "cadence {cadence} landed at {level}");
        }
    }

    #[test]
    fn frequency_runs_up_the_pad_and_stereo_runs_across_it() {
        let cells = [[200u8; 3]; LED_COUNT];

        // Hard left, all bands: bright on the left column, dark on the right.
        let left_only = Levels { left: [1.0; BANDS], right: [0.0; BANDS] };
        let lit = modulate(&cells, left_only);
        assert_eq!(lit[GRID[0][0][0] as usize], [200; 3]);
        assert_eq!(lit[GRID[GRID_W - 1][0][0] as usize], [0; 3]);
        // The middle column hears both channels equally.
        assert_eq!(lit[GRID[GRID_W / 2][0][0] as usize], [100; 3]);

        // Bass only, centred: the bottom row lights and the top row does not.
        let mut bass = [0.0; BANDS];
        bass[0] = 1.0;
        let lit = modulate(&cells, Levels { left: bass, right: bass });
        for column in GRID {
            assert_eq!(lit[column[0][0] as usize], [200; 3], "bottom row is the low band");
            assert_eq!(lit[column[BANDS - 1][0] as usize], [0; 3], "top row is the high band");
        }
    }

    #[test]
    fn a_lean_shows_as_a_gradient_whatever_the_volume() {
        // Pan is a share of the row's own energy, so a quiet band leaning hard
        // tilts as far as a loud one. Measuring the raw difference instead made
        // quiet material look centred.
        for loudness in [0.1f32, 1.0] {
            let levels = Levels {
                left: [loudness * 0.25; BANDS],
                right: [loudness * 0.75; BANDS],
            };
            let lit = modulate(&[[200u8; 3]; LED_COUNT], levels);
            let left_end = lit[GRID[0][0][0] as usize][0] as f32;
            let right_end = lit[GRID[GRID_W - 1][0][0] as usize][0] as f32;
            assert!(
                right_end > left_end * 2.0,
                "at {loudness} the pad read {left_end} to {right_end}"
            );
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
    fn the_sides_meter_one_channel_each() {
        let cells = [[200u8; 3]; LED_COUNT];
        let lit = modulate(&cells, Levels { left: [1.0; BANDS], right: [0.0; BANDS] });
        // Hard left: the left column is lit all the way down, right is out.
        for (led, _) in UNDERGLOW_LEFT {
            assert_eq!(lit[led as usize], [200; 3]);
        }
        for (led, _) in UNDERGLOW_RIGHT {
            assert_eq!(lit[led as usize], [0; 3]);
        }

        // Each side LED shows the band of the row it sits level with, so bass
        // alone lights only the lowest of the three.
        let mut bass = [0.0; BANDS];
        bass[0] = 1.0;
        let lit = modulate(&cells, Levels { left: bass, right: bass });
        for (led, row) in UNDERGLOW_LEFT {
            let wanted = if row == 0 { [200; 3] } else { [0; 3] };
            assert_eq!(lit[led as usize], wanted, "LED {led} sits on row {row}");
        }
    }

    #[test]
    fn only_the_newest_window_is_ever_analysed() {
        // Desired state, not a queue: whatever arrives is appended and anything
        // that no longer fits falls off the front. Working through a backlog in
        // order would put the pad behind the music with no way to catch up.
        let mut buffer: Vec<f32> = (0..FFT_SIZE * 3).map(|i| i as f32).collect();
        let stale = buffer.len() - FFT_SIZE;
        buffer.drain(..stale);
        assert_eq!(buffer.len(), FFT_SIZE);
        // What is left ends at the newest sample, not the oldest.
        assert_eq!(*buffer.last().unwrap(), (FFT_SIZE * 3 - 1) as f32);
    }

    #[test]
    fn a_stopped_player_means_no_levels_at_all() {
        // A capture stream whose target is missing falls back to the default
        // source -- verified: a nonsense target still captured live audio. So
        // with the player stopped, whatever arrives is not its music.
        let grid = std::array::from_fn(|_| [200u8; 3]);
        let playing = State {
            mode: Mode::Spotify,
            rgb: [0, 0, 0],
            album: Some(Album { grid }),
            playing: true,
        };
        let stopped = State { playing: false, ..playing };
        // Same cover either way; the difference is whether levels are believed.
        assert_eq!(desired_cells(playing), desired_cells(stopped));
        assert!(!stopped.playing, "a stopped player must gate the levels");
    }

    #[test]
    fn the_lights_go_out_when_the_music_stops() {
        // The bug: when playback stops the capture callback stops firing, so
        // the last levels sat in the slot and the pad stayed lit for ten
        // seconds. Decay has to run on the renderer's clock regardless.
        let release = survives(TICK_MS, RELEASE_HALF_LIFE_MS);
        let mut heard = Levels { left: [1.0; BANDS], right: [1.0; BANDS] };
        let ticks_in_a_second = (1000.0 / TICK_MS) as usize;
        for _ in 0..ticks_in_a_second {
            heard = follow(heard, SILENCE, release);
        }
        let brightest = heard.left.iter().chain(heard.right.iter()).fold(0.0f32, |a, b| a.max(*b));
        assert!(brightest < 0.05, "still lit after a second: {brightest}");
    }

    #[test]
    fn a_hit_reads_and_a_held_note_does_not() {
        // The muddiness this fixes: music has energy in every band all the
        // time, so a level display sits high and a kick barely moves it.
        let quiet = level(0.3);
        let loud = level(0.9);

        // Steady sound: no change, so only the presence floor shows.
        let steady = punch(onsets(loud, loud), loud);
        assert!((steady.left[0] - 0.9 * LEVEL_FLOOR).abs() < 0.01, "{steady:?}");

        // A hit of the same size against that steady background reads far
        // brighter than the background itself.
        let hit = punch(onsets(quiet, loud), loud);
        assert!(hit.left[0] > steady.left[0] * 2.0, "hit {hit:?} vs steady {steady:?}");
    }

    #[test]
    fn a_faint_band_does_not_flash_as_hard_as_a_loud_one() {
        // Onsets are broadband: a kick's click reaches every band. Measured on
        // real energy the high band's share stays small, so the top row does
        // not flash as hard as the bottom. Measured after per-band gain they
        // came out equal and the whole pad pulsed with the bass.
        let before = Levels { left: [0.10, 0.0, 0.0, 0.02], right: [0.10, 0.0, 0.0, 0.02] };
        let after = Levels { left: [0.70, 0.0, 0.0, 0.10], right: [0.70, 0.0, 0.0, 0.10] };
        let hit = onsets(before, after);
        assert!(
            hit.left[0] > hit.left[BANDS - 1] * 5.0,
            "low {} against high {}",
            hit.left[0],
            hit.left[BANDS - 1]
        );
    }
    #[test]
    fn a_fade_out_never_flashes() {
        // Onsets are rises only: getting quieter must not light anything.
        let falling = onsets(level(0.9), level(0.2));
        assert!(falling.left.iter().all(|f| *f == 0.0), "{falling:?}");
    }
    #[test]
    fn loud_and_quiet_music_both_use_the_whole_range() {
        // Without this the dB window maps ordinary music into the middle and
        // the pad glows at half brightness whatever is playing.
        let decay = survives(QUANTUM_MS, GAIN_HALF_LIFE_MS);
        for loudness in [0.2f32, 0.5, 0.9] {
            let heard = Levels { left: [loudness; BANDS], right: [loudness; BANDS] };
            let mut peak = GAIN_FLOOR;
            for _ in 0..50 {
                peak = track_peak(peak, heard, decay);
            }
            assert!(
                against_peak(heard, peak).left[0] > 0.9,
                "steady {loudness} settled low"
            );
        }
    }

    #[test]
    fn the_gain_leaves_the_balance_between_bands_alone() {
        // A gain per band makes every band use the whole range, which erases
        // the frequency information the rows exist to show: a 50Hz tone lit the
        // bottom two rows equally instead of mostly the bottom one.
        let heard = Levels {
            left: [0.8, 0.2, 0.0, 0.0],
            right: [0.8, 0.2, 0.0, 0.0],
        };
        let mut peak = GAIN_FLOOR;
        for _ in 0..200 {
            peak = track_peak(peak, heard, survives(QUANTUM_MS, GAIN_HALF_LIFE_MS));
        }
        let after = against_peak(heard, peak);
        let ratio = after.left[0] / after.left[1];
        assert!((ratio - 4.0).abs() < 0.2, "balance became {ratio}:1, not 4:1");
    }

    #[test]
    fn silence_darkens_the_pad_and_full_level_leaves_it_alone() {
        let cells = [[123u8, 45, 67]; LED_COUNT];
        assert_eq!(modulate(&cells, level(0.0)), [[0u8; 3]; LED_COUNT]);
        assert_eq!(modulate(&cells, level(1.0)), cells);
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

    #[test]
    fn every_led_is_sent_exactly_once_and_dimmed_on_the_way() {
        let cells: [[u8; 3]; LED_COUNT] = std::array::from_fn(|i| [i as u8, 255 - i as u8, 128]);
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

