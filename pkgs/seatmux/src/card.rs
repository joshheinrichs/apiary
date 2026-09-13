//! The GPU seatmux holds, and the leases it carves from it.
//!
//! seatmux never calls `drmSetMaster`: the fd arrives from logind already
//! master, the same way any compositor gets one. All this module does is read
//! the card's topology into the plain structs `lease` plans over, and turn a
//! plan into a real lease fd.

use anyhow::{Context, Result, anyhow};
use drm::Device as _;
use drm::control::{Device as ControlDevice, LeaseId, PlaneType, property};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use crate::lease::{Connector, Crtc, LeasePlan, Plane, PlaneKind, Resources};

/// A DRM device. Wraps any fd so it can hold either the card from logind or a
/// lease fd handed back by the kernel.
pub struct Card<F: AsFd>(F);

impl<F: AsFd> Card<F> {
    /// Universal planes must be enabled before enumerating, or the kernel only
    /// reports overlay planes and every CRTC looks like it has no primary.
    /// Atomic implies universal planes, but ask for both explicitly.
    pub fn new(fd: F) -> Result<Self> {
        let card = Card(fd);
        card.set_client_capability(drm::ClientCapability::UniversalPlanes, true)
            .context("enabling universal planes")?;
        card.set_client_capability(drm::ClientCapability::Atomic, true)
            .context("enabling atomic modesetting")?;
        Ok(card)
    }
}

impl<F: AsFd> AsFd for Card<F> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl<F: AsFd> drm::Device for Card<F> {}
impl<F: AsFd> ControlDevice for Card<F> {}

/// A live lease. Dropping it closes the fd; the kernel revokes the lease when
/// the last reference goes, which is what makes child restarts self-cleaning.
pub struct Lease {
    pub fd: OwnedFd,
    /// Kept for `revoke_lease`, though the usual path is simply dropping `fd`.
    #[allow(dead_code)]
    pub id: LeaseId,
}

impl<F: AsFd> Card<F> {
    /// Read the card's topology.
    ///
    /// `possible_crtcs` masks are expressed as bit positions into the card's
    /// CRTC list, so the index each CRTC occupies here is load-bearing and is
    /// carried through to planning.
    /// `probe` forces a connector re-probe rather than trusting cached state.
    /// After boot nothing may have probed yet, and an unprobed connector reads
    /// as `Unknown` — indistinguishable from disconnected.
    pub fn resources(&self, probe: bool) -> Result<Resources> {
        let handles = self.resource_handles().context("reading resource handles")?;

        let crtcs: Vec<Crtc> = handles
            .crtcs()
            .iter()
            .enumerate()
            .map(|(index, handle)| Crtc { id: (*handle).into(), index })
            .collect();

        let bit_of = |handle: &drm::control::crtc::Handle| -> Option<u32> {
            handles
                .crtcs()
                .iter()
                .position(|h| h == handle)
                .map(|i| 1u32 << i)
        };

        let mut connectors = Vec::new();
        for handle in handles.connectors() {
            let info = self
                .get_connector(*handle, probe)
                .with_context(|| format!("reading connector {handle:?}"))?;

            let mut possible = 0u32;
            for encoder_handle in info.encoders() {
                let encoder = self.get_encoder(*encoder_handle)?;
                for crtc in handles.filter_crtcs(encoder.possible_crtcs()) {
                    possible |= bit_of(&crtc).unwrap_or(0);
                }
            }

            connectors.push(Connector {
                id: (*handle).into(),
                name: format!("{}-{}", info.interface().as_str(), info.interface_id()),
                connected: info.state() == drm::control::connector::State::Connected,
                possible_crtcs: possible,
            });
        }

        let mut planes = Vec::new();
        for handle in self.plane_handles().context("reading plane handles")? {
            let info = self.get_plane(handle)?;
            let mut possible = 0u32;
            for crtc in handles.filter_crtcs(info.possible_crtcs()) {
                possible |= bit_of(&crtc).unwrap_or(0);
            }
            planes.push(Plane {
                id: handle.into(),
                possible_crtcs: possible,
                kind: self.plane_kind(handle)?,
            });
        }

        Ok(Resources { crtcs, planes, connectors })
    }

    /// A plane's primary/cursor/overlay role is a property, not a field.
    fn plane_kind(&self, handle: drm::control::plane::Handle) -> Result<PlaneKind> {
        let props = self.get_properties(handle)?;
        for (id, raw) in props.iter() {
            let info = self.get_property(*id)?;
            if info.name().to_str().ok() != Some("type") {
                continue;
            }
            let property::Value::Enum(Some(value)) = info.value_type().convert_value(*raw) else {
                continue;
            };
            return Ok(match value.value() as u32 {
                v if v == PlaneType::Primary as u32 => PlaneKind::Primary,
                v if v == PlaneType::Cursor as u32 => PlaneKind::Cursor,
                _ => PlaneKind::Overlay,
            });
        }
        Err(anyhow!("plane {handle:?} has no type property"))
    }

    /// Hand a seat its objects. The returned fd is what the child adopts via
    /// `WLR_DRM_LEASE_FD`.
    pub fn lease(&self, plan: &LeasePlan) -> Result<Lease> {
        let objects: Vec<std::num::NonZeroU32> = plan
            .objects()
            .into_iter()
            .map(|id| std::num::NonZeroU32::new(id).ok_or_else(|| anyhow!("object id 0")))
            .collect::<Result<_>>()?;

        let (id, fd) = self
            .create_lease(&objects, 0)
            .with_context(|| format!("leasing {:?} to seat '{}'", plan.objects(), plan.seat))?;
        Ok(Lease { fd, id })
    }
}
