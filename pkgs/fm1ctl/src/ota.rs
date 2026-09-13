//! The firmware write itself.
//!
//! The device drives: it asks for byte ranges of the image and the host
//! answers. Two addresses are signals rather than offsets, and they are how
//! each phase ends. `answer` is the whole decision, kept pure so it can be
//! tested against a simulated device; the loop around it is only I/O.

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::codec::{ADDR_UPGRADED, ADDR_VERIFIED, Packet, Request, SUCCESS};
use crate::midi;

/// Per-request receive window. The vendor tool waits 8 s.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// After the reboot command, before the device is worth talking to again.
const SETTLE: Duration = Duration::from_secs(3);
/// How long to wait for the port to come back after a reboot.
const REENUMERATE_TIMEOUT: Duration = Duration::from_secs(30);
/// A runaway device shouldn't spin forever.
const MAX_REQUESTS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Serve these bytes back.
    Serve(Packet),
    /// The device finished checking the image.
    Verified,
    /// The device finished writing it.
    Upgraded,
}

/// What to send back for one request. Out-of-range reads are an error rather
/// than a short or zero-filled reply: the device asked for something this
/// image cannot satisfy, which means the wrong file or a bad assumption, and
/// neither should be papered over mid-write.
pub fn answer(image: &[u8], request: Request) -> Result<Answer> {
    match request.address {
        ADDR_VERIFIED => return Ok(Answer::Verified),
        ADDR_UPGRADED => return Ok(Answer::Upgraded),
        _ => {}
    }

    let start = request.address as usize;
    let len = request.length as usize;
    let end = start
        .checked_add(len)
        .context("request length overflows its address")?;
    ensure!(
        end <= image.len(),
        "device asked for {len} bytes at {:#010x}, past the end of a {}-byte image",
        request.address,
        image.len()
    );

    Ok(Answer::Serve(request.reply(&image[start..end])))
}

/// Both sentinels are acknowledged the same way, with a fixed 8-byte payload.
fn acknowledge(request: Request) -> Packet {
    request.reply(SUCCESS)
}

/// Serve requests until the device signals the end of a phase.
fn serve(link: &mut midi::Link, image: &[u8]) -> Result<Answer> {
    for n in 1..=MAX_REQUESTS {
        let packet = link
            .recv(REQUEST_TIMEOUT)
            .with_context(|| format!("waiting for request {n}"))?;
        let request =
            Request::parse(&packet.payload).with_context(|| format!("parsing request {n}"))?;

        match answer(image, request)? {
            Answer::Serve(reply) => {
                link.send(&reply)?;
                if n % 100 == 0 {
                    eprint!("\r  served {n} requests");
                }
            }
            done => {
                link.send(&acknowledge(request))?;
                eprintln!("\r  served {n} requests");
                return Ok(done);
            }
        }
    }
    bail!("device asked for more than {MAX_REQUESTS} blocks without finishing");
}

/// Wait for the device to drop off and come back, then reopen it. The port
/// name changes when it reboots into OTA mode, so the search is by shape
/// rather than by remembering the old name.
fn reopen(timeout: Duration) -> Result<midi::Link> {
    let deadline = Instant::now() + timeout;
    let mut last = None;

    while Instant::now() < deadline {
        sleep(Duration::from_millis(250));
        let ports = match midi::ports() {
            Ok(ports) => ports,
            Err(e) => {
                last = Some(e);
                continue;
            }
        };
        let Some(port) = crate::guess_port(&ports) else {
            continue;
        };
        match midi::Link::open(&port.hw()) {
            Ok(link) => {
                eprintln!("  device back on {} ({})", port.hw(), port.name);
                return Ok(link);
            }
            Err(e) => last = Some(e),
        }
    }

    match last {
        Some(e) => Err(e).context("device did not come back after the reboot"),
        None => bail!("device did not come back after the reboot"),
    }
}

/// Reboot into OTA mode, then run a phase. The same literal does both jobs;
/// in normal mode it reboots the device, and in OTA mode it starts a phase.
fn phase(link: &mut midi::Link, image: &[u8], label: &str) -> Result<Answer> {
    eprintln!("{label}: entering OTA mode");
    link.send_raw(&crate::codec::ENTER_OTA)?;
    sleep(SETTLE);
    link.drain()?;
    serve(link, image)
}

/// The full sequence. Irreversible from the first `ENTER_OTA` onward, so
/// every check the caller wants to make has to happen before this is called.
pub fn run(hw: &str, image: &[u8]) -> Result<()> {
    let mut link = midi::Link::open(hw)?;

    match phase(&mut link, image, "verify")? {
        Answer::Verified => {}
        Answer::Upgraded => {
            eprintln!("device reported the upgrade finished during the verify phase");
            return Ok(());
        }
        Answer::Serve(_) => unreachable!("serve only returns on a sentinel"),
    }

    // The device reboots between phases and comes back under a new port name.
    drop(link);
    let mut link = reopen(REENUMERATE_TIMEOUT)?;

    match phase(&mut link, image, "write")? {
        Answer::Upgraded => {
            eprintln!("upgrade reported complete; the device will reboot");
            Ok(())
        }
        Answer::Verified => bail!("device signalled verification again instead of completion"),
        Answer::Serve(_) => unreachable!("serve only returns on a sentinel"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{Request, TYPE_DATA};

    fn image(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31) ^ 0x5A)
            .collect()
    }

    fn request(address: u32, length: u32) -> Request {
        Request {
            flash_type: 0,
            address,
            length,
        }
    }

    #[test]
    fn serves_exactly_the_bytes_asked_for() {
        let image = image(4096);
        let Answer::Serve(packet) = answer(&image, request(0x200, 512)).unwrap() else {
            panic!("expected data");
        };
        assert_eq!(packet.kind, TYPE_DATA);
        assert_eq!(packet.payload.len(), 8 + 512);
        assert_eq!(&packet.payload[8..], &image[0x200..0x200 + 512]);
    }

    #[test]
    fn recognises_both_sentinels() {
        let image = image(4096);
        assert_eq!(
            answer(&image, request(ADDR_VERIFIED, 8)).unwrap(),
            Answer::Verified
        );
        assert_eq!(
            answer(&image, request(ADDR_UPGRADED, 8)).unwrap(),
            Answer::Upgraded
        );
        // Both are acknowledged with the same fixed payload.
        assert_eq!(
            &acknowledge(request(ADDR_UPGRADED, 8)).payload[8..],
            SUCCESS
        );
    }

    #[test]
    fn refuses_to_read_past_the_image() {
        let image = image(1024);
        assert!(answer(&image, request(1024, 1)).is_err());
        assert!(answer(&image, request(1000, 512)).is_err());
        assert!(answer(&image, request(u32::MAX, 512)).is_err());
        // The last byte exactly is fine.
        assert!(answer(&image, request(1023, 1)).is_ok());
    }

    /// Drive `answer` the way the device actually does: non-monotonic reads,
    /// repeats, odd lengths, then a sentinel. Anything the host serves must
    /// reconstruct the image exactly where it was asked for.
    #[test]
    fn a_simulated_device_can_reassemble_the_whole_image() {
        let image = image(699_956);
        let mut seen = vec![false; image.len()];

        // A deliberately awkward order: backwards in big strides, with the
        // odd re-read and short tail request mixed in.
        let lengths = [512usize, 32, 16, 8, 481, 399, 339, 64];
        let mut cursor = 0usize;
        let mut requests = 0;
        for round in 0..3 {
            cursor = (cursor + 251) % image.len();
            let mut at = if round % 2 == 0 { 0 } else { cursor };
            let mut which = round;
            while at < image.len() {
                let len = lengths[which % lengths.len()].min(image.len() - at);
                which += 1;
                let Answer::Serve(packet) = answer(&image, request(at as u32, len as u32)).unwrap()
                else {
                    panic!("expected data");
                };
                let served = &packet.payload[8..];
                assert_eq!(served, &image[at..at + len], "at {at:#x} len {len}");
                for b in &mut seen[at..at + len] {
                    *b = true;
                }
                requests += 1;
                at += len;
            }
        }

        assert!(requests > 1000, "only {requests} requests");
        assert!(seen.iter().all(|b| *b), "some bytes never served");

        assert_eq!(
            answer(&image, request(ADDR_UPGRADED, 8)).unwrap(),
            Answer::Upgraded
        );
    }
}
