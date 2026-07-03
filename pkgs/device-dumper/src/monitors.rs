use libdisplay_info::info::Info;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Serialize, Deserialize)]
pub struct Monitor {
    pub connector: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub edid_version: String,
    pub manufacturer: String,
    pub product_code: u16,
    pub serial_number: Option<u32>,
    pub manufacture_week: i32,
    pub manufacture_year: i32,
    pub model_year: Option<i32>,
    pub gamma: Option<f32>,
    pub video_input: Option<VideoInput>,
    pub screen: Screen,
    pub detailed_timings: Vec<DetailedTiming>,
    pub range_limits: Vec<RangeLimits>,
    pub hdr: Hdr,
    pub color_primaries: ColorPrimaries,
    pub colorimetry: Colorimetry,
}

#[derive(Serialize, Deserialize)]
pub struct VideoInput {
    pub interface: String,
    pub color_bit_depth: Option<i32>,
}

#[derive(Serialize, Deserialize)]
pub struct Screen {
    pub width_cm: Option<i32>,
    pub height_cm: Option<i32>,
}

#[derive(Serialize, Deserialize)]
pub struct DetailedTiming {
    pub pixel_clock_hz: i32,
    pub horiz_video: i32,
    pub vert_video: i32,
    pub horiz_image_mm: i32,
    pub vert_image_mm: i32,
    pub interlaced: bool,
}

#[derive(Serialize, Deserialize)]
pub struct RangeLimits {
    pub min_vert_rate_hz: i32,
    pub max_vert_rate_hz: i32,
    pub min_horiz_rate_hz: i32,
    pub max_horiz_rate_hz: i32,
    pub max_pixel_clock_hz: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct Hdr {
    pub traditional_sdr: bool,
    pub traditional_hdr: bool,
    pub pq: bool,
    pub hlg: bool,
    pub max_luminance: f32,
    pub max_frame_avg_luminance: f32,
    pub min_luminance: f32,
}

#[derive(Serialize, Deserialize)]
pub struct ColorPrimaries {
    pub has_primaries: bool,
    pub red: [f32; 2],
    pub green: [f32; 2],
    pub blue: [f32; 2],
    pub white: [f32; 2],
}

#[derive(Serialize, Deserialize)]
pub struct Colorimetry {
    pub bt2020_rgb: bool,
    pub bt2020_ycc: bool,
    pub bt2020_cycc: bool,
    pub st2113_rgb: bool,
    pub ictcp: bool,
}

pub fn list() -> Vec<Monitor> {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };

    let mut monitors = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(bytes) = fs::read(path.join("edid")) else {
            continue;
        };
        let Ok(info) = Info::parse_edid(&bytes) else {
            continue;
        };
        let Some(edid) = info.edid() else {
            continue;
        };

        let connector = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches("card")
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches('-')
            .to_string();

        let vp = edid.vendor_product();
        let ss = edid.screen_size();

        let video_input = edid.video_input_digital().map(|d| VideoInput {
            interface: format!("{:?}", d.interface),
            color_bit_depth: d.color_bit_depth,
        });

        let detailed_timings = edid
            .detailed_timing_defs()
            .map(|t| DetailedTiming {
                pixel_clock_hz: t.pixel_clock_hz,
                horiz_video: t.horiz_video,
                vert_video: t.vert_video,
                horiz_image_mm: t.horiz_image_mm,
                vert_image_mm: t.vert_image_mm,
                interlaced: t.interlaced,
            })
            .collect();

        let range_limits = edid
            .display_descriptors()
            .iter()
            .filter_map(|d| d.range_limits())
            .map(|r| RangeLimits {
                min_vert_rate_hz: r.min_vert_rate_hz,
                max_vert_rate_hz: r.max_vert_rate_hz,
                min_horiz_rate_hz: r.min_horiz_rate_hz,
                max_horiz_rate_hz: r.max_horiz_rate_hz,
                max_pixel_clock_hz: r.max_pixel_clock_hz,
            })
            .collect();

        let h = info.hdr_static_metadata();
        let cp = info.default_color_primaries();
        let c = info.supported_signal_colorimetry();

        monitors.push(Monitor {
            connector,
            make: info.make(),
            model: info.model(),
            serial: info.serial(),
            edid_version: format!("{}.{}", edid.version(), edid.revision()),
            manufacturer: vp.manufacturer.iter().collect(),
            product_code: vp.product,
            serial_number: vp.serial,
            manufacture_week: vp.manufacture_week,
            manufacture_year: vp.manufacture_year,
            model_year: vp.model_year,
            gamma: info.default_gamma().or_else(|| edid.basic_gamma()),
            video_input,
            screen: Screen {
                width_cm: ss.width_cm,
                height_cm: ss.height_cm,
            },
            detailed_timings,
            range_limits,
            hdr: Hdr {
                traditional_sdr: h.traditional_sdr,
                traditional_hdr: h.traditional_hdr,
                pq: h.pq,
                hlg: h.hlg,
                max_luminance: h.desired_content_max_luminance,
                max_frame_avg_luminance: h.desired_content_max_frame_avg_luminance,
                min_luminance: h.desired_content_min_luminance,
            },
            color_primaries: ColorPrimaries {
                has_primaries: cp.has_primaries,
                red: [cp.primary[0].x, cp.primary[0].y],
                green: [cp.primary[1].x, cp.primary[1].y],
                blue: [cp.primary[2].x, cp.primary[2].y],
                white: [cp.default_white.x, cp.default_white.y],
            },
            colorimetry: Colorimetry {
                bt2020_rgb: c.bt2020_rgb,
                bt2020_ycc: c.bt2020_ycc,
                bt2020_cycc: c.bt2020_cycc,
                st2113_rgb: c.st2113_rgb,
                ictcp: c.ictcp,
            },
        });
    }

    monitors
}
