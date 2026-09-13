//! The seatd server each child talks to.
//!
//! Children reach input through libseat's `seatd` backend rather than logind,
//! because `TakeControl` is exclusive per session — seatmux holds it, so nothing
//! else in the session can. Every device a child opens passes through here,
//! which makes `OpenDevice` the single point where seat policy is enforced.

use anyhow::{Context, Result};
use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::config::Seat;
use crate::device;
use crate::proto::{Event, Request, Response};

pub struct Listener {
    pub path: PathBuf,
    pub listener: UnixListener,
}

impl Listener {
    /// Bind before any child is spawned: `SEATD_SOCK` has to exist by the time
    /// libseat connects to it.
    pub fn bind(dir: &Path, seat: &str) -> Result<Listener> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{seat}.sock"));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("binding {}", path.display()))?;
        listener.set_nonblocking(true)?;
        Ok(Listener { path, listener })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One connected compositor.
pub struct Client {
    stream: UnixStream,
    pending: Vec<u8>,
    devices: HashMap<i32, libseat::Device>,
    next_id: i32,
    /// Set once the child acknowledges a `DisableSeat`, so seatmux knows it has
    /// released its devices and logind may deactivate.
    pub disabled: bool,
}

impl Client {
    pub fn new(stream: UnixStream) -> Result<Client> {
        stream.set_nonblocking(true)?;
        Ok(Client {
            stream,
            pending: Vec::new(),
            devices: HashMap::new(),
            next_id: 1,
            disabled: false,
        })
    }

    pub fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.stream.as_fd()
    }

    /// Read whatever has arrived and answer it. Returns `false` when the peer
    /// has gone, so the caller can drop the client.
    pub fn pump(
        &mut self,
        seat: &Seat,
        libseat: &mut libseat::SeatRef,
        udev_seat: &str,
        active: bool,
    ) -> Result<bool> {
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }

        while let Some((request, used)) = Request::decode(&self.pending)? {
            self.pending.drain(..used);
            self.handle(request, seat, libseat, udev_seat, active)?;
        }
        Ok(true)
    }

    fn handle(
        &mut self,
        request: Request,
        seat: &Seat,
        libseat: &mut libseat::SeatRef,
        udev_seat: &str,
        active: bool,
    ) -> Result<()> {
        match request {
            // libseat holds a freshly opened seat disabled until the server
            // says otherwise, so an active session must be announced here.
            // Without it a compositor waits forever for its session.
            Request::OpenSeat => {
                // The udev seat, not our label: libinput enumerates devices by
                // this name, and every device on the machine is on seat0. Our
                // own split is enforced in OPEN_DEVICE, not here.
                let reply = crate::proto::open_seat_reply(udev_seat, active);
                self.stream.write_all(&reply)?;
                if active {
                    self.disabled = false;
                }
                Ok(())
            }
            Request::CloseSeat => self.send(&Response::SeatClosed),
            Request::Ping => self.send(&Response::Pong),
            Request::DisableSeat => {
                self.disabled = true;
                self.send(&Response::SeatDisabled)
            }
            // VT belongs to seatmux. A child asking to switch is refused rather
            // than silently ignored, so the failure is visible in its log.
            Request::SwitchSession { .. } => {
                self.send(&Response::Error { code: nix::libc::EPERM })
            }
            Request::CloseDevice { device_id } => match self.devices.remove(&device_id) {
                Some(device) => {
                    let _ = libseat.close_device(device);
                    self.send(&Response::DeviceClosed)
                }
                None => self.send(&Response::Error { code: nix::libc::EBADF }),
            },
            Request::OpenDevice { path } => self.open_device(&path, seat, libseat),
        }
    }

    /// The policy point. A device this seat does not own is refused with
    /// `EACCES`, which libinput treats as "not available" and skips — so the
    /// child never learns the device exists.
    fn open_device(
        &mut self,
        path: &str,
        seat: &Seat,
        libseat: &mut libseat::SeatRef,
    ) -> Result<()> {
        let node = Path::new(path);
        let identity = device::identify(node).unwrap_or_default();

        if !seat.devices.allows(&identity) {
            return self.send(&Response::Error { code: nix::libc::EACCES });
        }

        let device = match libseat.open_device(&node) {
            Ok(device) => device,
            Err(errno) => return self.send(&Response::Error { code: errno.0 }),
        };

        let device_id = self.next_id;
        self.next_id += 1;
        let fd = device.as_fd().as_raw_fd();
        self.devices.insert(device_id, device);

        // The reply body and the fd must ride the same sendmsg, or libseat
        // reads the header without its descriptor.
        self.send_with_fd(&Response::DeviceOpened { device_id }, fd)
    }

    /// `DisableSeat`/`EnableSeat` are the only messages a server may volunteer;
    /// anything else mid-exchange desynchronises libseat.
    pub fn send_event(&mut self, event: Event) -> Result<()> {
        if event == Event::EnableSeat {
            self.disabled = false;
        }
        self.stream.write_all(&event.encode())?;
        Ok(())
    }

    fn send(&mut self, response: &Response) -> Result<()> {
        self.stream.write_all(&response.encode())?;
        Ok(())
    }

    fn send_with_fd(&mut self, response: &Response, fd: std::os::fd::RawFd) -> Result<()> {
        let buf = response.encode();
        let iov = [std::io::IoSlice::new(&buf)];
        let fds = [fd];
        let cmsg = [ControlMessage::ScmRights(&fds)];
        sendmsg::<()>(self.stream.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)
            .context("passing device fd")?;
        Ok(())
    }

    /// Hand every device back to logind. Used when a child dies, so its
    /// replacement can open the same hardware.
    pub fn release_all(&mut self, libseat: &mut libseat::SeatRef) {
        for (_, device) in self.devices.drain() {
            let _ = libseat.close_device(device);
        }
    }
}
