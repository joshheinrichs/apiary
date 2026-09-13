//! Fetching, parsing and storing firmware images.
//!
//! The vendor serves one unversioned URL and overwrites it in place on each
//! release, so a downloaded image is not re-downloadable once they move on.
//! That is why these live under `XDG_STATE_HOME` rather than in a cache: a
//! cleaner would throw away the only remaining copy of a build.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

pub const URL: &str =
    "https://yms-file-store.oss-cn-hongkong.aliyuncs.com/software/firmware/FM-1.fwsc";

/// The container opens with a table of 48-byte slots carrying one character
/// each, at byte 47, obfuscated by subtracting the slot's own index plus one.
/// Decoded they spell `NAME_VVV`, e.g. `FM-1_015`. `}` in the *raw* byte ends
/// the string. The vendor tool tries 36 slots and falls back to 20.
const SLOT: usize = 48;
const SLOT_CHAR: usize = 47;
const SLOT_COUNTS: [usize; 2] = [36, 20];
const TERMINATOR: u8 = b'}';
const SEPARATOR: u8 = b'_';
/// Version digits are positional, hundreds first, so three at most.
const FIRST_PLACE: u32 = 100;

/// JieLi's container magic, in the last 16 bytes.
const TRAILER: &[u8] = b"JLUFW";
/// Generous next to the ~700 KB real images, tight enough to bound a bad URL.
const MAX_LEN: u64 = 8 * 1024 * 1024;

const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub name: String,
    pub version: u32,
}

impl Image {
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        let tail_start = bytes
            .len()
            .checked_sub(16)
            .context("far too short to be a firmware container")?;
        ensure!(
            bytes[tail_start..]
                .windows(TRAILER.len())
                .any(|w| w == TRAILER),
            "no {} trailer in the last 16 bytes -- truncated, or not a .fwsc",
            String::from_utf8_lossy(TRAILER)
        );

        let (name, version) = SLOT_COUNTS
            .iter()
            .find_map(|slots| read_slots(&bytes, *slots))
            .context("no readable name and version in the slot table")?;

        Ok(Self {
            sha256: hex(&Sha256::digest(&bytes)),
            name,
            version,
            bytes,
        })
    }

    /// The vendor's own convention, so a stored file is recognisable next to
    /// one downloaded by hand.
    pub fn filename(&self) -> String {
        format!("{}_{:03}.fwsc", self.name, self.version)
    }

    pub fn label(&self) -> String {
        format!("{} v{}", self.name, self.version)
    }
}

/// Walk the slot table. `None` for any layout that does not decode cleanly,
/// so the caller can try the next slot count.
///
/// Stricter than the vendor tool in one respect: it accepts a table with no
/// terminator, which leaves the version at zero. Refusing that is worth it
/// when the output decides what gets written to a device with no recovery
/// path.
fn read_slots(bytes: &[u8], slots: usize) -> Option<(String, u32)> {
    if bytes.len() < slots * SLOT {
        return None;
    }

    let mut name = String::new();
    let mut version: u32 = 0;
    let mut place = FIRST_PLACE;
    let mut reading_version = false;
    let mut terminated = false;

    for i in 0..slots {
        let raw = bytes[i * SLOT + SLOT_CHAR];
        let ch = raw.wrapping_sub(i as u8).wrapping_sub(1);

        if terminated {
            // Everything past the terminator is padding, and must look it.
            if raw != TERMINATOR {
                return None;
            }
            continue;
        }

        if !reading_version {
            if ch == SEPARATOR {
                reading_version = true;
            } else if ch.is_ascii_graphic() {
                name.push(char::from(ch));
            } else {
                return None;
            }
            continue;
        }

        if raw == TERMINATOR {
            terminated = true;
            continue;
        }
        let digit = u32::from(ch.checked_sub(b'0')?);
        if digit > 9 || place == 0 {
            return None;
        }
        version += digit * place;
        place /= 10;
    }

    if !terminated || name.is_empty() {
        return None;
    }
    Some((name, version))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn dir() -> Result<PathBuf> {
    let base = xdg::BaseDirectories::with_prefix("fm1ctl");
    let dir = base
        .get_state_home()
        .context("no XDG state directory available")?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Every stored image, best last. Bad files are reported rather than silently
/// skipped -- a corrupt image in the store is worth knowing about before a
/// flash, not during one.
pub fn stored() -> Result<Vec<(PathBuf, Image)>> {
    let dir = dir()?;
    let mut found = Vec::new();

    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "fwsc") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        match Image::parse(bytes) {
            Ok(image) => found.push((path, image)),
            Err(e) => eprintln!("ignoring {}: {e:#}", path.display()),
        }
    }

    found.sort_by_key(|(path, image)| (image.version, mtime(path)));
    Ok(found)
}

/// What a flash should use: the highest version stored.
pub fn best() -> Result<Option<(PathBuf, Image)>> {
    Ok(stored()?.pop())
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Download from the vendor. Returns the image and whatever they said about
/// when it was published.
pub fn fetch() -> Result<(Image, Option<String>)> {
    eprintln!("fetching {URL}");

    let mut response = ureq::get(URL)
        .config()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .call()
        .with_context(|| format!("requesting {URL}"))?;

    let published = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_LEN)
        .read_to_vec()
        .context("reading the response body")?;

    let image = Image::parse(bytes).context("the download is not a usable firmware container")?;
    Ok((image, published))
}

/// Write an image into the store. Returns the path and whether it was new.
/// A same-named file whose contents differ is a release that got respun under
/// the same version, which is worth refusing rather than silently resolving.
pub fn store(image: &Image) -> Result<(PathBuf, bool)> {
    let path = dir()?.join(image.filename());
    if path.exists() {
        let existing = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        if existing == image.bytes {
            return Ok((path, false));
        }
        bail!(
            "{} already exists with different contents (stored {}, downloaded {}) -- \
             move it aside if the download is the one you want",
            path.display(),
            hex(&Sha256::digest(&existing)),
            image.sha256
        );
    }
    fs::write(&path, &image.bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok((path, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot bytes 0..8 read verbatim off the real V15 download, then `}`
    /// padding as the vendor writes it.
    const V15_SLOTS: [u8; 8] = [0x47, 0x4f, 0x30, 0x35, 0x64, 0x36, 0x38, 0x3d];

    fn container(slot_chars: &[u8], slots: usize, len: usize) -> Vec<u8> {
        let mut v = vec![0xAB; len.max(slots * SLOT)];
        for i in 0..slots {
            v[i * SLOT + SLOT_CHAR] = slot_chars.get(i).copied().unwrap_or(TERMINATOR);
        }
        let tail = v.len() - 16;
        v[tail..tail + TRAILER.len()].copy_from_slice(TRAILER);
        v
    }

    #[test]
    fn reads_the_real_v15_slot_bytes() {
        let image = Image::parse(container(&V15_SLOTS, 20, 2048)).unwrap();
        assert_eq!(image.name, "FM-1");
        assert_eq!(image.version, 15);
        assert_eq!(image.filename(), "FM-1_015.fwsc");
        assert_eq!(image.label(), "FM-1 v15");
    }

    /// The only difference between the V14 and V15 containers' slot tables is
    /// this one byte -- the last version digit.
    #[test]
    fn reads_v14_from_the_neighbouring_byte() {
        let mut slots = V15_SLOTS;
        slots[7] = 0x3c;
        let image = Image::parse(container(&slots, 20, 2048)).unwrap();
        assert_eq!((image.name.as_str(), image.version), ("FM-1", 14));
    }

    #[test]
    fn rejects_a_truncated_download() {
        // The realistic failure: bytes arrive, but not all of them, so the
        // trailer is gone.
        let mut bytes = container(&V15_SLOTS, 20, 2048);
        bytes.truncate(1500);
        let err = Image::parse(bytes).unwrap_err().to_string();
        assert!(err.contains("JLUFW"), "{err}");
    }

    #[test]
    fn rejects_a_table_with_no_terminator() {
        // Every slot a digit: the vendor would accept this and report a
        // nonsense version.
        let slots: Vec<u8> = (0..20u8)
            .map(|i| b'5'.wrapping_add(i).wrapping_add(1))
            .collect();
        assert!(Image::parse(container(&slots, 20, 2048)).is_err());
    }

    #[test]
    fn rejects_noise_that_happens_to_have_a_trailer() {
        let mut bytes = vec![0xAB; 2048];
        let tail = bytes.len() - 16;
        bytes[tail..tail + TRAILER.len()].copy_from_slice(TRAILER);
        assert!(Image::parse(bytes).is_err());
    }

    #[test]
    fn rejects_something_far_too_small() {
        assert!(Image::parse(vec![0; 8]).is_err());
        assert!(Image::parse(vec![0; 64]).is_err());
    }
}
