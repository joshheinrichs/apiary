//! Wire format for the M-Vave FM-1 OTA protocol. Pure -- no I/O lives here.
//!
//! A packet is framed as:
//!
//! ```text
//! 00 59 | type | u24le payload_len | payload | !sum(payload)
//! ```
//!
//! and then carried inside a single MIDI SysEx message, with the packet bytes
//! packed as a little-endian 7-bit bitstream between `F0` and `F7`.

use anyhow::{Context, Result, bail, ensure};

pub const MAGIC: [u8; 2] = [0x00, 0x59];

pub const TYPE_IDENTIFY: u8 = 0x11;
pub const TYPE_DATA: u8 = 0x30;

/// The device's identify reply: 7 header/checksum bytes around a 27-byte
/// payload. The host rejects anything else.
pub const IDENTIFY_REPLY_LEN: usize = 0x22;

/// Reboots the device into OTA mode, and confirms the upgrade once it is
/// there. Sent as a literal MIDI message -- this is *not* a `Packet`, so it
/// bypasses `encode_sysex` entirely.
#[allow(dead_code)]
pub const ENTER_OTA: [u8; 6] = [0xF0, 0x22, 0x24, 0x35, 0x7F, 0xF7];

// The rest of this module is the serve loop the `flash` command will use. It
// is verified by the tests below but has no caller yet.

/// Addresses the device asks for that are signals rather than file offsets.
#[allow(dead_code)]
pub const ADDR_VERIFIED: u32 = 0xE000_0000;
#[allow(dead_code)]
pub const ADDR_UPGRADED: u32 = 0xF000_0000;

/// What the host answers a sentinel address with.
#[allow(dead_code)]
pub const SUCCESS: &[u8; 8] = b"success\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub kind: u8,
    pub payload: Vec<u8>,
}

/// One block of firmware the device has asked for. `length` is what it wants,
/// not what it got.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub flash_type: u8,
    pub address: u32,
    pub length: u32,
}

/// What the device calls itself: a single NUL-terminated `name_version`
/// string, e.g. `FM-1_015`. Same convention as the `.fwsc` filenames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub version: u32,
}

impl Identity {
    pub fn parse(payload: &[u8]) -> Result<Self> {
        let text: String = payload
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as char)
            .collect();
        let (name, version) = text
            .rsplit_once('_')
            .with_context(|| format!("no '_' separating name from version in {text:?}"))?;
        let version = version
            .parse()
            .with_context(|| format!("version {version:?} in {text:?} is not a number"))?;
        Ok(Self {
            name: name.to_string(),
            version,
        })
    }
}

pub fn checksum(payload: &[u8]) -> u8 {
    !payload.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

fn u24le(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

fn read_u24le(b: &[u8]) -> u32 {
    u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16
}

#[allow(dead_code)]
fn read_u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

impl Packet {
    pub fn new(kind: u8, payload: Vec<u8>) -> Self {
        Self { kind, payload }
    }

    /// A type-only packet with an empty payload -- the identify query.
    pub fn query(kind: u8) -> Self {
        Self::new(kind, Vec::new())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(self.kind);
        out.extend_from_slice(&u24le(self.payload.len() as u32));
        out.extend_from_slice(&self.payload);
        out.push(checksum(&self.payload));
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 7, "packet is {} bytes, need 7", bytes.len());
        ensure!(
            bytes[..2] == MAGIC,
            "bad magic {:02x?}, expected {:02x?}",
            &bytes[..2],
            MAGIC
        );

        let len = read_u24le(&bytes[3..6]) as usize;
        let total = 7 + len;
        ensure!(
            bytes.len() >= total,
            "packet claims {len} payload bytes but only {} are present",
            bytes.len().saturating_sub(7)
        );

        let payload = bytes[6..6 + len].to_vec();
        let want = checksum(&payload);
        let got = bytes[6 + len];
        ensure!(got == want, "checksum {got:#04x}, computed {want:#04x}");

        Ok(Self::new(bytes[2], payload))
    }
}

#[allow(dead_code)]
impl Request {
    /// The device's request payload is always these 8 bytes.
    pub fn parse(payload: &[u8]) -> Result<Self> {
        ensure!(
            payload.len() == 8,
            "request payload is {} bytes, expected 8",
            payload.len()
        );
        Ok(Self {
            flash_type: payload[0],
            address: read_u32le(&payload[1..5]),
            length: read_u24le(&payload[5..8]),
        })
    }

    fn to_payload(self) -> [u8; 8] {
        let addr = self.address.to_le_bytes();
        let len = u24le(self.length);
        [
            self.flash_type,
            addr[0],
            addr[1],
            addr[2],
            addr[3],
            len[0],
            len[1],
            len[2],
        ]
    }

    /// How the device frames a request: the payload alone, no data.
    pub fn to_packet(self) -> Packet {
        Packet::new(TYPE_DATA, self.to_payload().to_vec())
    }

    /// The reply echoes the request, except that `length` becomes the number
    /// of bytes actually served, and the data follows.
    pub fn reply(self, data: &[u8]) -> Packet {
        let echo = Self {
            length: data.len() as u32,
            ..self
        };
        let mut payload = Vec::with_capacity(8 + data.len());
        payload.extend_from_slice(&echo.to_payload());
        payload.extend_from_slice(data);
        Packet::new(TYPE_DATA, payload)
    }
}

/// Pack packet bytes into a SysEx message: `F0`, a little-endian 7-bit
/// bitstream, `F7`.
pub fn encode_sysex(packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + packet.len() * 8 / 7 + 2);
    out.push(0xF0);

    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for b in packet {
        acc |= u32::from(*b) << bits;
        bits += 8;
        while bits >= 7 {
            out.push((acc & 0x7F) as u8);
            acc >>= 7;
            bits -= 7;
        }
    }
    if bits > 0 {
        out.push((acc & 0x7F) as u8);
    }

    out.push(0xF7);
    out
}

/// The inverse. `body` is the bytes between `F0` and `F7`, exclusive.
pub fn decode_sysex(body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len() * 7 / 8 + 1);

    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for b in body {
        if b & 0x80 != 0 {
            bail!("byte {b:#04x} in SysEx body has its high bit set");
        }
        acc |= u32::from(*b) << bits;
        bits += 7;
        while bits >= 8 {
            out.push((acc & 0xFF) as u8);
            acc >>= 8;
            bits -= 8;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one byte sequence we have independent confirmation of: an identify
    /// query observed on the wire by third-party capture. Everything about the
    /// codec -- magic, u24 length, inverted checksum, bitstream packing -- has
    /// to be right for this to come out.
    #[test]
    fn identify_query_matches_captured_bytes() {
        let packet = Packet::query(TYPE_IDENTIFY);
        assert_eq!(
            packet.to_bytes(),
            [0x00, 0x59, 0x11, 0x00, 0x00, 0x00, 0xFF]
        );
        assert_eq!(
            encode_sysex(&packet.to_bytes()),
            [0xF0, 0x00, 0x32, 0x45, 0x00, 0x00, 0x00, 0x40, 0x7F, 0xF7]
        );
    }

    /// The same capture's reply decodes to a well-formed 34-byte packet, which
    /// is exactly the length the vendor tool insists on.
    #[test]
    fn captured_identify_reply_decodes() {
        let body = [0x00, 0x32, 0x45, 0x58, 0x01, 0x00, 0x00];
        let decoded = decode_sysex(&body).unwrap();
        assert_eq!(decoded[..3], [0x00, 0x59, 0x11]);
        assert_eq!(read_u24le(&decoded[3..6]) as usize + 7, IDENTIFY_REPLY_LEN);
    }

    #[test]
    fn sysex_round_trips_every_length() {
        for len in 0..64usize {
            let packet: Vec<u8> = (0..len)
                .map(|i| (i as u8).wrapping_mul(37) ^ 0xA5)
                .collect();
            let sysex = encode_sysex(&packet);
            assert_eq!(sysex[0], 0xF0);
            assert_eq!(*sysex.last().unwrap(), 0xF7);
            assert!(sysex[1..sysex.len() - 1].iter().all(|b| b & 0x80 == 0));

            let decoded = decode_sysex(&sysex[1..sysex.len() - 1]).unwrap();
            // The bitstream is byte-aligned only by luck, so a trailing partial
            // byte is expected; the packet's own length field bounds the parse.
            assert_eq!(decoded[..len], packet[..]);
        }
    }

    #[test]
    fn packet_round_trips_through_sysex() {
        let packet = Packet::new(TYPE_DATA, (0..520).map(|i| i as u8).collect());
        let sysex = encode_sysex(&packet.to_bytes());
        let decoded = decode_sysex(&sysex[1..sysex.len() - 1]).unwrap();
        assert_eq!(Packet::parse(&decoded).unwrap(), packet);
    }

    #[test]
    fn request_parses_and_replies() {
        // flash_type 0, address 0x00094200, length 512 -- a real logged request.
        let payload = [0x00, 0x00, 0x42, 0x09, 0x00, 0x00, 0x02, 0x00];
        let req = Request::parse(&payload).unwrap();
        assert_eq!(req.flash_type, 0);
        assert_eq!(req.address, 0x0009_4200);
        assert_eq!(req.length, 512);

        let data = vec![0xEE; 512];
        let reply = req.reply(&data);
        assert_eq!(reply.kind, TYPE_DATA);
        assert_eq!(reply.payload.len(), 520);
        assert_eq!(reply.payload[..8], payload);
        assert_eq!(reply.to_bytes().len(), 527);
    }

    #[test]
    fn a_request_round_trips_as_the_device_would_send_it() {
        let req = Request {
            flash_type: 0,
            address: ADDR_UPGRADED,
            length: 8,
        };
        // This is the exact 15 bytes the host must accept and parse.
        let on_wire = req.to_packet().to_bytes();
        assert_eq!(on_wire.len(), 15);
        let parsed = Packet::parse(&on_wire).unwrap();
        assert_eq!(parsed.kind, TYPE_DATA);
        assert_eq!(Request::parse(&parsed.payload).unwrap(), req);
    }

    #[test]
    fn identity_parses_what_the_device_actually_sent() {
        // Captured from an FM-1 over USB: 27 bytes, NUL-padded.
        let mut payload = b"FM-1_015".to_vec();
        payload.resize(27, 0);
        let id = Identity::parse(&payload).unwrap();
        assert_eq!(id.name, "FM-1");
        assert_eq!(id.version, 15);
    }

    #[test]
    fn identity_keeps_underscores_in_a_name() {
        let id = Identity::parse(b"TANK_PRO_033\0\0").unwrap();
        assert_eq!(id.name, "TANK_PRO");
        assert_eq!(id.version, 33);
    }

    #[test]
    fn identity_rejects_junk() {
        assert!(Identity::parse(b"FM-1\0").is_err());
        assert!(Identity::parse(b"FM-1_xx\0").is_err());
    }

    #[test]
    fn parse_rejects_a_corrupt_checksum() {
        let mut bytes = Packet::new(TYPE_DATA, vec![1, 2, 3]).to_bytes();
        *bytes.last_mut().unwrap() ^= 0xFF;
        assert!(Packet::parse(&bytes).is_err());
    }

    #[test]
    fn parse_rejects_foreign_sysex() {
        // A stray MIDI clock or another vendor's message must not be mistaken
        // for a request.
        assert!(Packet::parse(&[0x7E, 0x00, 0x06, 0x01, 0x00, 0x00, 0x00]).is_err());
        assert!(Packet::parse(&[0x00, 0x59]).is_err());
    }
}
