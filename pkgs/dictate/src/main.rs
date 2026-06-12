use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

const PIPEWIRE: &str = env!("DICTATE_PIPEWIRE");
const SHERPA_ONNX: &str = env!("DICTATE_SHERPA_ONNX");
const WTYPE: &str = env!("DICTATE_WTYPE");
const MODEL: &str = env!("DICTATE_MODEL");
const TARGET: &str = env!("DICTATE_TARGET");

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("dictate")
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn start() -> Result<()> {
    let dir = state_dir();
    fs::create_dir_all(&dir)?;
    let pidfile = dir.join("pid");
    if let Ok(s) = fs::read_to_string(&pidfile) {
        if s.trim().parse().map(alive).unwrap_or(false) {
            return Ok(());
        }
    }
    let wav = dir.join("rec.wav");
    let _ = fs::remove_file(&wav);

    // Record from the filtered mic when mic-filter is up, the default
    // source otherwise.
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
    let child = cmd
        .args(["--rate=16000", "--channels=1", "--format=s16"])
        .arg(&wav)
        .spawn()
        .context("spawning pw-record")?;
    fs::write(&pidfile, child.id().to_string())?;
    Ok(())
}

fn stop() -> Result<()> {
    let dir = state_dir();
    let wav = dir.join("rec.wav");
    let pidfile = dir.join("pid");
    let Ok(s) = fs::read_to_string(&pidfile) else {
        return Ok(());
    };
    let _ = fs::remove_file(&pidfile);
    let pid: i32 = s.trim().parse().context("parsing pidfile")?;

    // SIGINT lets pw-record finalize the wav header.
    unsafe { libc::kill(pid, libc::SIGINT) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while alive(pid) && Instant::now() < deadline {
        sleep(Duration::from_millis(20));
    }

    if fs::metadata(&wav).map(|m| m.len()).unwrap_or(0) == 0 {
        return Ok(());
    }
    let out = Command::new(format!("{SHERPA_ONNX}/bin/sherpa-onnx-offline"))
        .arg(format!("--encoder={MODEL}/encoder.int8.onnx"))
        .arg(format!("--decoder={MODEL}/decoder.int8.onnx"))
        .arg(format!("--joiner={MODEL}/joiner.int8.onnx"))
        .arg(format!("--tokens={MODEL}/tokens.txt"))
        .arg("--model-type=nemo_transducer")
        .arg(&wav)
        .stderr(Stdio::null())
        .output()
        .context("running sherpa-onnx-offline")?;
    let _ = fs::remove_file(&wav);
    if !out.status.success() {
        bail!("sherpa-onnx-offline failed: {}", out.status);
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = stdout
        .lines()
        .rev()
        .find(|l| l.starts_with('{'))
        .context("no result in sherpa-onnx-offline output")?;
    let result: serde_json::Value = serde_json::from_str(json)?;
    let text = result["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Ok(());
    }

    let status = Command::new(format!("{WTYPE}/bin/wtype"))
        .arg(text)
        .status()
        .context("running wtype")?;
    if !status.success() {
        bail!("wtype failed: {status}");
    }
    Ok(())
}

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("start") => start(),
        Some("stop") => stop(),
        _ => bail!("usage: dictate <start|stop>"),
    }
}
