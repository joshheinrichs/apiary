//! The control socket behind `seatmux status` and `seatmux stop`.
//!
//! Separate from the seat sockets: those speak libseat's protocol to child
//! compositors and have no room for anything of our own. This one is ours, so
//! it is as small as it can be — one word in, one report out, connection closed.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SOCKET: &str = "control.sock";

/// Long enough for a local peer that has already connected, short enough that a
/// wedged client cannot stall the seats.
const READ_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Status,
    Stop,
}

impl Command {
    pub fn parse(word: &str) -> Option<Command> {
        match word {
            "status" => Some(Command::Status),
            "stop" => Some(Command::Stop),
            _ => None,
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Command::Status => "status",
            Command::Stop => "stop",
        }
    }
}

pub struct Control {
    path: PathBuf,
    listener: UnixListener,
}

impl Control {
    pub fn bind(dir: &Path) -> Result<Control> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(SOCKET);
        let _ = std::fs::remove_file(&path);
        let listener =
            UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
        listener.set_nonblocking(true)?;
        Ok(Control { path, listener })
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.listener.as_fd()
    }

    /// A connected client with the command it asked for, or nothing waiting.
    /// A client that connects and says something unrecognised is answered and
    /// dropped rather than allowed to hold the loop.
    pub fn accept(&self) -> Result<Option<(UnixStream, Command)>> {
        let mut stream = match self.listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        stream.set_read_timeout(Some(READ_TIMEOUT))?;

        let mut word = String::new();
        if stream.read_to_string(&mut word).is_err() {
            let _ = writeln!(stream, "seatmux: unreadable request");
            return Ok(None);
        }

        match Command::parse(word.trim()) {
            Some(command) => Ok(Some((stream, command))),
            None => {
                let _ = writeln!(stream, "seatmux: no such command: {}", word.trim());
                Ok(None)
            }
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Client side: ask a running seatmux for something and print what it says.
pub fn send(dir: &Path, command: Command) -> Result<String> {
    let path = dir.join(SOCKET);
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("no seatmux is running ({} does not exist)", path.display())
        }
        Err(e) => bail!("connecting to {}: {e}", path.display()),
    };

    stream.write_all(command.wire().as_bytes())?;
    // The server reads to EOF, so the write side has to close before it replies.
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_commands_parse() {
        assert_eq!(Command::parse("status"), Some(Command::Status));
        assert_eq!(Command::parse("stop"), Some(Command::Stop));
        for word in ["", "STOP", "sto", "stop now", "run"] {
            assert_eq!(Command::parse(word), None, "{word} should not parse");
        }
    }

    #[test]
    fn every_command_round_trips_through_the_wire_word() {
        for command in [Command::Status, Command::Stop] {
            assert_eq!(Command::parse(command.wire()), Some(command));
        }
    }

    /// Drives the socket the way the two subcommands do: a client asks, the
    /// server sees which command it was and answers, the client reads it back.
    #[test]
    fn a_command_reaches_the_server_and_its_reply_comes_back() {
        let dir = std::env::temp_dir().join(format!("seatmux-test-{}", std::process::id()));
        let control = Control::bind(&dir).expect("bind");

        for asked in [Command::Status, Command::Stop] {
            let client = {
                let dir = dir.clone();
                std::thread::spawn(move || send(&dir, asked))
            };

            let (mut stream, got) = loop {
                if let Some(pair) = control.accept().expect("accept") {
                    break pair;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(got, asked);
            write!(stream, "answered {}", asked.wire()).expect("reply");
            drop(stream);

            let reply = client.join().expect("client thread").expect("send");
            assert_eq!(reply, format!("answered {}", asked.wire()));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_command_is_answered_and_dropped_rather_than_accepted() {
        let dir = std::env::temp_dir().join(format!("seatmux-test-bad-{}", std::process::id()));
        let control = Control::bind(&dir).expect("bind");

        let path = dir.join(SOCKET);
        let client = std::thread::spawn(move || {
            let mut stream = UnixStream::connect(&path).expect("connect");
            stream.write_all(b"reboot").expect("write");
            stream.shutdown(std::net::Shutdown::Write).expect("shutdown");
            let mut reply = String::new();
            stream.read_to_string(&mut reply).expect("read");
            reply
        });

        // Nothing is ever yielded to the caller, but the client still gets told.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            assert!(control.accept().expect("accept").is_none());
            if client.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        let reply = client.join().expect("client thread");
        assert!(reply.contains("no such command: reboot"), "got {reply:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
