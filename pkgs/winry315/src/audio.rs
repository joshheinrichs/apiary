//! Listening to a PipeWire node and turning it into light.
//!
//! One band per row of the grid, one meter per channel down the sides. Any
//! mode that wants to react to sound opens a [`Meter`] on the node it cares
//! about; modes do not share one, because they do not listen to the same
//! thing.

use crate::grid::{Cells, GRID, GRID_H, GRID_W, LED_COUNT, UNDERGLOW_LEFT, UNDERGLOW_RIGHT};
use anyhow::{Context, Result};
use pipewire as pw;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One band per *row*: frequency runs up the pad, low at the bottom. Stereo
/// position runs across it. Each axis carries one thing.
pub const BANDS: usize = GRID_H;
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
/// The quietest reference worth dividing by, so silence stays dark instead of
/// being amplified into noise.
const GAIN_FLOOR: f32 = 0.15;
/// How much a rise counts for. Flux is a change in level, so a kick lifting its
/// band is a small number -- this is what turns that into something you can see.
const FLUX_GAIN: f32 = 6.0;
/// How much steady sound still shows. Pure onset detection goes dark through a
/// sustained chord, which is right for following a beat and wrong for watching
/// music; this keeps some presence under the flashes.
const LEVEL_FLOOR: f32 = 0.35;
/// Levels older than this are not news any more: the sound has stopped.
const AUDIO_TIMEOUT: Duration = Duration::from_millis(200);
/// No sound at all.
pub const SILENCE: Levels = Levels {
    left: [0.0; BANDS],
    right: [0.0; BANDS],
};

/// What the sound is doing, per channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub left: [f32; BANDS],
    pub right: [f32; BANDS],
}

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
fn follow(previous: Levels, now: Levels, survives: f32) -> Levels {
    Levels {
        left: std::array::from_fn(|b| hold(previous.left[b], now.left[b], survives)),
        right: std::array::from_fn(|b| hold(previous.right[b], now.right[b], survives)),
    }
}

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
            (hit[b] * FLUX_GAIN)
                .max(present[b] * LEVEL_FLOOR)
                .clamp(0.0, 1.0)
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

/// Scale the picture by what the sound is doing. Frequency runs up the pad and
/// stereo runs across it:
///
///     level = (left + right) / 2                  how loud this band is
///     tilt  = (right - left) / (right + left)     which way it leans, -1..1
///     gain(x, y) = level[y] * (1 + tilt[y] * x)   for x across -1..1
///
/// Applied after easing and never eased itself -- a hit has to land on the
/// frame it happens, or it stops reading as a hit.
pub fn modulate(cells: &Cells, levels: Levels) -> Cells {
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

/// A live read of one PipeWire node, as levels the renderer can scale by.
///
/// The published value is the newest, never a queue: a late frame is a wrong
/// frame. Cloning a meter shares the same capture.
#[derive(Clone)]
pub struct Meter(Arc<Mutex<Option<(Levels, Instant)>>>);

impl Meter {
    /// Start capturing `target` on its own thread. A missing node is not fatal
    /// -- the meter simply reads silence and whatever wanted it does not pulse.
    pub fn watch(target: &'static str) -> Meter {
        let meter = Meter(Arc::new(Mutex::new(None)));
        let published = Arc::clone(&meter.0);
        thread::spawn(move || {
            if let Err(e) = capture(target, published) {
                eprintln!("winry315: audio {target}: {e:#}");
            }
        });
        meter
    }

    /// What the meter shows now, having decayed from what it showed `millis`
    /// ago. Decay runs on the renderer's clock, not the audio thread's: when
    /// the sound stops the capture callback simply stops firing, so levels left
    /// in the slot would otherwise stay lit -- the pad held its last frame for
    /// ten seconds after a pause.
    ///
    /// `live` is the caller's own gate. A capture stream whose target is
    /// missing silently falls back to the default source, so a mode that knows
    /// its source is not running must say so rather than dance to the
    /// microphone.
    pub fn tick(&self, heard: Levels, live: bool) -> Levels {
        let fresh = live
            .then(|| self.0.lock().ok().and_then(|l| *l))
            .flatten()
            .filter(|(_, at)| at.elapsed() < AUDIO_TIMEOUT)
            .map(|(levels, _)| levels);
        follow(
            heard,
            fresh.unwrap_or(SILENCE),
            survives(crate::TICK_MS, RELEASE_HALF_LIFE_MS),
        )
    }
}

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

/// Fold one quantum of interleaved audio in and read the newest window.
///
/// The buffer holds exactly one window: whatever arrives is appended and
/// anything that no longer fits is dropped off the front. Keeping a backlog and
/// working through it in order would put the pad behind the sound and it would
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
    // sound uses the whole range instead of sitting in the middle of it.
    state.peak = track_peak(state.peak, heard, survives(since_last, GAIN_HALF_LIFE_MS));
    let present = against_peak(heard, state.peak);

    // Show what just arrived over a floor of what is merely playing.
    let lit = punch(hit, present);
    state.levels = follow(
        state.levels,
        lit,
        survives(since_last, RELEASE_HALF_LIFE_MS),
    );
    if let Ok(mut out) = state.published.lock() {
        *out = Some((state.levels, Instant::now()));
    }
}

/// Capture one node's output and publish band levels.
fn capture(target: &str, published: Arc<Mutex<Option<(Levels, Instant)>>>) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
    };
    props.insert(*pw::keys::TARGET_OBJECT, target);

    let stream = pw::stream::StreamBox::new(&core, "winry315", props)?;
    let _listener = stream
        .add_local_listener_with_user_data(Listener {
            format: Default::default(),
            left: Vec::with_capacity(FFT_SIZE * 2),
            right: Vec::with_capacity(FFT_SIZE * 2),
            levels: SILENCE,
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
            let Some(data) = datas.first_mut() else {
                return;
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICK_MS;

    /// A PipeWire quantum at 48kHz, which is how often levels are refreshed.
    const QUANTUM_MS: f32 = 256.0 / 48.0;

    fn tone(hz: f32, rate: f32) -> [f32; FFT_SIZE] {
        std::array::from_fn(|i| (std::f32::consts::TAU * hz * i as f32 / rate).sin())
    }

    fn level(all: f32) -> Levels {
        Levels {
            left: [all; BANDS],
            right: [all; BANDS],
        }
    }

    #[test]
    fn pitch_climbs_the_pad_smoothly() {
        let rate = 48_000.0;
        let brightest = |hz: f32| {
            let levels = spectrum_levels(&tone(hz, rate), rate);
            (0..BANDS)
                .max_by(|a, b| levels[*a].total_cmp(&levels[*b]))
                .unwrap()
        };
        // Rising pitch never moves down the pad, and spans it end to end.
        let climb: Vec<usize> = [40.0, 200.0, 800.0, 3000.0, 10_000.0]
            .iter()
            .map(|hz| brightest(*hz))
            .collect();
        assert!(
            climb.windows(2).all(|p| p[0] <= p[1]),
            "pitch fell: {climb:?}"
        );
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

    #[test]
    fn attack_is_instant_and_release_is_not() {
        let step = survives(QUANTUM_MS, RELEASE_HALF_LIFE_MS);
        // A hit lands whole, on the frame it happens.
        assert_eq!(follow(level(0.1), level(0.9), step), level(0.9));
        // Letting go does not: the level falls over many frames.
        let mut now = follow(level(0.9), SILENCE, step);
        assert!(
            now.left[0] > 0.0 && now.left[0] < 0.9,
            "fell to {now:?} at once"
        );
        for _ in 0..(2_000.0 / QUANTUM_MS) as usize {
            now = follow(now, SILENCE, step);
        }
        assert!(now.left[0] < 0.01, "never finished falling: {now:?}");
    }

    #[test]
    fn release_is_a_time_not_a_frame_count() {
        // One half-life halves the level, whatever the update cadence -- the
        // audio thread runs on PipeWire's quantum and the renderer on its tick,
        // and the same sound has to fade the same way through either.
        for cadence in [QUANTUM_MS, QUANTUM_MS * 4.0, TICK_MS] {
            let mut level = 1.0f32;
            for _ in 0..(RELEASE_HALF_LIFE_MS / cadence).round() as usize {
                level *= survives(cadence, RELEASE_HALF_LIFE_MS);
            }
            assert!(
                (level - 0.5).abs() < 0.03,
                "cadence {cadence} landed at {level}"
            );
        }
    }

    #[test]
    fn the_lights_go_out_when_the_sound_stops() {
        // The bug: when playback stops the capture callback stops firing, so
        // the last levels sat in the slot and the pad stayed lit for ten
        // seconds. Decay has to run on the renderer's clock regardless.
        let release = survives(TICK_MS, RELEASE_HALF_LIFE_MS);
        let mut heard = level(1.0);
        let ticks_in_a_second = (1000.0 / TICK_MS) as usize;
        for _ in 0..ticks_in_a_second {
            heard = follow(heard, SILENCE, release);
        }
        let brightest = heard
            .left
            .iter()
            .chain(heard.right.iter())
            .fold(0.0f32, |a, b| a.max(*b));
        assert!(brightest < 0.05, "still lit after a second: {brightest}");
    }

    #[test]
    fn a_silent_meter_fades_and_a_dead_one_never_lights() {
        // A meter nothing has published to reads silence, so a mode whose
        // source is absent simply does not pulse.
        let meter = Meter(Arc::new(Mutex::new(None)));
        assert_eq!(meter.tick(SILENCE, true), SILENCE);
        // And an ungated meter is never read at all, however fresh its slot.
        let live = Meter(Arc::new(Mutex::new(Some((level(1.0), Instant::now())))));
        assert_eq!(live.tick(SILENCE, false), SILENCE);
        assert_eq!(live.tick(SILENCE, true), level(1.0));
    }

    #[test]
    fn stale_levels_are_not_news() {
        // Older than the timeout means the sound stopped; the slot is ignored
        // rather than held.
        let old = Instant::now() - AUDIO_TIMEOUT * 2;
        let meter = Meter(Arc::new(Mutex::new(Some((level(1.0), old)))));
        let faded = meter.tick(level(1.0), true);
        assert!(faded.left[0] < 1.0, "stale levels were believed: {faded:?}");
    }

    #[test]
    fn a_hit_reads_and_a_held_note_does_not() {
        // The muddiness this fixes: sound has energy in every band all the
        // time, so a level display sits high and a kick barely moves it.
        let quiet = level(0.3);
        let loud = level(0.9);

        // Steady sound: no change, so only the presence floor shows.
        let steady = punch(onsets(loud, loud), loud);
        assert!(
            (steady.left[0] - 0.9 * LEVEL_FLOOR).abs() < 0.01,
            "{steady:?}"
        );

        // A hit of the same size against that steady background reads far
        // brighter than the background itself.
        let hit = punch(onsets(quiet, loud), loud);
        assert!(
            hit.left[0] > steady.left[0] * 2.0,
            "hit {hit:?} vs steady {steady:?}"
        );
    }

    #[test]
    fn a_faint_band_does_not_flash_as_hard_as_a_loud_one() {
        // Onsets are broadband: a kick's click reaches every band. Measured on
        // real energy the high band's share stays small, so the top row does
        // not flash as hard as the bottom. Measured after per-band gain they
        // came out equal and the whole pad pulsed with the bass.
        let before = Levels {
            left: [0.10, 0.0, 0.0, 0.02],
            right: [0.10, 0.0, 0.0, 0.02],
        };
        let after = Levels {
            left: [0.70, 0.0, 0.0, 0.10],
            right: [0.70, 0.0, 0.0, 0.10],
        };
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
    fn loud_and_quiet_sound_both_use_the_whole_range() {
        // Without this the dB window maps ordinary music into the middle and
        // the pad glows at half brightness whatever is playing.
        let decay = survives(QUANTUM_MS, GAIN_HALF_LIFE_MS);
        for loudness in [0.2f32, 0.5, 0.9] {
            let heard = level(loudness);
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
        assert!(
            (ratio - 4.0).abs() < 0.2,
            "balance became {ratio}:1, not 4:1"
        );
    }

    #[test]
    fn only_the_newest_window_is_ever_analysed() {
        // Desired state, not a queue: whatever arrives is appended and anything
        // that no longer fits falls off the front. Working through a backlog in
        // order would put the pad behind the sound with no way to catch up.
        let mut buffer: Vec<f32> = (0..FFT_SIZE * 3).map(|i| i as f32).collect();
        let stale = buffer.len() - FFT_SIZE;
        buffer.drain(..stale);
        assert_eq!(buffer.len(), FFT_SIZE);
        // What is left ends at the newest sample, not the oldest.
        assert_eq!(*buffer.last().unwrap(), (FFT_SIZE * 3 - 1) as f32);
    }

    #[test]
    fn frequency_runs_up_the_pad_and_stereo_runs_across_it() {
        let cells = [[200u8; 3]; LED_COUNT];

        // Hard left, all bands: bright on the left column, dark on the right.
        let left_only = Levels {
            left: [1.0; BANDS],
            right: [0.0; BANDS],
        };
        let lit = modulate(&cells, left_only);
        assert_eq!(lit[GRID[0][0][0] as usize], [200; 3]);
        assert_eq!(lit[GRID[GRID_W - 1][0][0] as usize], [0; 3]);
        // The middle column hears both channels equally.
        assert_eq!(lit[GRID[GRID_W / 2][0][0] as usize], [100; 3]);

        // Bass only, centred: the bottom row lights and the top row does not.
        let mut bass = [0.0; BANDS];
        bass[0] = 1.0;
        let lit = modulate(
            &cells,
            Levels {
                left: bass,
                right: bass,
            },
        );
        for column in GRID {
            assert_eq!(
                lit[column[0][0] as usize], [200; 3],
                "bottom row is the low band"
            );
            assert_eq!(
                lit[column[BANDS - 1][0] as usize],
                [0; 3],
                "top row is the high band"
            );
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
    fn the_sides_meter_one_channel_each() {
        let cells = [[200u8; 3]; LED_COUNT];
        let lit = modulate(
            &cells,
            Levels {
                left: [1.0; BANDS],
                right: [0.0; BANDS],
            },
        );
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
        let lit = modulate(
            &cells,
            Levels {
                left: bass,
                right: bass,
            },
        );
        for (led, row) in UNDERGLOW_LEFT {
            let wanted = if row == 0 { [200; 3] } else { [0; 3] };
            assert_eq!(lit[led as usize], wanted, "LED {led} sits on row {row}");
        }
    }

    #[test]
    fn silence_darkens_the_pad_and_full_level_leaves_it_alone() {
        let cells = [[123u8, 45, 67]; LED_COUNT];
        assert_eq!(modulate(&cells, level(0.0)), [[0u8; 3]; LED_COUNT]);
        assert_eq!(modulate(&cells, level(1.0)), cells);
    }
}
