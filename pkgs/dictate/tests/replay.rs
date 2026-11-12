//! Hears a sentence: espeak-ng says it into a WAV (the build makes it, as
//! `DICTATE_FIXTURE`), and replaying it must type its words a piece at a time,
//! and a minute of it must be typed faster than it was said.

use std::process::Command;
use std::time::Instant;

fn replay(env: &str) -> Option<(Vec<String>, f64, f64)> {
    let Ok(wav) = std::env::var(env) else {
        eprintln!("no {env}; the Nix build makes one with espeak-ng");
        return None;
    };
    let said = (std::fs::metadata(&wav).unwrap().len() - 44) as f64 / 32_000.0;
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_dictate"))
        .args(["replay", &wav])
        .output()
        .unwrap();
    let took = start.elapsed().as_secs_f64();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let tails = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    Some((tails, said, took))
}

#[test]
fn it_types_what_was_said_as_it_is_said() {
    let Some((tails, _, _)) = replay("DICTATE_FIXTURE") else {
        return;
    };
    let typed = tails.concat().to_lowercase();
    // Words espeak-ng says clearly: its robot "agents" can be heard as "agent".
    for word in ["agent", "finished", "build", "tests", "passed"] {
        assert!(typed.contains(word), "typed {typed:?}");
    }
    assert!(tails.len() >= 2, "it types as it hears, not only at the end: {tails:?}");
}

#[test]
fn a_long_dictation_keeps_up_with_the_speaker() {
    let Some((tails, said, took)) = replay("DICTATE_LONG_FIXTURE") else {
        return;
    };
    let typed = tails.concat().to_lowercase();
    assert!(typed.matches("passed").count() >= 15, "typed {typed:?}");
    assert!(took < said, "{said:.0}s of speech took {took:.1}s to type");
}
