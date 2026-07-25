use nusb::MaybeFuture;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UsbDevice {
    pub identifier: String,
    pub name: Option<String>,
    pub manufacturer: Option<String>,
    pub vendor: u16,
    pub product: u16,
    pub serial: Option<String>,
}

// Raw USB peripherals: the whole-device identity (idVendor:idProduct[:serial])
// that keys udev permission rules — e.g. the GameCube controller adapter, which
// Dolphin opens directly via libusb and so never appears as an input event node
// for `inputs` to catch. Hubs (bDeviceClass 09, including the root/xHCI
// controllers) are pure infrastructure and excluded. Overlaps intentionally with
// `inputs`/`audio`: this is the USB-identity axis (for permissions), not sway
// seat identifiers or PipeWire profiles.
//
// nusb reads the descriptors from the OS without opening the device, so this
// needs no privileges and no libusb C dependency.
pub fn list() -> Vec<UsbDevice> {
    // list_devices() is a MaybeFuture (dual sync/async); .wait() takes the
    // blocking path.
    let Ok(devices) = nusb::list_devices().wait() else {
        return Vec::new();
    };

    devices
        .filter(|info| info.class() != 0x09)
        .map(|info| {
            let vendor = info.vendor_id();
            let product = info.product_id();
            let serial = info.serial_number().map(str::to_string);
            let identifier = match &serial {
                Some(s) => format!("{vendor:04x}:{product:04x}:{s}"),
                None => format!("{vendor:04x}:{product:04x}"),
            };
            UsbDevice {
                identifier,
                name: info.product_string().map(str::to_string),
                manufacturer: info.manufacturer_string().map(str::to_string),
                vendor,
                product,
                serial,
            }
        })
        .collect()
}
