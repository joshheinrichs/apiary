//! seatd wire protocol.
//!
//! Children reach seatmux through libseat's `seatd` backend, so this must match
//! what libseat expects rather than anything of our own choosing. Shape, from
//! seatd's `protocol.h` and libseat's `backend/seatd.c`:
//!
//! - every message is a 4-byte header (`opcode: u16`, `size: u16`) in native
//!   byte order, where `size` counts the body *after* the header
//! - each client request has exactly one response; the client reads it
//!   synchronously and rejects any other opcode
//! - `DisableSeat` and `EnableSeat` are the only messages a server may push
//!   asynchronously. Sending anything else unprompted desynchronises the client.
//! - `Error` may stand in for any response, and its `code` is a positive errno
//!   the client assigns straight to `errno`

use anyhow::{Result, bail};

pub const MAX_PATH_LEN: usize = 256;
pub const MAX_SEAT_LEN: usize = 64;

const HEADER_LEN: usize = 4;
const SERVER_BIT: u16 = 1 << 15;

const CLIENT_OPEN_SEAT: u16 = 1;
const CLIENT_CLOSE_SEAT: u16 = 2;
const CLIENT_OPEN_DEVICE: u16 = 3;
const CLIENT_CLOSE_DEVICE: u16 = 4;
const CLIENT_DISABLE_SEAT: u16 = 5;
const CLIENT_SWITCH_SESSION: u16 = 6;
const CLIENT_PING: u16 = 7;

const SERVER_SEAT_OPENED: u16 = SERVER_BIT + 1;
const SERVER_SEAT_CLOSED: u16 = SERVER_BIT + 2;
const SERVER_DEVICE_OPENED: u16 = SERVER_BIT + 3;
const SERVER_DEVICE_CLOSED: u16 = SERVER_BIT + 4;
const SERVER_DISABLE_SEAT: u16 = SERVER_BIT + 5;
const SERVER_ENABLE_SEAT: u16 = SERVER_BIT + 6;
const SERVER_PONG: u16 = SERVER_BIT + 7;
const SERVER_SESSION_SWITCHED: u16 = SERVER_BIT + 8;
const SERVER_SEAT_DISABLED: u16 = SERVER_BIT + 9;
const SERVER_ERROR: u16 = SERVER_BIT + 0x7FFF;

/// A request from a child compositor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    OpenSeat,
    CloseSeat,
    OpenDevice { path: String },
    CloseDevice { device_id: i32 },
    DisableSeat,
    SwitchSession { session: i32 },
    Ping,
}

/// The single reply owed to each request. `DeviceOpened` additionally carries a
/// file descriptor in `SCM_RIGHTS` auxiliary data, which is not part of this
/// encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // SessionSwitched completes the protocol; seatmux refuses instead.
pub enum Response {
    SeatOpened { seat_name: String },
    SeatClosed,
    DeviceOpened { device_id: i32 },
    DeviceClosed,
    Pong,
    SessionSwitched,
    SeatDisabled,
    Error { code: i32 },
}

/// Server-pushed messages. These two, and only these two, may be sent without a
/// preceding request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    DisableSeat,
    EnableSeat,
}

fn header(opcode: u16, body_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + body_len);
    out.extend_from_slice(&opcode.to_ne_bytes());
    out.extend_from_slice(&(body_len as u16).to_ne_bytes());
    out
}

fn u16_at(buf: &[u8], off: usize) -> u16 {
    u16::from_ne_bytes([buf[off], buf[off + 1]])
}

fn i32_at(buf: &[u8], off: usize) -> i32 {
    i32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

impl Request {
    /// Decode one request from the front of `buf`.
    ///
    /// Returns `Ok(None)` when `buf` holds less than a whole message, so a
    /// caller can keep reading. On success, returns the request and how many
    /// bytes it consumed.
    pub fn decode(buf: &[u8]) -> Result<Option<(Request, usize)>> {
        if buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let opcode = u16_at(buf, 0);
        let size = u16_at(buf, 2) as usize;
        let total = HEADER_LEN + size;
        if buf.len() < total {
            return Ok(None);
        }
        let body = &buf[HEADER_LEN..total];

        let request = match opcode {
            CLIENT_OPEN_SEAT => Request::OpenSeat,
            CLIENT_CLOSE_SEAT => Request::CloseSeat,
            CLIENT_PING => Request::Ping,
            CLIENT_DISABLE_SEAT => Request::DisableSeat,
            CLIENT_CLOSE_DEVICE => {
                if body.len() < 4 {
                    bail!("close_device body too short: {}", body.len());
                }
                Request::CloseDevice {
                    device_id: i32_at(body, 0),
                }
            }
            CLIENT_SWITCH_SESSION => {
                if body.len() < 4 {
                    bail!("switch_session body too short: {}", body.len());
                }
                Request::SwitchSession {
                    session: i32_at(body, 0),
                }
            }
            CLIENT_OPEN_DEVICE => {
                if body.len() < 2 {
                    bail!("open_device body too short: {}", body.len());
                }
                let path_len = u16_at(body, 0) as usize;
                if path_len == 0 || path_len > MAX_PATH_LEN {
                    bail!("open_device path_len out of range: {path_len}");
                }
                if body.len() < 2 + path_len {
                    bail!("open_device path truncated");
                }
                // path_len counts the trailing NUL that libseat sends.
                let bytes = &body[2..2 + path_len - 1];
                Request::OpenDevice {
                    path: String::from_utf8(bytes.to_vec())?,
                }
            }
            other => bail!("unknown client opcode: {other}"),
        };
        Ok(Some((request, total)))
    }
}

impl Response {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Response::SeatOpened { seat_name } => {
                let name = seat_name.as_bytes();
                let len = name.len() + 1; // includes NUL
                let mut out = header(SERVER_SEAT_OPENED, 2 + len);
                out.extend_from_slice(&(len as u16).to_ne_bytes());
                out.extend_from_slice(name);
                out.push(0);
                out
            }
            Response::SeatClosed => header(SERVER_SEAT_CLOSED, 0),
            Response::DeviceOpened { device_id } => {
                let mut out = header(SERVER_DEVICE_OPENED, 4);
                out.extend_from_slice(&device_id.to_ne_bytes());
                out
            }
            Response::DeviceClosed => header(SERVER_DEVICE_CLOSED, 0),
            Response::Pong => header(SERVER_PONG, 0),
            Response::SessionSwitched => header(SERVER_SESSION_SWITCHED, 0),
            Response::SeatDisabled => header(SERVER_SEAT_DISABLED, 0),
            Response::Error { code } => {
                let mut out = header(SERVER_ERROR, 4);
                out.extend_from_slice(&code.to_ne_bytes());
                out
            }
        }
    }
}

/// The exact reply libseat expects to `OPEN_SEAT`.
///
/// A seat is disabled until the server says otherwise, so an active session must
/// be announced in the same breath. Relaying only logind.s activation *changes*
/// leaves a client that connected to an already-active session waiting forever
/// at "Waiting for a session to become active".
pub fn open_seat_reply(seat_name: &str, active: bool) -> Vec<u8> {
    let mut out = Response::SeatOpened {
        seat_name: seat_name.to_string(),
    }
    .encode();
    if active {
        out.extend_from_slice(&Event::EnableSeat.encode());
    }
    out
}

impl Event {
    pub fn encode(self) -> Vec<u8> {
        match self {
            Event::DisableSeat => header(SERVER_DISABLE_SEAT, 0),
            Event::EnableSeat => header(SERVER_ENABLE_SEAT, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read back the (opcode, body_len) of every message in a server-bound byte
    /// stream, so tests assert on the wire rather than on our own types.
    fn opcodes(buf: &[u8]) -> Vec<(u16, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + HEADER_LEN <= buf.len() {
            let opcode = u16_at(buf, at);
            let size = u16_at(buf, at + 2) as usize;
            out.push((opcode, size));
            at += HEADER_LEN + size;
        }
        assert_eq!(at, buf.len(), "trailing bytes in stream");
        out
    }

    /// Build a client request the way libseat does, so decoding is tested
    /// against the wire form rather than against our own encoder.
    fn wire(opcode: u16, body: &[u8]) -> Vec<u8> {
        let mut out = header(opcode, body.len());
        out.extend_from_slice(body);
        out
    }

    fn open_device_wire(path: &str) -> Vec<u8> {
        let len = path.len() + 1;
        let mut body = (len as u16).to_ne_bytes().to_vec();
        body.extend_from_slice(path.as_bytes());
        body.push(0);
        wire(CLIENT_OPEN_DEVICE, &body)
    }

    #[test]
    fn decodes_bodyless_requests() {
        for (opcode, expected) in [
            (CLIENT_OPEN_SEAT, Request::OpenSeat),
            (CLIENT_CLOSE_SEAT, Request::CloseSeat),
            (CLIENT_PING, Request::Ping),
            (CLIENT_DISABLE_SEAT, Request::DisableSeat),
        ] {
            let buf = wire(opcode, &[]);
            let (req, used) = Request::decode(&buf).unwrap().unwrap();
            assert_eq!(req, expected);
            assert_eq!(used, buf.len());
        }
    }

    #[test]
    fn decodes_requests_with_int_bodies() {
        let buf = wire(CLIENT_CLOSE_DEVICE, &7i32.to_ne_bytes());
        assert_eq!(
            Request::decode(&buf).unwrap().unwrap().0,
            Request::CloseDevice { device_id: 7 }
        );

        let buf = wire(CLIENT_SWITCH_SESSION, &3i32.to_ne_bytes());
        assert_eq!(
            Request::decode(&buf).unwrap().unwrap().0,
            Request::SwitchSession { session: 3 }
        );
    }

    #[test]
    fn decodes_open_device_stripping_nul() {
        let buf = open_device_wire("/dev/input/event0");
        let (req, used) = Request::decode(&buf).unwrap().unwrap();
        assert_eq!(
            req,
            Request::OpenDevice {
                path: "/dev/input/event0".into()
            }
        );
        assert_eq!(used, buf.len());
    }

    /// A short read must be reported as "not yet", never as a parse error, or
    /// the server would drop a connection mid-message.
    #[test]
    fn partial_messages_are_incomplete_not_errors() {
        let buf = open_device_wire("/dev/input/event0");
        for cut in 0..buf.len() {
            assert!(
                Request::decode(&buf[..cut]).unwrap().is_none(),
                "prefix of {cut} bytes should be incomplete"
            );
        }
        assert!(Request::decode(&buf).unwrap().is_some());
    }

    /// Two requests arriving in one read must both be recoverable.
    #[test]
    fn decodes_successive_requests_from_one_buffer() {
        let mut buf = wire(CLIENT_OPEN_SEAT, &[]);
        buf.extend_from_slice(&open_device_wire("/dev/dri/card1"));

        let (first, used) = Request::decode(&buf).unwrap().unwrap();
        assert_eq!(first, Request::OpenSeat);
        let (second, _) = Request::decode(&buf[used..]).unwrap().unwrap();
        assert_eq!(
            second,
            Request::OpenDevice {
                path: "/dev/dri/card1".into()
            }
        );
    }

    #[test]
    fn rejects_malformed_bodies() {
        assert!(Request::decode(&wire(CLIENT_CLOSE_DEVICE, &[1, 2])).is_err());
        assert!(Request::decode(&wire(CLIENT_OPEN_DEVICE, &[0, 0])).is_err());
        assert!(Request::decode(&wire(4242, &[])).is_err());

        // path_len claiming more than MAX_PATH_LEN
        let mut body = (9999u16).to_ne_bytes().to_vec();
        body.extend_from_slice(b"x\0");
        assert!(Request::decode(&wire(CLIENT_OPEN_DEVICE, &body)).is_err());
    }

    /// libseat checks `header.size` against the exact size it expects, so a
    /// bodyless response must declare zero and a sized one must be exact.
    #[test]
    fn responses_declare_the_size_libseat_expects() {
        for (response, body_len) in [
            (Response::SeatClosed, 0),
            (Response::DeviceClosed, 0),
            (Response::Pong, 0),
            (Response::SessionSwitched, 0),
            (Response::SeatDisabled, 0),
            (Response::DeviceOpened { device_id: 1 }, 4),
            (Response::Error { code: 13 }, 4),
        ] {
            let buf = response.encode();
            assert_eq!(buf.len(), HEADER_LEN + body_len, "{response:?}");
            assert_eq!(u16_at(&buf, 2) as usize, body_len, "{response:?}");
        }
    }

    #[test]
    fn seat_opened_carries_nul_terminated_name() {
        let buf = Response::SeatOpened {
            seat_name: "tv".into(),
        }
        .encode();
        assert_eq!(u16_at(&buf, 0), SERVER_SEAT_OPENED);
        assert_eq!(u16_at(&buf, 2) as usize, buf.len() - HEADER_LEN);
        assert_eq!(u16_at(&buf, 4), 3); // "tv" + NUL
        assert_eq!(&buf[6..], b"tv\0");
    }

    /// The rule that cost three reboots: libseat blocks forever unless an
    /// active session is announced right after the seat is opened.
    #[test]
    fn opening_a_seat_announces_an_active_session() {
        let stream = open_seat_reply("tv", true);
        assert_eq!(
            opcodes(&stream),
            vec![(SERVER_SEAT_OPENED, 5), (SERVER_ENABLE_SEAT, 0)],
            "SEAT_OPENED must be followed by ENABLE_SEAT"
        );
        // The name still has to survive alongside the extra event.
        assert_eq!(&stream[6..9], b"tv\0");
    }

    /// An inactive session must not be announced as enabled, or the client
    /// would drive hardware it does not own.
    #[test]
    fn opening_a_seat_while_inactive_stays_quiet() {
        let stream = open_seat_reply("tv", false);
        assert_eq!(opcodes(&stream), vec![(SERVER_SEAT_OPENED, 5)]);
    }

    #[test]
    fn events_are_bodyless_and_server_tagged() {
        for event in [Event::DisableSeat, Event::EnableSeat] {
            let buf = event.encode();
            assert_eq!(buf.len(), HEADER_LEN);
            assert_eq!(u16_at(&buf, 2), 0);
            assert!(u16_at(&buf, 0) & SERVER_BIT != 0);
        }
    }
}
