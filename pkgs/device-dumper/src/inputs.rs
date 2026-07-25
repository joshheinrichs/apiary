use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub struct InputDevice {
    pub identifier: String,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub bus: String,
    pub uniq: Option<String>,
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_hex(path: &Path) -> Option<u16> {
    u16::from_str_radix(&read_trimmed(path)?, 16).ok()
}

// USB and Bluetooth only: the attachable peripherals (mice, keyboards,
// controllers). Everything else on the input bus is platform noise — ACPI
// buttons, ALSA jack sensing, PS/2.
fn bus_name(bus: u16) -> Option<String> {
    match bus {
        0x03 => Some("usb".to_string()),
        0x05 => Some("bluetooth".to_string()),
        _ => None,
    }
}

// Identifier exactly as sway's input_device_get_identifier computes it, so it
// can key `seat <name> attach <identifier>` config: "vendor:product:name" with
// spaces and non-printable bytes in the trimmed name replaced by underscores.
fn sway_identifier(vendor: u16, product: u16, name: &str) -> String {
    let sanitized: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_graphic() { c } else { '_' })
        .collect();
    format!("{vendor}:{product}:{sanitized}")
}

pub fn list() -> Vec<InputDevice> {
    let Ok(entries) = fs::read_dir("/sys/class/input") else {
        return Vec::new();
    };

    let mut inputs = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        let device = entry.path().join("device");

        // Physical devices only: virtual nodes (e.g. the uinput pads Steam
        // creates) have an empty phys.
        if read_trimmed(&device.join("phys"))
            .unwrap_or_default()
            .is_empty()
        {
            continue;
        }

        let Some(name) = read_trimmed(&device.join("name")) else {
            continue;
        };
        let (Some(vendor), Some(product), Some(bus)) = (
            read_hex(&device.join("id/vendor")),
            read_hex(&device.join("id/product")),
            read_hex(&device.join("id/bustype")).and_then(bus_name),
        ) else {
            continue;
        };

        inputs.push(InputDevice {
            identifier: sway_identifier(vendor, product, &name),
            name,
            vendor,
            product,
            bus,
            uniq: read_trimmed(&device.join("uniq")).filter(|s| !s.is_empty()),
        });
    }

    inputs
}
