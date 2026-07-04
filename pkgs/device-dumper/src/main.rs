mod audio;
mod monitors;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;

#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    monitors: Vec<monitors::Monitor>,
    #[serde(default)]
    audio: Vec<audio::AudioDevice>,
}

fn monitor_key(m: &monitors::Monitor) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        m.make.as_deref().unwrap_or(""),
        m.model.as_deref().unwrap_or(""),
        m.serial.as_deref().unwrap_or("")
    )
}

fn merge<T>(existing: Vec<T>, current: Vec<T>, key: impl Fn(&T) -> String) -> Vec<T> {
    let mut by_id: BTreeMap<String, T> = BTreeMap::new();
    for x in existing.into_iter().chain(current) {
        by_id.insert(key(&x), x);
    }
    by_id.into_values().collect()
}

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let existing: Record = serde_json::from_str(&input).unwrap_or(Record {
        monitors: Vec::new(),
        audio: Vec::new(),
    });

    let record = Record {
        monitors: merge(existing.monitors, monitors::list(), monitor_key),
        audio: merge(existing.audio, audio::list(), |a| a.serial.clone()),
    };
    println!("{}", serde_json::to_string_pretty(&record).unwrap());
}
