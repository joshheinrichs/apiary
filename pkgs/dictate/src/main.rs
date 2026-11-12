use std::ffi::{CStr, CString};
use std::fs;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::process::{ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;

mod sherpa {
    #![allow(
        non_upper_case_globals,
        non_camel_case_types,
        non_snake_case,
        dead_code
    )]
    include!(concat!(env!("OUT_DIR"), "/sherpa.rs"));
}

const PW_RECORD: &str = env!("DICTATE_PW_RECORD");
const PW_CLI: &str = env!("DICTATE_PW_CLI");
const WTYPE: &str = env!("DICTATE_WTYPE");
const MODEL: &str = env!("DICTATE_MODEL");
const TARGET: &str = env!("DICTATE_TARGET");

// 16 kHz mono s16 — the rate Nemotron expects; pw-record resamples to it.
const SAMPLE_RATE: i32 = 16000;
// Silence after the last word, so the final chunk is heard out.
const TAIL: usize = SAMPLE_RATE as usize;

// ---------------------------------------------------------------------------
// Pure core.
// ---------------------------------------------------------------------------

// What to type now that the transcript reads `text`, given `typed` is already
// in the field. A streaming transducer only ever appends, so this is the new
// tail; if it ever doesn't extend what was typed, nothing can be unsaid, so
// type nothing and carry on from the new text.
fn fresh<'a>(typed: &str, text: &'a str) -> &'a str {
    text.strip_prefix(typed).unwrap_or("")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_new_tail_is_typed() {
        assert_eq!(fresh("", "Hello"), "Hello");
        assert_eq!(fresh("Hello", "Hello world"), " world");
        assert_eq!(fresh("Hello world", "Hello world"), "");
        assert_eq!(fresh("Hello world", "Hullo world"), "", "never retypes");
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
// Recognizer (sherpa-onnx FFI): one streaming decode, audio in once, text out.
// ---------------------------------------------------------------------------

struct Recognizer(*const sherpa::SherpaOnnxOnlineRecognizer);

struct Stream<'a>(&'a Recognizer, *const sherpa::SherpaOnnxOnlineStream);

impl Recognizer {
    fn new() -> Result<Self> {
        let encoder = CString::new(format!("{MODEL}/encoder.int8.onnx"))?;
        let decoder = CString::new(format!("{MODEL}/decoder.int8.onnx"))?;
        let joiner = CString::new(format!("{MODEL}/joiner.int8.onnx"))?;
        let tokens = CString::new(format!("{MODEL}/tokens.txt"))?;
        let provider = CString::new("cpu")?;
        let decoding = CString::new("greedy_search")?;
        let rec = unsafe {
            // Zero-init is the documented way to default every unused field.
            let mut config: sherpa::SherpaOnnxOnlineRecognizerConfig =
                MaybeUninit::zeroed().assume_init();
            config.feat_config.sample_rate = SAMPLE_RATE;
            config.feat_config.feature_dim = 80;
            config.model_config.transducer.encoder = encoder.as_ptr();
            config.model_config.transducer.decoder = decoder.as_ptr();
            config.model_config.transducer.joiner = joiner.as_ptr();
            config.model_config.tokens = tokens.as_ptr();
            config.model_config.num_threads = 2;
            config.model_config.provider = provider.as_ptr();
            config.decoding_method = decoding.as_ptr();
            sherpa::SherpaOnnxCreateOnlineRecognizer(&config)
        };
        if rec.is_null() {
            bail!("failed to create sherpa-onnx online recognizer");
        }
        Ok(Recognizer(rec))
    }

    fn stream(&self) -> Stream<'_> {
        Stream(self, unsafe { sherpa::SherpaOnnxCreateOnlineStream(self.0) })
    }
}

impl Stream<'_> {
    // Take new audio and decode every chunk it completes.
    fn hear(&self, samples: &[f32]) {
        unsafe {
            sherpa::SherpaOnnxOnlineStreamAcceptWaveform(
                self.1,
                SAMPLE_RATE,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }
        self.decode();
    }

    // No more audio: a little silence to carry the last chunk through.
    fn finish(&self) {
        self.hear(&[0.0; TAIL]);
        unsafe { sherpa::SherpaOnnxOnlineStreamInputFinished(self.1) };
        self.decode();
    }

    fn decode(&self) {
        unsafe {
            while sherpa::SherpaOnnxIsOnlineStreamReady(self.0.0, self.1) != 0 {
                sherpa::SherpaOnnxDecodeOnlineStream(self.0.0, self.1);
            }
        }
    }

    // Everything heard so far, with the model's own punctuation and casing.
    fn text(&self) -> String {
        unsafe {
            let result = sherpa::SherpaOnnxGetOnlineStreamResult(self.0.0, self.1);
            let text = if result.is_null() || (*result).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*result).text)
                    .to_string_lossy()
                    .trim_start()
                    .to_string()
            };
            sherpa::SherpaOnnxDestroyOnlineRecognizerResult(result);
            text
        }
    }
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOnlineStream(self.1) };
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOnlineRecognizer(self.0) };
    }
}

// ---------------------------------------------------------------------------
// I/O edges: process lifecycle, capture, the listen loop.
// ---------------------------------------------------------------------------

// Per-user runtime dir for our pidfile (XDG_RUNTIME_DIR/dictate).
fn runtime_dir() -> Result<xdg::BaseDirectories> {
    xdg::BaseDirectories::with_prefix("dictate").context("locating XDG runtime dir")
}

fn alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

// pw-record as raw 16 kHz mono s16 on stdout, from the filtered mic when
// mic-filter is up, the default source otherwise.
fn recorder() -> Command {
    let have_target = Command::new(PW_CLI)
        .args(["info", TARGET])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let mut cmd = Command::new(PW_RECORD);
    if have_target {
        cmd.args(["--target", TARGET]);
    }
    cmd.args(["--rate=16000", "--channels=1", "--format=s16", "--raw", "-"]);
    cmd
}

// `dictate start` (key press) launches a detached worker and returns at once so
// it doesn't block the sway keybinding; `dictate stop` (release) signals it.
fn start() -> Result<()> {
    let pidfile = runtime_dir()?.place_runtime_file("pid")?;
    if let Ok(s) = fs::read_to_string(&pidfile)
        && s.trim().parse().is_ok_and(alive)
    {
        return Ok(());
    }
    let child = Command::new(std::env::current_exe()?)
        .arg("__run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own session, so the release keybinding can't signal it.
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

// Capture thread: append pw-record's samples to the clip until it exits.
fn capture(mut stdout: ChildStdout, clip: Arc<Mutex<Vec<f32>>>) {
    let mut carry: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n @ 1..) = stdout.read(&mut buf) {
        carry.extend_from_slice(&buf[..n]);
        let (samples, leftover) = pcm_to_f32(&carry);
        carry = leftover.into_iter().collect();
        clip.lock().unwrap().extend_from_slice(&samples);
    }
}

// Feed the clip to one stream as it grows, typing each new tail of the
// transcript, until `ended` is set and the clip is drained.
fn listen(
    clip: &Mutex<Vec<f32>>,
    ended: &AtomicBool,
    mut out: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let recognizer = Recognizer::new()?;
    let stream = recognizer.stream();
    let (mut fed, mut typed) = (0, String::new());
    loop {
        // Ended first: then the clip read after holds everything there will be.
        let over = ended.load(Ordering::SeqCst);
        let new: Vec<f32> = clip.lock().unwrap()[fed..].to_vec();
        if new.is_empty() && over {
            break;
        }
        if new.is_empty() {
            sleep(Duration::from_millis(20));
            continue;
        }
        fed += new.len();
        stream.hear(&new);
        let text = stream.text();
        out(fresh(&typed, &text))?;
        typed = text;
    }
    stream.finish();
    out(fresh(&typed, &stream.text()))
}

fn wtype(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    Command::new(WTYPE)
        .args(["--", text])
        .status()
        .context("running wtype")?;
    Ok(())
}

// The worker: capture the mic and type what it hears until released.
fn run() -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    flag::register(SIGTERM, stop.clone())?;
    flag::register(SIGINT, stop.clone())?;

    let mut recorder = recorder()
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning pw-record")?;
    let stdout = recorder.stdout.take().context("pw-record has no stdout")?;
    let clip = Arc::new(Mutex::new(Vec::new()));
    let ended = Arc::new(AtomicBool::new(false));
    {
        let (clip, ended, stop) = (clip.clone(), ended.clone(), stop.clone());
        let pid = Pid::from_raw(recorder.id() as i32);
        thread::spawn(move || {
            let capturing = {
                let clip = clip.clone();
                thread::spawn(move || capture(stdout, clip))
            };
            while !stop.load(Ordering::SeqCst) {
                sleep(Duration::from_millis(15));
            }
            // Released: pw-record flushes its tail on SIGINT, then capture ends.
            let _ = kill(pid, Signal::SIGINT);
            let _ = capturing.join();
            ended.store(true, Ordering::SeqCst);
        });
    }
    listen(&clip, &ended, wtype)?;
    let _ = recorder.wait();
    if let Some(pidfile) = runtime_dir().ok().and_then(|x| x.find_runtime_file("pid")) {
        let _ = fs::remove_file(pidfile);
    }
    Ok(())
}

// `dictate replay <wav>`: a 16 kHz mono s16 WAV through the same listen loop,
// at ten times speed, printing each typed tail on its own line.
fn replay(path: &str) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("reading {path}"))?;
    let samples = pcm_to_f32(bytes.get(44..).unwrap_or(&[])).0;
    let clip = Arc::new(Mutex::new(Vec::new()));
    let ended = Arc::new(AtomicBool::new(false));
    {
        let (clip, ended) = (clip.clone(), ended.clone());
        thread::spawn(move || {
            for second in samples.chunks(SAMPLE_RATE as usize) {
                clip.lock().unwrap().extend_from_slice(second);
                sleep(Duration::from_millis(100));
            }
            ended.store(true, Ordering::SeqCst);
        });
    }
    let mut stdout = std::io::stdout().lock();
    listen(&clip, &ended, |tail| {
        if !tail.is_empty() {
            writeln!(stdout, "{tail}")?;
        }
        Ok(())
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("start") => start(),
        Some("stop") => stop(),
        Some("__run") => run(),
        Some("replay") => replay(args.get(2).context("usage: dictate replay <wav>")?),
        _ => bail!("usage: dictate <start|stop|replay <wav>>"),
    }
}
