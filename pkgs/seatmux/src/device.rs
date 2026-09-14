//! The identity a seat policy names a device by.
//!
//! `/dev/input/eventN` numbering shuffles across reboots, so policy matches on
//! properties udev derives from the hardware. Several are offered because they
//! fail differently: `vendor:product` survives being replugged into another
//! port, which `ID_PATH` does not, while `ID_PATH` distinguishes two identical
//! devices, which `vendor:product` cannot.
//!
//! Beware `vendor:product` for anything behind a Logitech Unifying receiver:
//! udev reports the *receiver's* USB identity, so every device paired to one
//! looks identical. The evdev name is what actually tells them apart.

use anyhow::Result;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// Lowercase hex, e.g. `046d:404d`. Matches mainframe-devices' vendor/product.
    pub vendor_product: Option<String>,
    /// Physical port, e.g. `pci-0000:77:00.0-usb-0:1.3:1.2`.
    pub id_path: Option<String>,
    /// Human name, e.g. `Logitech K400 Plus`.
    pub name: Option<String>,
}

impl Identity {
    /// True when `key` names this device by any of its identities.
    pub fn matches(&self, key: &str) -> bool {
        [&self.vendor_product, &self.id_path, &self.name]
            .into_iter()
            .flatten()
            .any(|value| value == key)
    }

    /// Whether udev told us anything at all. A device with no identity cannot be
    /// named, so an include list can never grant it.
    pub fn is_empty(&self) -> bool {
        self.vendor_product.is_none() && self.id_path.is_none() && self.name.is_none()
    }
}

pub fn identify(node: &Path) -> Result<Identity> {
    let devnum = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(node)?.rdev()
    };
    // Both DRM and input nodes are character devices.
    let Ok(device) = udev::Device::from_devnum(udev::DeviceType::Character, devnum) else {
        return Ok(Identity::default());
    };

    let property = |key: &str| {
        device
            .property_value(key)
            .and_then(|v| v.to_str())
            .map(str::to_owned)
    };

    // udev only tags the USB parent with vendor/model ids, so walk up if the
    // event node itself carries none.
    let vendor = property("ID_VENDOR_ID").or_else(|| walk_up(&device, "ID_VENDOR_ID"));
    let model = property("ID_MODEL_ID").or_else(|| walk_up(&device, "ID_MODEL_ID"));

    Ok(Identity {
        vendor_product: vendor.zip(model).map(|(v, m)| format!("{v}:{m}")),
        id_path: property("ID_PATH").or_else(|| walk_up(&device, "ID_PATH")),
        name: evdev_name(device.syspath()),
    })
}

fn walk_up(device: &udev::Device, key: &str) -> Option<String> {
    let mut current = device.parent();
    while let Some(parent) = current {
        if let Some(value) = parent.property_value(key).and_then(|v| v.to_str()) {
            return Some(value.to_owned());
        }
        current = parent.parent();
    }
    None
}

/// The evdev device name, e.g. `Logitech K400 Plus`.
///
/// This is a sysfs attribute of the parent input device, not a udev property —
/// udev only carries the USB identity, which for anything behind a Logitech
/// Unifying receiver is the *receiver*, identical across every device paired to
/// it. The evdev name is the only human-readable thing that tells them apart,
/// and it survives moving the receiver between ports.
fn evdev_name(syspath: &Path) -> Option<String> {
    let name = syspath.parent()?.join("name");
    let text = std::fs::read_to_string(name).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
