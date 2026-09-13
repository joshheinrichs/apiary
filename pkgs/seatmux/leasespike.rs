// Spike: can a DRM master carve a lease over a *disconnected* connector, and
// what does the lessee see? Run against an idle card so no compositor is
// disturbed. Enumerates and leases only -- never modesets.

use std::ffi::CString;
use std::os::raw::{c_char, c_int};

#[repr(C)]
struct Res {
    count_fbs: c_int,
    fbs: *mut u32,
    count_crtcs: c_int,
    crtcs: *mut u32,
    count_connectors: c_int,
    connectors: *mut u32,
    count_encoders: c_int,
    encoders: *mut u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
struct PlaneRes {
    count_planes: u32,
    planes: *mut u32,
}

#[repr(C)]
struct Connector {
    connector_id: u32,
    encoder_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    count_modes: c_int,
    modes: *mut u8,
    count_props: c_int,
    props: *mut u32,
    prop_values: *mut u64,
    count_encoders: c_int,
    encoders: *mut u32,
}

extern "C" {
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn __errno_location() -> *mut c_int;
    fn drmSetClientCap(fd: c_int, capability: u64, value: u64) -> c_int;
    fn drmSetMaster(fd: c_int) -> c_int;
    fn drmDropMaster(fd: c_int) -> c_int;
    fn drmIsMaster(fd: c_int) -> c_int;
    fn drmModeGetResources(fd: c_int) -> *mut Res;
    fn drmModeFreeResources(p: *mut Res);
    fn drmModeGetPlaneResources(fd: c_int) -> *mut PlaneRes;
    fn drmModeFreePlaneResources(p: *mut PlaneRes);
    fn drmModeGetConnectorCurrent(fd: c_int, id: u32) -> *mut Connector;
    fn drmModeFreeConnector(p: *mut Connector);
    fn drmModeCreateLease(
        fd: c_int,
        objects: *const u32,
        num_objects: c_int,
        flags: c_int,
        lessee_id: *mut u32,
    ) -> c_int;
    fn drmModeRevokeLease(fd: c_int, lessee_id: u32) -> c_int;
}

fn errno() -> i32 {
    unsafe { *__errno_location() }
}

fn conn_type(t: u32) -> &'static str {
    match t {
        1 => "VGA", 2 => "DVI-I", 3 => "DVI-D", 4 => "DVI-A",
        5 => "Composite", 6 => "SVIDEO", 7 => "LVDS", 8 => "Component",
        9 => "DIN", 10 => "DP", 11 => "HDMI-A", 12 => "HDMI-B",
        13 => "TV", 14 => "eDP", 15 => "Virtual", 16 => "DSI",
        17 => "DPI", 18 => "Writeback", 19 => "SPI", 20 => "USB",
        _ => "?",
    }
}

fn conn_state(c: u32) -> &'static str {
    match c { 1 => "connected", 2 => "disconnected", _ => "unknown" }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or("/dev/dri/card0".into());
    let cpath = CString::new(path.clone()).unwrap();

    let fd = unsafe { open(cpath.as_ptr(), 2 /*O_RDWR*/) };
    if fd < 0 {
        eprintln!("open {}: errno {}", path, errno());
        std::process::exit(1);
    }
    println!("opened {} fd={}", path, fd);
    let cap_up = unsafe { drmSetClientCap(fd, 2 /*UNIVERSAL_PLANES*/, 1) };
    let cap_at = unsafe { drmSetClientCap(fd, 3 /*ATOMIC*/, 1) };
    println!("client caps: universal_planes={} atomic={}", cap_up, cap_at);
    println!("is_master(before setmaster) = {}", unsafe { drmIsMaster(fd) });

    let sm = unsafe { drmSetMaster(fd) };
    println!("drmSetMaster = {} (errno {})", sm, if sm < 0 { errno() } else { 0 });
    let have_master = sm >= 0;
    if !have_master {
        eprintln!("NOTE: not DRM master (errno {}) -- enumerating only; lease test skipped", errno());
    }

    let res = unsafe { drmModeGetResources(fd) };
    let planes = unsafe { drmModeGetPlaneResources(fd) };
    if res.is_null() || planes.is_null() {
        eprintln!("failed to enumerate resources");
        std::process::exit(3);
    }
    let r = unsafe { &*res };
    let p = unsafe { &*planes };
    let crtcs = unsafe { std::slice::from_raw_parts(r.crtcs, r.count_crtcs as usize) };
    let conns = unsafe { std::slice::from_raw_parts(r.connectors, r.count_connectors as usize) };
    let plane_ids = unsafe { std::slice::from_raw_parts(p.planes, p.count_planes as usize) };

    println!("\nresources: {} crtcs, {} connectors, {} planes",
        crtcs.len(), conns.len(), plane_ids.len());
    println!("  crtcs:  {:?}", crtcs);
    println!("  planes: {:?}", plane_ids);

    println!("\nconnectors:");
    let mut target: Option<u32> = None;
    for &cid in conns {
        let c = unsafe { drmModeGetConnectorCurrent(fd, cid) };
        if c.is_null() { continue; }
        let cc = unsafe { &*c };
        println!("  id={:<4} {}-{:<2} {:<13} modes={}",
            cc.connector_id, conn_type(cc.connector_type), cc.connector_type_id,
            conn_state(cc.connection), cc.count_modes);
        // Deliberately prefer a DISCONNECTED connector -- that is the case we
        // need to work (TV powered off at lease time).
        if cc.connection == 2 && target.is_none() {
            target = Some(cc.connector_id);
        }
        unsafe { drmModeFreeConnector(c) };
    }

    if !have_master {
        unsafe { drmModeFreePlaneResources(planes); drmModeFreeResources(res); close(fd); }
        println!("\n(no DRM master -- lease test skipped; rerun from a bare TTY)");
        return;
    }

    let Some(conn) = target else {
        eprintln!("no disconnected connector to test with");
        std::process::exit(4);
    };
    let crtc = crtcs[0];
    let plane = plane_ids[0];

    println!("\n=== leasing DISCONNECTED connector {} + crtc {} + plane {} ===",
        conn, crtc, plane);
    let objects: [u32; 3] = [conn, crtc, plane];
    let mut lessee_id: u32 = 0;
    let lease_fd = unsafe {
        drmModeCreateLease(fd, objects.as_ptr(), 3, 0, &mut lessee_id)
    };

    if lease_fd < 0 {
        println!("RESULT: drmModeCreateLease FAILED, ret={} errno={}", lease_fd, errno());
        println!("  -> lease over a disconnected connector is NOT possible");
    } else {
        println!("RESULT: lease created. fd={} lessee_id={}", lease_fd, lessee_id);
        println!("  lessee is_master = {}", unsafe { drmIsMaster(lease_fd) });

        let lres = unsafe { drmModeGetResources(lease_fd) };
        if lres.is_null() {
            println!("  lessee drmModeGetResources -> NULL (errno {})", errno());
        } else {
            let lr = unsafe { &*lres };
            let lc = unsafe { std::slice::from_raw_parts(lr.connectors, lr.count_connectors as usize) };
            let lcr = unsafe { std::slice::from_raw_parts(lr.crtcs, lr.count_crtcs as usize) };
            println!("  lessee sees: connectors {:?} crtcs {:?}", lc, lcr);
            unsafe { drmModeFreeResources(lres) };
        }
        let lp = unsafe { drmModeGetPlaneResources(lease_fd) };
        if !lp.is_null() {
            let lpr = unsafe { &*lp };
            let lpi = unsafe { std::slice::from_raw_parts(lpr.planes, lpr.count_planes as usize) };
            println!("  lessee sees: planes {:?}", lpi);
            unsafe { drmModeFreePlaneResources(lp) };
        }

        println!("\n  revoking lease...");
        let rv = unsafe { drmModeRevokeLease(fd, lessee_id) };
        println!("  drmModeRevokeLease = {}", rv);
        println!("  lessee is_master after revoke = {}", unsafe { drmIsMaster(lease_fd) });
        unsafe { close(lease_fd) };
    }

    unsafe {
        drmModeFreePlaneResources(planes);
        drmModeFreeResources(res);
        drmDropMaster(fd);
        close(fd);
    }
    println!("\ndropped master, closed. card untouched (no modeset performed).");
}
