use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

#[derive(Serialize, Deserialize)]
pub struct AudioDevice {
    pub serial: String,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub device_name: String,
    pub bus: Option<String>,
    pub profiles: Vec<Profile>,
}

#[derive(Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub description: String,
}

pub fn list() -> Vec<AudioDevice> {
    let output = match Command::new(env!("PWDUMP")).output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    let objects: Vec<Value> = match serde_json::from_slice(&output) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut devices = Vec::new();
    for obj in &objects {
        let props = &obj["info"]["props"];
        if props["media.class"].as_str() != Some("Audio/Device") {
            continue;
        }
        let Some(device_name) = props["device.name"].as_str().map(String::from) else {
            continue;
        };

        // USB serial (last token of device.serial); fall back to the unique device.name.
        let serial = props["device.serial"]
            .as_str()
            .and_then(|s| s.rsplit('_').next())
            .map(String::from)
            .unwrap_or_else(|| device_name.clone());

        let profiles = obj["info"]["params"]["EnumProfile"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        Some(Profile {
                            name: p["name"].as_str()?.to_string(),
                            description: p["description"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        devices.push(AudioDevice {
            serial,
            vendor: props["device.vendor.name"].as_str().map(String::from),
            product: props["device.product.name"].as_str().map(String::from),
            device_name,
            bus: props["device.bus"].as_str().map(String::from),
            profiles,
        });
    }

    devices
}
