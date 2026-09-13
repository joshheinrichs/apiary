//! Seat configuration.
//!
//! Parsed once into typed structs with every ambiguity already rejected, so the
//! runtime never has to ask whether a device belongs to two seats or a connector
//! was claimed twice. A bad config fails at load, not at spawn.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;

use crate::device::Identity;
use crate::proto::MAX_SEAT_LEN;

/// Which input devices a seat may open, matched against the identities udev
/// reports — `vendor:product`, `ID_PATH`, or the device name.
///
/// The asymmetry is deliberate: the desk excludes, so new hardware at the desk
/// needs no config change, while the TV includes, so nothing drifts onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePolicy {
    Include(Vec<String>),
    Exclude(Vec<String>),
}

impl DevicePolicy {
    /// A device udev tells us nothing about cannot be named, so an include list
    /// can never grant it and an exclude list can never rule it out.
    pub fn allows(&self, identity: &Identity) -> bool {
        let named = |list: &[String]| list.iter().any(|key| identity.matches(key));
        match self {
            DevicePolicy::Include(list) => !identity.is_empty() && named(list),
            DevicePolicy::Exclude(list) => identity.is_empty() || !named(list),
        }
    }

    fn named(&self) -> &[String] {
        match self {
            DevicePolicy::Include(l) | DevicePolicy::Exclude(l) => l,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    pub name: String,
    pub connectors: Vec<String>,
    pub devices: DevicePolicy,
    pub sink: Option<String>,
    pub source: Option<String>,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub seats: Vec<Seat>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    seat: Vec<RawSeat>,
}

#[derive(Debug, Deserialize)]
struct RawSeat {
    name: String,
    #[serde(default)]
    connectors: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    sink: Option<String>,
    source: Option<String>,
    #[serde(default)]
    command: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text).context("parsing seatmux config")?;
        if raw.seat.is_empty() {
            bail!("no seats configured");
        }

        let seats: Vec<Seat> = raw
            .seat
            .into_iter()
            .map(Seat::from_raw)
            .collect::<Result<_>>()?;

        reject_duplicates(seats.iter().map(|s| s.name.as_str()), "seat name")?;
        reject_duplicates(
            seats.iter().flat_map(|s| s.connectors.iter().map(String::as_str)),
            "connector",
        )?;
        // Two seats both *including* a device would race to open it. Exclude
        // lists may legitimately repeat, since they name what a seat gives up.
        reject_duplicates(
            seats
                .iter()
                .filter(|s| matches!(s.devices, DevicePolicy::Include(_)))
                .flat_map(|s| s.devices.named().iter().map(String::as_str)),
            "device",
        )?;

        Ok(Config { seats })
    }
}

impl Seat {
    fn from_raw(raw: RawSeat) -> Result<Seat> {
        if raw.name.is_empty() {
            bail!("seat has an empty name");
        }
        // libseat carries the seat name over the wire in a fixed-size field.
        if raw.name.len() + 1 > MAX_SEAT_LEN {
            bail!("seat name '{}' exceeds {} bytes", raw.name, MAX_SEAT_LEN - 1);
        }
        if raw.connectors.is_empty() {
            bail!("seat '{}' has no connectors", raw.name);
        }
        if raw.command.is_empty() {
            bail!("seat '{}' has no command", raw.name);
        }

        let devices = match (raw.include.is_empty(), raw.exclude.is_empty()) {
            (false, true) => DevicePolicy::Include(raw.include),
            (true, false) => DevicePolicy::Exclude(raw.exclude),
            (true, true) => bail!(
                "seat '{}' sets neither include nor exclude; say which devices it owns",
                raw.name
            ),
            (false, false) => bail!(
                "seat '{}' sets both include and exclude; pick one",
                raw.name
            ),
        };

        Ok(Seat {
            name: raw.name,
            connectors: raw.connectors,
            devices,
            sink: raw.sink,
            source: raw.source,
            command: raw.command,
        })
    }
}

fn reject_duplicates<'a>(items: impl Iterator<Item = &'a str>, what: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for item in items {
        if !seen.insert(item) {
            bail!("{what} '{item}' is claimed by more than one seat");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESK_AND_TV: &str = r#"
[[seat]]
name       = "desk"
connectors = ["DP-1", "DP-2"]
exclude    = ["046d:404d"]
sink       = "komplete"
source     = "mic-filter"
command    = ["sway"]

[[seat]]
name       = "tv"
connectors = ["HDMI-A-2"]
include    = ["046d:404d"]
sink       = "hdmi"
command    = ["sway", "-c", "/tv.conf"]
"#;

    #[test]
    fn parses_the_real_two_seat_shape() {
        let config = Config::parse(DESK_AND_TV).unwrap();
        assert_eq!(config.seats.len(), 2);

        let desk = &config.seats[0];
        assert_eq!(desk.connectors, ["DP-1", "DP-2"]);
        assert_eq!(desk.source.as_deref(), Some("mic-filter"));
        assert!(matches!(desk.devices, DevicePolicy::Exclude(_)));

        let tv = &config.seats[1];
        assert_eq!(tv.command, ["sway", "-c", "/tv.conf"]);
        // No microphone at the couch.
        assert_eq!(tv.source, None);
        assert!(matches!(tv.devices, DevicePolicy::Include(_)));
    }

    /// Real hardware: the K400 is the couch keyboard, the MX Master is the desk
    /// mouse. Both are Logitech, so vendor alone would not separate them.
    fn k400() -> Identity {
        Identity {
            vendor_product: Some("046d:404d".into()),
            id_path: Some("pci-0000:77:00.0-usb-0:1.3:1.2".into()),
            name: Some("Logitech K400 Plus".into()),
        }
    }

    fn mx_master() -> Identity {
        Identity {
            vendor_product: Some("046d:4082".into()),
            id_path: Some("pci-0000:10:00.0-usb-0:2.2:1.2".into()),
            name: Some("Logitech MX Master 3".into()),
        }
    }

    #[test]
    fn policies_route_the_k400_to_the_tv_only() {
        let config = Config::parse(DESK_AND_TV).unwrap();
        let (desk, tv) = (&config.seats[0], &config.seats[1]);

        assert!(!desk.devices.allows(&k400()), "desk must not take the K400");
        assert!(tv.devices.allows(&k400()), "tv must take the K400");
        assert!(desk.devices.allows(&mx_master()));
        assert!(!tv.devices.allows(&mx_master()), "tv takes only what it lists");
    }

    /// Any of the identities udev reports may name a device, so a config written
    /// against ID_PATH keeps working if the key is later changed to
    /// vendor:product, and vice versa.
    #[test]
    fn any_identity_can_name_a_device() {
        for key in ["046d:404d", "pci-0000:77:00.0-usb-0:1.3:1.2", "Logitech K400 Plus"] {
            let text = DESK_AND_TV.replace("046d:404d", key);
            let config = Config::parse(&text).unwrap();
            assert!(config.seats[1].devices.allows(&k400()), "tv should match on {key}");
            assert!(!config.seats[0].devices.allows(&k400()), "desk should exclude by {key}");
        }
    }

    /// A device udev tells us nothing about cannot be named, so include can
    /// never grant it and exclude can never withhold it.
    #[test]
    fn unnamed_devices_fall_to_the_excluding_seat() {
        let config = Config::parse(DESK_AND_TV).unwrap();
        let unknown = Identity::default();
        assert!(config.seats[0].devices.allows(&unknown));
        assert!(!config.seats[1].devices.allows(&unknown));
    }

    #[test]
    fn rejects_a_connector_claimed_twice() {
        let text = DESK_AND_TV.replace(r#"connectors = ["HDMI-A-2"]"#, r#"connectors = ["DP-1"]"#);
        let err = Config::parse(&text).unwrap_err().to_string();
        assert!(err.contains("DP-1"), "{err}");
    }

    /// Two seats including the same device would race to open it; two seats
    /// excluding the same device is normal and must stay legal.
    #[test]
    fn rejects_a_device_included_twice_but_allows_it_excluded_twice() {
        let both_include = r#"
[[seat]]
name = "a"
connectors = ["DP-1"]
include = ["dev-1"]
command = ["sway"]

[[seat]]
name = "b"
connectors = ["DP-2"]
include = ["dev-1"]
command = ["sway"]
"#;
        assert!(Config::parse(both_include).unwrap_err().to_string().contains("dev-1"));

        let both_exclude = both_include.replace("include", "exclude");
        assert!(Config::parse(&both_exclude).is_ok());
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_seats() {
        let cases = [
            (r#"[[seat]]
name = "a"
connectors = ["DP-1"]
command = ["sway"]"#, "neither include nor exclude"),
            (r#"[[seat]]
name = "a"
connectors = ["DP-1"]
include = ["x"]
exclude = ["y"]
command = ["sway"]"#, "both include and exclude"),
            (r#"[[seat]]
name = "a"
include = ["x"]
command = ["sway"]"#, "no connectors"),
            (r#"[[seat]]
name = "a"
connectors = ["DP-1"]
include = ["x"]"#, "no command"),
        ];
        for (text, expected) in cases {
            let err = Config::parse(text).unwrap_err().to_string();
            assert!(err.contains(expected), "expected {expected:?}, got {err:?}");
        }
    }

    #[test]
    fn rejects_empty_and_oversized_names() {
        assert!(Config::parse("").is_err());

        let long = "x".repeat(MAX_SEAT_LEN);
        let text = format!(
            "[[seat]]\nname = \"{long}\"\nconnectors = [\"DP-1\"]\ninclude = [\"x\"]\ncommand = [\"sway\"]"
        );
        assert!(Config::parse(&text).unwrap_err().to_string().contains("exceeds"));
    }
}
