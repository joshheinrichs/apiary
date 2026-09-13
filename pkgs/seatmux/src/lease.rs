//! Carving `card1` into one lease per seat.
//!
//! A DRM lease is a set of object IDs: the connectors a seat drives, a CRTC for
//! each, and planes belonging to those CRTCs. Choosing them is pure computation
//! over enumerated resources, so it lives here and is tested against this
//! machine's real topology. The ioctls themselves are the caller's job — this
//! module decides *what* to lease, not *when*.

use anyhow::{Result, bail};
use std::collections::HashSet;

use crate::config::Seat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneKind {
    Primary,
    Cursor,
    Overlay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crtc {
    pub id: u32,
    /// Position in the card's CRTC list. `possible_crtcs` masks index by this,
    /// not by `id`.
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub id: u32,
    pub possible_crtcs: u32,
    pub kind: PlaneKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    pub id: u32,
    pub name: String,
    pub connected: bool,
    /// Union of the `possible_crtcs` of this connector's encoders.
    pub possible_crtcs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    pub crtcs: Vec<Crtc>,
    pub planes: Vec<Plane>,
    pub connectors: Vec<Connector>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasePlan {
    pub seat: String,
    pub connectors: Vec<u32>,
    pub crtcs: Vec<u32>,
    pub planes: Vec<u32>,
}

impl LeasePlan {
    /// Flat object list in the form `drmModeCreateLease` wants.
    pub fn objects(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.connectors.len() + self.crtcs.len() + self.planes.len());
        out.extend_from_slice(&self.connectors);
        out.extend_from_slice(&self.crtcs);
        out.extend_from_slice(&self.planes);
        out
    }
}

/// Which seats have all their connectors physically present.
///
/// Leases are built lazily: a seat whose display is switched off simply has no
/// compositor yet, and starts when the connector appears.
pub fn ready_seats<'a>(resources: &Resources, seats: &'a [Seat]) -> Vec<&'a Seat> {
    seats
        .iter()
        .filter(|seat| {
            seat.connectors.iter().all(|name| {
                resources
                    .connectors
                    .iter()
                    .any(|c| &c.name == name && c.connected)
            })
        })
        .collect()
}

/// Assign CRTCs and planes to every named seat, or explain why it cannot.
///
/// Connectors are served most-constrained-first: a connector that only one CRTC
/// can drive is placed before one that any CRTC can, so a greedy walk does not
/// strand it.
pub fn plan_leases(resources: &Resources, seats: &[&Seat]) -> Result<Vec<LeasePlan>> {
    let mut wanted: Vec<(&str, &Connector)> = Vec::new();
    for seat in seats {
        for name in &seat.connectors {
            let connector = resources
                .connectors
                .iter()
                .find(|c| &c.name == name)
                .ok_or_else(|| anyhow::anyhow!("seat '{}': no such connector '{name}'", seat.name))?;
            wanted.push((seat.name.as_str(), connector));
        }
    }

    wanted.sort_by_key(|(_, c)| c.possible_crtcs.count_ones());

    let mut taken: HashSet<u32> = HashSet::new();
    let mut assigned: Vec<(&str, &Connector, &Crtc)> = Vec::new();
    for (seat_name, connector) in wanted {
        let crtc = resources
            .crtcs
            .iter()
            .find(|c| !taken.contains(&c.id) && connector.possible_crtcs & (1 << c.index) != 0)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no free CRTC can drive connector '{}' for seat '{seat_name}'",
                    connector.name
                )
            })?;
        taken.insert(crtc.id);
        assigned.push((seat_name, connector, crtc));
    }

    let mut plans: Vec<LeasePlan> = Vec::new();
    for seat in seats {
        let mut plan = LeasePlan {
            seat: seat.name.clone(),
            connectors: Vec::new(),
            crtcs: Vec::new(),
            planes: Vec::new(),
        };
        for (seat_name, connector, crtc) in &assigned {
            if *seat_name != seat.name {
                continue;
            }
            plan.connectors.push(connector.id);
            plan.crtcs.push(crtc.id);
            plan.planes.extend(planes_for(resources, crtc)?);
        }
        plans.push(plan);
    }
    Ok(plans)
}

/// Every CRTC needs a primary plane to scan out at all. A cursor plane is taken
/// when one exists, so the seat gets a hardware cursor; overlays are left behind
/// for whoever wants them.
fn planes_for(resources: &Resources, crtc: &Crtc) -> Result<Vec<u32>> {
    let usable = |p: &&Plane| p.possible_crtcs & (1 << crtc.index) != 0;

    let primary = resources
        .planes
        .iter()
        .filter(usable)
        .find(|p| p.kind == PlaneKind::Primary)
        .map(|p| p.id);

    let Some(primary) = primary else {
        bail!("CRTC {} has no primary plane", crtc.id);
    };

    let mut out = vec![primary];
    if let Some(cursor) = resources
        .planes
        .iter()
        .filter(usable)
        .find(|p| p.kind == PlaneKind::Cursor)
    {
        out.push(cursor.id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// This machine: card1, four CRTCs, the connector IDs read off the hardware.
    fn card1() -> Resources {
        let crtcs: Vec<Crtc> = [432u32, 437, 442, 447]
            .into_iter()
            .enumerate()
            .map(|(index, id)| Crtc { id, index })
            .collect();

        // Any CRTC can drive any of these connectors on amdgpu.
        let all = 0b1111;
        let connectors = vec![
            Connector { id: 449, name: "DP-1".into(), connected: true, possible_crtcs: all },
            Connector { id: 459, name: "DP-2".into(), connected: true, possible_crtcs: all },
            Connector { id: 466, name: "HDMI-A-1".into(), connected: false, possible_crtcs: all },
            Connector { id: 473, name: "HDMI-A-2".into(), connected: true, possible_crtcs: all },
        ];

        // One primary and one cursor per CRTC, plus spare overlays.
        let mut planes = Vec::new();
        for index in 0..4u32 {
            planes.push(Plane { id: 100 + index, possible_crtcs: 1 << index, kind: PlaneKind::Primary });
            planes.push(Plane { id: 200 + index, possible_crtcs: 1 << index, kind: PlaneKind::Cursor });
        }
        for index in 0..3u32 {
            planes.push(Plane { id: 300 + index, possible_crtcs: 1 << index, kind: PlaneKind::Overlay });
        }

        Resources { crtcs, planes, connectors }
    }

    fn seats() -> Config {
        Config::parse(
            r#"
[[seat]]
name       = "desk"
connectors = ["DP-1", "DP-2"]
exclude    = ["k400"]
command    = ["sway"]

[[seat]]
name       = "tv"
connectors = ["HDMI-A-2"]
include    = ["k400"]
command    = ["sway"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn plans_both_seats_on_this_machine() {
        let resources = card1();
        let config = seats();
        let ready = ready_seats(&resources, &config.seats);
        assert_eq!(ready.len(), 2);

        let plans = plan_leases(&resources, &ready).unwrap();
        let desk = plans.iter().find(|p| p.seat == "desk").unwrap();
        let tv = plans.iter().find(|p| p.seat == "tv").unwrap();

        assert_eq!(desk.connectors, [449, 459]);
        assert_eq!(tv.connectors, [473]);

        // Two CRTCs for the desk, one for the TV, one left over.
        assert_eq!(desk.crtcs.len(), 2);
        assert_eq!(tv.crtcs.len(), 1);

        // Primary plus cursor for each CRTC.
        assert_eq!(desk.planes.len(), 4);
        assert_eq!(tv.planes.len(), 2);
    }

    /// No object may appear in two leases — the kernel would refuse, and a seat
    /// would be silently sharing scanout hardware.
    #[test]
    fn leases_are_disjoint() {
        let resources = card1();
        let config = seats();
        let ready = ready_seats(&resources, &config.seats);
        let plans = plan_leases(&resources, &ready).unwrap();

        let mut seen = HashSet::new();
        for plan in &plans {
            for object in plan.objects() {
                assert!(seen.insert(object), "object {object} leased twice");
            }
        }
    }

    /// The TV is off more often than on. Its seat must simply not be ready,
    /// rather than failing the whole plan.
    #[test]
    fn a_dark_connector_makes_only_its_own_seat_unready() {
        let mut resources = card1();
        resources.connectors.iter_mut().find(|c| c.name == "HDMI-A-2").unwrap().connected = false;

        let config = seats();
        let ready = ready_seats(&resources, &config.seats);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].name, "desk");

        assert!(plan_leases(&resources, &ready).is_ok());
    }

    /// A connector only one CRTC can drive must be placed before an
    /// unconstrained one, or a greedy walk strands it.
    #[test]
    fn constrained_connectors_are_placed_first() {
        let mut resources = card1();
        resources.crtcs.truncate(2);
        resources.connectors.iter_mut().find(|c| c.name == "DP-1").unwrap().possible_crtcs = 0b11;
        // HDMI-A-2 can only use CRTC index 0, which a naive walk gives to DP-1.
        resources.connectors.iter_mut().find(|c| c.name == "HDMI-A-2").unwrap().possible_crtcs = 0b01;

        let config = Config::parse(
            r#"
[[seat]]
name = "desk"
connectors = ["DP-1"]
exclude = ["k400"]
command = ["sway"]

[[seat]]
name = "tv"
connectors = ["HDMI-A-2"]
include = ["k400"]
command = ["sway"]
"#,
        )
        .unwrap();

        let ready = ready_seats(&resources, &config.seats);
        let plans = plan_leases(&resources, &ready).unwrap();
        let tv = plans.iter().find(|p| p.seat == "tv").unwrap();
        let desk = plans.iter().find(|p| p.seat == "desk").unwrap();
        assert_eq!(tv.crtcs, [432], "constrained connector must get the only CRTC it can use");
        assert_eq!(desk.crtcs, [437]);
    }

    #[test]
    fn reports_exhausted_crtcs_and_unknown_connectors() {
        let mut resources = card1();
        resources.crtcs.truncate(1);
        let config = seats();
        let ready = ready_seats(&resources, &config.seats);
        let err = plan_leases(&resources, &ready).unwrap_err().to_string();
        assert!(err.contains("no free CRTC"), "{err}");

        let resources = card1();
        let config = Config::parse(
            r#"
[[seat]]
name = "desk"
connectors = ["DP-9"]
exclude = ["k400"]
command = ["sway"]
"#,
        )
        .unwrap();
        let refs: Vec<&Seat> = config.seats.iter().collect();
        let err = plan_leases(&resources, &refs).unwrap_err().to_string();
        assert!(err.contains("DP-9"), "{err}");
    }

    #[test]
    fn a_crtc_without_a_primary_plane_is_an_error() {
        let mut resources = card1();
        resources.planes.retain(|p| p.kind != PlaneKind::Primary);
        let config = seats();
        let ready = ready_seats(&resources, &config.seats);
        let err = plan_leases(&resources, &ready).unwrap_err().to_string();
        assert!(err.contains("no primary plane"), "{err}");
    }
}
