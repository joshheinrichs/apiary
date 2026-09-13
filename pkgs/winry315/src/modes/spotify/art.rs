//! Album art: fetch it, cache it, and boil it down to something the pad can
//! show. Slow and networked, so nothing here runs on the render path.

use crate::grid::{GRID_H, GRID_W};
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::PathBuf;

/// Album art is a few hundred KB of JPEG; anything larger is not album art.
const ART_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// One cover, distilled to a colour per grid cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Album {
    pub grid: [[u8; 3]; GRID_W * GRID_H],
}

/// Decode one cover onto the grid.
///
/// Blurhash rather than a box average: it low-passes in *linear* light, which
/// is where an average turns complementary colours to grey, and at 5x4 each
/// cell would otherwise average ~30,000 pixels of a 640x640 cover. Gamma
/// correction on the way to the LEDs restores the saturation band-limiting
/// costs.
///
/// The cover is square and the grid is 5:4, so the centre is kept and the top
/// and bottom lost -- covers put their subject in the middle.
pub fn album_of_art(jpeg: &[u8]) -> Result<Album> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .context("decoding album art")?
        .into_rgb8();

    let (w, h) = img.dimensions();
    let want = GRID_W as f32 / GRID_H as f32;
    let (cw, ch) = match (w as f32 / h as f32) > want {
        true => ((h as f32 * want) as u32, h),
        false => (w, (w as f32 / want) as u32),
    };
    let cropped = image::imageops::crop_imm(&img, (w - cw) / 2, (h - ch) / 2, cw, ch).to_image();
    let rgba = image::DynamicImage::ImageRgb8(cropped).into_rgba8();

    let hash = blurhash::encode(4, 3, rgba.width(), rgba.height(), rgba.as_raw())
        .map_err(|e| anyhow!("blurhash encode: {e:?}"))?;
    let small = blurhash::decode(&hash, GRID_W as u32, GRID_H as u32, 1.0)
        .map_err(|e| anyhow!("blurhash decode: {e:?}"))?;
    // Images count rows downward from the top; the pad counts them upward from
    // the bottom, because row 0 of a column is its lowest LED. Flip, or every
    // cover shows upside down.
    let grid = std::array::from_fn(|i| {
        let (row, column) = (i / GRID_W, i % GRID_W);
        let pixel = ((GRID_H - 1 - row) * GRID_W + column) * 4;
        [small[pixel], small[pixel + 1], small[pixel + 2]]
    });

    Ok(Album { grid })
}

/// The art cache lives under `$XDG_CACHE_HOME/winry315-daemon`, keyed by the
/// URL's content hash. Network is the expensive part; re-extracting from a
/// cached JPEG is milliseconds, so the image is what we keep.
fn cache_dir() -> Option<PathBuf> {
    xdg::BaseDirectories::with_prefix("winry315-daemon")
        .ok()?
        .create_cache_directory("art")
        .ok()
}

/// The last path segment of a Spotify art URL is a content hash. Reject
/// anything that is not, rather than letting a URL choose where we write.
fn cache_key(url: &str) -> Option<&str> {
    let key = url.rsplit('/').next()?;
    let sane = !key.is_empty()
        && key.len() <= 64
        && key.chars().all(|c| c.is_ascii_alphanumeric());
    sane.then_some(key)
}

/// Fetch album art, preferring the cache. A cache that cannot be read or
/// written is not an error: it just means going to the network.
pub fn load_art(url: &str) -> Result<Vec<u8>> {
    let cached = cache_dir().zip(cache_key(url)).map(|(dir, key)| dir.join(key));
    if let Some(path) = &cached
        && let Ok(bytes) = fs::read(path)
    {
        return Ok(bytes);
    }
    let bytes = ureq::get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?
        .body_mut()
        .with_config()
        .limit(ART_MAX_BYTES)
        .read_to_vec()
        .context("reading album art")?;
    if let Some(path) = &cached {
        // Write then rename, so a crash mid-write cannot leave a torn JPEG that
        // would poison the cache for that album forever.
        let part = path.with_extension("part");
        if fs::write(&part, &bytes).and_then(|_| fs::rename(&part, path)).is_err() {
            eprintln!("winry315: could not cache album art");
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_accepts_content_hashes_and_rejects_paths() {
        assert_eq!(
            cache_key("https://i.scdn.co/image/ab67616d0000b27364096943f064249144d1a972"),
            Some("ab67616d0000b27364096943f064249144d1a972")
        );
        // Only the last segment is ever used, so a key can never contain a
        // separator and the join can never leave the cache directory.
        for url in ["https://evil/../../etc/passwd", "https://x/a/b/c"] {
            assert!(cache_key(url).is_none_or(|k| !k.contains('/')));
        }
        assert_eq!(cache_key("https://evil/.."), None);
        assert_eq!(cache_key("https://evil/a b"), None);
        assert_eq!(cache_key("https://evil/"), None);
        assert_eq!(cache_key(&format!("https://evil/{}", "a".repeat(65))), None);
    }
}
