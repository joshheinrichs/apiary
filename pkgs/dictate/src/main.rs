use std::ffi::{CStr, CString};
use std::fs;
use std::io::Read;
use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;

mod sherpa {
    #![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]
    include!(concat!(env!("OUT_DIR"), "/sherpa.rs"));
}

mod keyboard;

// Store paths are substituted in at build time by Nix (see default.nix).
const PIPEWIRE: &str = "@pipewire@";
const MODEL: &str = "@model@";
const TARGET: &str = "@target@";

// 16 kHz mono s16 — the rate Parakeet expects; pw-record resamples to it.
const SAMPLE_RATE: i32 = 16000;
// Chunk size for `replay` (the live path decodes back-to-back, no fixed tick).
const DECODE_INTERVAL_MS: u64 = 300;

// ---------------------------------------------------------------------------
// Pure core (no I/O): the typing reconciler and transcript diffing.
// ---------------------------------------------------------------------------

// A change to a region of `have`: delete `del` chars starting at char index
// `at`, inserting `ins` there.
struct Hunk {
    at: usize,
    del: usize,
    ins: String,
}

// A cursor operation the typist performs. The cursor starts at the end of the
// field each reconcile (the typist re-anchors it there before applying).
#[derive(Debug, PartialEq)]
pub enum Op {
    Left(usize),
    Right(usize),
    Back(usize),
    Type(String),
}

// Character-level diff from `have` to `want` as non-overlapping hunks — the
// separate changed spots. Revising an early word and appending at the end yield
// two small hunks, not one rewrite of everything between them.
fn diff(have: &str, want: &str) -> Vec<Hunk> {
    let h: Vec<char> = have.chars().collect();
    let w: Vec<char> = want.chars().collect();
    let (n, m) = (h.len(), w.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if h[i] == w[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && h[i] == w[j] {
            i += 1;
            j += 1;
            continue;
        }
        let at = i;
        let mut ins = String::new();
        while i < n || j < m {
            if i < n && j < m && h[i] == w[j] {
                break;
            }
            if j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j]) {
                ins.push(w[j]);
                j += 1;
            } else {
                i += 1;
            }
        }
        hunks.push(Hunk { at, del: i - at, ins });
    }
    hunks
}

// Turn hunks into cursor ops. The cursor starts at the end of the field (the
// typist re-anchors it there first). Process right-to-left so each hunk's
// position in `have` stays valid (edits to the right don't shift positions to
// the left), then move the cursor back to the end.
fn plan(hunks: &[Hunk], have_len: usize, want_len: usize) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut cursor = have_len;
    let mut order: Vec<&Hunk> = hunks.iter().collect();
    order.sort_by_key(|h| std::cmp::Reverse(h.at));
    for h in order {
        let target = h.at + h.del;
        if cursor > target {
            ops.push(Op::Left(cursor - target));
        }
        cursor = target;
        if h.del > 0 {
            ops.push(Op::Back(h.del));
            cursor -= h.del;
        }
        let len = h.ins.chars().count();
        if len > 0 {
            ops.push(Op::Type(h.ins.clone()));
            cursor += len;
        }
    }
    if want_len > cursor {
        ops.push(Op::Right(want_len - cursor));
    }
    ops
}

// Apply ops to an in-memory field with a cursor (replay/tests).
fn apply_ops_to_string(field: &mut String, ops: &[Op]) {
    let mut chars: Vec<char> = field.chars().collect();
    let mut cur = chars.len();
    for op in ops {
        match op {
            Op::Left(n) => cur = cur.saturating_sub(*n),
            Op::Right(n) => cur = (cur + n).min(chars.len()),
            Op::Back(n) => {
                let start = cur.saturating_sub(*n);
                chars.drain(start..cur);
                cur = start;
            }
            Op::Type(t) => {
                let ins: Vec<char> = t.chars().collect();
                let len = ins.len();
                chars.splice(cur..cur, ins);
                cur += len;
            }
        }
    }
    *field = chars.into_iter().collect();
}

// Convert whole little-endian s16 frames to normalized f32 samples; return the
// samples and the leftover trailing byte (when the input has an odd length).
fn pcm_to_f32(bytes: &[u8]) -> (Vec<f32>, Option<u8>) {
    let samples = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    let leftover = (bytes.len() % 2 == 1).then(|| bytes[bytes.len() - 1]);
    (samples, leftover)
}

// Split text into comparable tokens: lowercased, punctuation stripped.
fn words_lower(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect()
        })
        .filter(|w: &String| !w.is_empty())
        .collect()
}

// Longest-common-subsequence word diff of reference `a` vs candidate `b`. Each
// entry is tagged ' ' (same), '-' (in `a` only → dropped), or '+' (in `b` only
// → inserted).
fn word_diff(a: &[String], b: &[String]) -> Vec<(char, String)> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i].clone()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(('-', a[i].clone()));
            i += 1;
        } else {
            out.push(('+', b[j].clone()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|w| ('-', w.clone())));
    out.extend(b[j..].iter().map(|w| ('+', w.clone())));
    out
}

// Reconcile the field (currently `typed`) toward `want`: the cursor ops to run
// plus the new field contents. The cursor is assumed at the end of `typed`.
fn reconcile(typed: &str, want: &str) -> Vec<Op> {
    plan(
        &diff(typed, want),
        typed.chars().count(),
        want.chars().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(have: &str, want: &str) -> String {
        let mut f = have.to_string();
        apply_ops_to_string(&mut f, &reconcile(have, want));
        f
    }

    #[test]
    fn diff_round_trips() {
        for (have, want) in [
            ("", "hello world"),
            ("hello", "hello world"),
            ("Alright, here's a test", "All right, here's a test"),
            ("the cat sat on the mat", "the dog sat on the rug"),
            ("abc", "xyz"),
            ("hello world", "hello"),
            ("one two three four", "one TWO three FOUR five"),
        ] {
            assert_eq!(round_trip(have, want), want, "{have:?} -> {want:?}");
        }
    }

    #[test]
    fn diff_touches_only_the_changed_regions() {
        // Two separate word changes → two hunks; the middle is left alone.
        assert_eq!(diff("the cat sat on the mat", "the dog sat on the rug").len(), 2);
    }

    #[test]
    fn word_diff_reports_drops_and_inserts() {
        let a = words_lower("The quick, brown fox.");
        let b = words_lower("the brown lazy fox");
        let d = word_diff(&a, &b);
        let dropped: Vec<&str> = d.iter().filter(|(t, _)| *t == '-').map(|(_, w)| w.as_str()).collect();
        let inserted: Vec<&str> = d.iter().filter(|(t, _)| *t == '+').map(|(_, w)| w.as_str()).collect();
        assert_eq!(dropped, ["quick"]);
        assert_eq!(inserted, ["lazy"]);
    }

    #[test]
    fn pcm_decodes_samples_and_keeps_the_odd_byte() {
        let (samples, leftover) = pcm_to_f32(&[0x00, 0x00, 0xFF, 0x7F, 0x42]);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], 0.0);
        assert!((samples[1] - 0.99997).abs() < 1e-4);
        assert_eq!(leftover, Some(0x42));
    }
}

// ---------------------------------------------------------------------------
// Recognizer (sherpa-onnx FFI): decode a whole clip to text. Audio in, text out.
// ---------------------------------------------------------------------------

struct Recognizer {
    rec: *const sherpa::SherpaOnnxOfflineRecognizer,
}

impl Recognizer {
    fn new() -> Result<Self> {
        let encoder = CString::new(format!("{MODEL}/encoder.int8.onnx"))?;
        let decoder = CString::new(format!("{MODEL}/decoder.int8.onnx"))?;
        let joiner = CString::new(format!("{MODEL}/joiner.int8.onnx"))?;
        let tokens = CString::new(format!("{MODEL}/tokens.txt"))?;
        let model_type = CString::new("nemo_transducer")?;
        let provider = CString::new("cpu")?;
        let decoding = CString::new("greedy_search")?;

        let rec = unsafe {
            // Zero-init is the documented way to default every unused field.
            let mut config: sherpa::SherpaOnnxOfflineRecognizerConfig =
                MaybeUninit::zeroed().assume_init();
            config.feat_config.sample_rate = SAMPLE_RATE;
            config.feat_config.feature_dim = 80;
            config.model_config.transducer.encoder = encoder.as_ptr();
            config.model_config.transducer.decoder = decoder.as_ptr();
            config.model_config.transducer.joiner = joiner.as_ptr();
            config.model_config.tokens = tokens.as_ptr();
            config.model_config.model_type = model_type.as_ptr();
            config.model_config.num_threads = 2;
            config.model_config.provider = provider.as_ptr();
            config.decoding_method = decoding.as_ptr();
            sherpa::SherpaOnnxCreateOfflineRecognizer(&config)
        };
        if rec.is_null() {
            bail!("failed to create sherpa-onnx offline recognizer");
        }
        Ok(Recognizer { rec })
    }

    // Decode `samples` into text (with the model's own punctuation/casing).
    fn decode(&self, samples: &[f32]) -> String {
        unsafe {
            let stream = sherpa::SherpaOnnxCreateOfflineStream(self.rec);
            sherpa::SherpaOnnxAcceptWaveformOffline(
                stream,
                SAMPLE_RATE,
                samples.as_ptr(),
                samples.len() as i32,
            );
            sherpa::SherpaOnnxDecodeOfflineStream(self.rec, stream);
            let result = sherpa::SherpaOnnxGetOfflineStreamResult(stream);
            let text = if result.is_null() || (*result).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*result).text).to_string_lossy().into_owned()
            };
            sherpa::SherpaOnnxDestroyOfflineRecognizerResult(result);
            sherpa::SherpaOnnxDestroyOfflineStream(stream);
            text
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOfflineRecognizer(self.rec) };
    }
}

// ---------------------------------------------------------------------------
// I/O edges: process lifecycle, capture, decode loop, typist.
// ---------------------------------------------------------------------------

// Per-user runtime dir for our pidfile (XDG_RUNTIME_DIR/dictate), created on
// demand. `place_runtime_file` ensures the dir exists; `find_runtime_file`
// returns the path only if the file is present.
fn runtime_dir() -> Result<xdg::BaseDirectories> {
    xdg::BaseDirectories::with_prefix("dictate").context("locating XDG runtime dir")
}

fn alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

// Common pw-record invocation: 16 kHz mono s16, from the filtered mic when
// mic-filter is up, the default source otherwise. Callers add the output
// (a WAV path for `record`, or `--raw -` to stream raw PCM to stdout).
fn base_recorder() -> Command {
    let have_target = Command::new(format!("{PIPEWIRE}/bin/pw-cli"))
        .args(["info", TARGET])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let mut cmd = Command::new(format!("{PIPEWIRE}/bin/pw-record"));
    if have_target {
        cmd.args(["--target", TARGET]);
    }
    cmd.args(["--rate=16000", "--channels=1", "--format=s16"]);
    cmd
}

// `dictate start` (key press) launches a detached worker and returns at once so
// it doesn't block the sway keybinding; `dictate stop` (release) signals it.
fn start() -> Result<()> {
    let pidfile = runtime_dir()?.place_runtime_file("pid")?;
    if let Ok(s) = fs::read_to_string(&pidfile) {
        if s.trim().parse().map(alive).unwrap_or(false) {
            return Ok(());
        }
    }
    let exe = std::env::current_exe()?;
    let child = Command::new(exe)
        .arg("__run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Detach into its own session so the release keybinding can't deliver
        // terminal signals to it.
        .process_group(0)
        .spawn()
        .context("spawning dictate worker")?;
    fs::write(&pidfile, child.id().to_string())?;
    Ok(())
}

fn stop() -> Result<()> {
    let Some(pidfile) = runtime_dir()?.find_runtime_file("pid") else {
        return Ok(());
    };
    let Ok(s) = fs::read_to_string(&pidfile) else {
        return Ok(());
    };
    let _ = fs::remove_file(&pidfile);
    let pid: i32 = s.trim().parse().context("parsing pidfile")?;
    let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
    Ok(())
}

// Capture thread: read raw s16 PCM from pw-record's stdout and append samples
// to the shared clip until the stream ends (pw-record exits on SIGINT). Carries
// any odd trailing byte across reads.
fn capture(mut stdout: ChildStdout, clip: Arc<Mutex<Vec<f32>>>) {
    let mut carry: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let (samples, leftover) = pcm_to_f32(&carry);
        carry = leftover.into_iter().collect();
        if !samples.is_empty() {
            clip.lock().unwrap().extend_from_slice(&samples);
        }
    }
}

// Typist thread: own the keyboard and the model of the field. Receive desired
// text, coalesce to the latest, re-anchor the cursor to the end, and apply the
// minimal multi-region edit. Re-anchoring each time means a stray/dropped key
// can never compound across reconciles.
fn typist(rx: Receiver<String>) -> Result<()> {
    let mut kbd = keyboard::Keyboard::new()?;
    let mut typed = String::new();
    while let Ok(mut want) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            want = newer; // only the latest decode matters
        }
        if want == typed {
            continue;
        }
        kbd.to_end()?;
        kbd.apply(&reconcile(&typed, &want))?;
        typed = want;
    }
    Ok(())
}

// The worker: capture the mic, re-decode the whole clip back-to-back, send the
// desired text to the typist. Three threads passing events; the decode time is
// the natural pace.
fn run() -> Result<()> {
    // `stop` (release) signals SIGTERM; flip the flag and let the loop drain.
    let stop = Arc::new(AtomicBool::new(false));
    flag::register(SIGTERM, stop.clone())?;
    flag::register(SIGINT, stop.clone())?;

    let mut recorder = base_recorder()
        .arg("--raw")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning pw-record")?;
    let stdout = recorder.stdout.take().context("pw-record has no stdout")?;

    let clip = Arc::new(Mutex::new(Vec::<f32>::new()));
    let cap_clip = clip.clone();
    let capture = thread::spawn(move || capture(stdout, cap_clip));

    let (tx, rx) = mpsc::channel::<String>();
    let typist = thread::spawn(move || typist(rx));

    // Load the model once. Recording is already buffering into `clip`, so any
    // speech during this load is decoded as soon as we catch up.
    let recognizer = Recognizer::new()?;

    // Decode whenever new audio has arrived; send the desired text on change.
    let mut last_len = 0;
    let mut last_sent = String::new();
    while !stop.load(Ordering::SeqCst) {
        let samples = clip.lock().unwrap().clone();
        if samples.len() == last_len {
            sleep(Duration::from_millis(15));
            continue;
        }
        last_len = samples.len();
        let want = recognizer.decode(&samples).trim().to_string();
        // An empty decode means silence/garble — never reconcile to it.
        if !want.is_empty() && want != last_sent {
            last_sent = want.clone();
            let _ = tx.send(want);
        }
    }

    // Released: stop the recorder, let capture drain the tail, decode once more
    // over the whole clip so the settled text is exactly the full-file decode.
    let _ = kill(Pid::from_raw(recorder.id() as i32), Signal::SIGINT);
    let _ = capture.join();
    let samples = clip.lock().unwrap().clone();
    let want = recognizer.decode(&samples).trim().to_string();
    if !want.is_empty() && want != last_sent {
        let _ = tx.send(want);
    }
    drop(tx);
    let _ = typist.join();
    let _ = recorder.wait();
    if let Ok(xdg) = runtime_dir() {
        if let Some(pidfile) = xdg.find_runtime_file("pid") {
            let _ = fs::remove_file(pidfile);
        }
    }
    Ok(())
}

// `dictate record <wav>`: capture a test clip from the same source the worker
// uses. Runs until Ctrl-C, which lets pw-record finalize the WAV.
fn record(path: &str) -> Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            bail!("directory does not exist: {}", parent.display());
        }
    }
    eprintln!("recording to {path} — speak, then Ctrl-C to stop");
    // Inherit stdio so pw-record's own errors are visible; Ctrl-C is the normal
    // stop, so its signal exit isn't treated as a failure.
    base_recorder()
        .arg(path)
        .status()
        .context("running pw-record")?;
    Ok(())
}

// Read a 16 kHz mono s16 WAV (as produced by `record`) into f32 samples.
fn read_wav_f32(path: &str) -> Result<Vec<f32>> {
    let bytes = fs::read(path).with_context(|| format!("reading {path}"))?;
    let pcm = bytes.get(44..).unwrap_or(&[]);
    Ok(pcm_to_f32(pcm).0)
}

// `dictate replay <wav>`: stream a recorded WAV through the exact reconcile path
// (no mic, no keyboard) in chunks, then word-diff the result against a plain
// full-file decode — dropped = words we lost, inserted = filler we hallucinated.
fn replay(path: &str) -> Result<()> {
    let samples = read_wav_f32(path)?;
    let recognizer = Recognizer::new()?;

    let mut typed = String::new();
    let mut field = String::new();
    let mut clip: Vec<f32> = Vec::new();
    let chunk = (SAMPLE_RATE as usize * DECODE_INTERVAL_MS as usize / 1000).max(1);
    for window in samples.chunks(chunk) {
        clip.extend_from_slice(window);
        let want = recognizer.decode(&clip).trim().to_string();
        if want.is_empty() {
            continue;
        }
        apply_ops_to_string(&mut field, &reconcile(&typed, &want));
        typed = want;
    }

    let full = recognizer.decode(&samples);
    let wd = word_diff(&words_lower(&full), &words_lower(&field));
    let dropped: Vec<&str> = wd.iter().filter(|(t, _)| *t == '-').map(|(_, w)| w.as_str()).collect();
    let inserted: Vec<&str> = wd.iter().filter(|(t, _)| *t == '+').map(|(_, w)| w.as_str()).collect();

    eprintln!("streaming : {}", field.trim());
    eprintln!("full-file : {}", full.trim());
    eprintln!("dropped   ({}): {dropped:?}", dropped.len());
    eprintln!("inserted  ({}): {inserted:?}", inserted.len());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("start") => start(),
        Some("stop") => stop(),
        Some("__run") => run(),
        Some("record") => record(args.get(2).context("usage: dictate record <wav>")?),
        Some("replay") => replay(args.get(2).context("usage: dictate replay <wav>")?),
        Some("type") => {
            let mut kb = keyboard::Keyboard::new()?;
            kb.type_text(args.get(2).map(String::as_str).unwrap_or("dictate keyboard test"))?;
            Ok(())
        }
        _ => bail!("usage: dictate <start|stop|record <wav>|replay <wav>>"),
    }
}
