mod monitors;

use monitors::Monitor;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;

#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    monitors: Vec<Monitor>,
}

fn identity(m: &Monitor) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        m.make.as_deref().unwrap_or(""),
        m.model.as_deref().unwrap_or(""),
        m.serial.as_deref().unwrap_or("")
    )
}

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    // Append-only: seed from the prior record, then upsert what's connected now.
    let mut by_id: BTreeMap<String, Monitor> = BTreeMap::new();
    if let Ok(existing) = serde_json::from_str::<Record>(&input) {
        for m in existing.monitors {
            by_id.insert(identity(&m), m);
        }
    }
    for m in monitors::list() {
        by_id.insert(identity(&m), m);
    }

    let record = Record {
        monitors: by_id.into_values().collect(),
    };
    println!("{}", serde_json::to_string_pretty(&record).unwrap());
}
