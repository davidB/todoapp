//! IPC between `tda` invocations and a running `tda tui` server.
//!
//! While the TUI is up it holds the (exclusively-locked) database, so other
//! `tda` processes can't open it — they send their command here instead and the
//! TUI runs it in-process. This module owns the wire types; the Unix-socket
//! transport (added alongside the server/client that use it) is `#[cfg(unix)]`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::command::Cmd;

/// A command to execute, plus the caller's context the server can't infer:
/// `cwd` (for `--here`, `ws`, and relative file args) and `stdin` (for
/// `add --batch`).
#[derive(Serialize, Deserialize)]
pub struct Request {
    pub cmd: Cmd,
    pub cwd: PathBuf,
    pub stdin: Vec<u8>,
}

/// The captured result of a command: `out` goes to stdout, `err` (if any) to
/// stderr with a non-zero exit.
#[derive(Serialize, Deserialize)]
pub struct Reply {
    pub out: Vec<u8>,
    pub err: Option<String>,
}

// Client side (`send`, `sock_path_for`) is always available so a headless
// `tda` can reach a running TUI. The server side (`bind`/`try_accept`/`reply`)
// only exists in `tui` builds.
#[cfg(all(unix, feature = "tui"))]
pub use unix::{bind, reply, try_accept};
#[cfg(unix)]
pub use unix::{send, sock_path_for};

/// Length-prefixed JSON framing (`u32` LE length + body). One frame per
/// direction, one exchange per connection.
#[cfg(unix)]
mod frame {
    use std::io::{Read, Write};

    use anyhow::Context as _;
    use serde::Serialize;
    use serde::de::DeserializeOwned;

    pub(super) fn write<W: Write, T: Serialize>(mut w: W, msg: &T) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(msg)?;
        let len = u32::try_from(bytes.len()).context("ipc frame too large")?;
        w.write_all(&len.to_le_bytes())?;
        w.write_all(&bytes)?;
        w.flush()?;
        Ok(())
    }

    pub(super) fn read<R: Read, T: DeserializeOwned>(mut r: R) -> anyhow::Result<T> {
        let mut len = [0u8; 4];
        r.read_exact(&mut len)?;
        let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
        r.read_exact(&mut buf)?;
        Ok(serde_json::from_slice(&buf)?)
    }
}

#[cfg(unix)]
mod unix {
    use std::io::ErrorKind;
    #[cfg(feature = "tui")]
    use std::os::unix::net::UnixListener;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    #[cfg(feature = "tui")]
    use std::time::Duration;

    use anyhow::Context as _;

    #[cfg(feature = "tui")]
    use super::Reply;
    use super::{Request, frame};

    /// The IPC socket sits next to the db file, e.g. `.tda/tda.sock`.
    #[must_use]
    pub fn sock_path_for(db: &Path) -> PathBuf {
        db.with_file_name("tda.sock")
    }

    /// Send a command to a running TUI server and wait for its reply.
    ///
    /// `Ok(None)` means no server is listening (socket missing or refused) — the
    /// caller should run the command directly. `Ok(Some(_))` is the server's
    /// reply. `Err` means the server was reachable but the exchange failed; the
    /// caller must NOT fall back to the db (the server holds its lock).
    pub fn send(sock: &Path, req: &Request) -> anyhow::Result<Option<super::Reply>> {
        let mut stream = match UnixStream::connect(sock) {
            Ok(s) => s,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                return Ok(None);
            }
            Err(e) => return Err(e).context("connect to tda server"),
        };
        frame::write(&mut stream, req).context("send request to tda server")?;
        let reply = frame::read(&mut stream).context("read reply from tda server")?;
        Ok(Some(reply))
    }

    /// Bind the server socket, clearing any stale file left by a crashed
    /// server. Non-blocking so the TUI can poll it between terminal events.
    #[cfg(feature = "tui")]
    pub fn bind(sock: &Path) -> anyhow::Result<UnixListener> {
        let _ = std::fs::remove_file(sock);
        let listener =
            UnixListener::bind(sock).with_context(|| format!("bind {}", sock.display()))?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    /// Accept one pending connection and read its request, if any is waiting.
    /// Returns `None` when nothing is pending, or when a client misbehaves — a
    /// slow or malformed client is dropped rather than stalling the TUI.
    // ponytail: reads the request frame on the UI thread with a short timeout;
    // fine for a local single-user socket. Move to a task if it ever matters.
    #[cfg(feature = "tui")]
    pub fn try_accept(listener: &UnixListener) -> Option<(UnixStream, Request)> {
        let (mut stream, _) = listener.accept().ok()?;
        stream.set_nonblocking(false).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        let req = frame::read(&mut stream).ok()?;
        Some((stream, req))
    }

    /// Send the reply back to a client obtained from [`try_accept`].
    #[cfg(feature = "tui")]
    pub fn reply(mut stream: UnixStream, reply: &Reply) -> anyhow::Result<()> {
        frame::write(&mut stream, reply)
    }
}

#[cfg(all(test, unix, feature = "tui"))]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use super::{Reply, Request, bind, frame, reply, try_accept};
    use crate::command::Cmd;

    /// Drive the non-blocking listener until a client connects or we give up.
    fn accept_within(listener: &std::os::unix::net::UnixListener) -> Option<(UnixStream, Request)> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(pair) = try_accept(listener) {
                return Some(pair);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn round_trips_a_request_and_reply() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("tda.sock");
        let listener = bind(&sock).unwrap();

        let sock2 = sock.clone();
        let client = std::thread::spawn(move || {
            let mut stream = UnixStream::connect(&sock2).unwrap();
            let req = Request {
                cmd: Cmd::Ls {
                    id: None,
                    tree: false,
                },
                cwd: "/tmp".into(),
                stdin: vec![],
            };
            frame::write(&mut stream, &req).unwrap();
            let reply: Reply = frame::read(&mut stream).unwrap();
            reply
        });

        let (stream, got) = accept_within(&listener).expect("no client connected");
        assert!(matches!(got.cmd, Cmd::Ls { .. }));
        assert_eq!(got.cwd, std::path::Path::new("/tmp"));
        reply(
            stream,
            &Reply {
                out: b"hello".to_vec(),
                err: None,
            },
        )
        .unwrap();

        let got_reply = client.join().unwrap();
        assert_eq!(got_reply.out, b"hello");
        assert_eq!(got_reply.err, None);
    }

    #[test]
    fn drops_a_client_that_sends_no_frame() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("tda.sock");
        let listener = bind(&sock).unwrap();

        // Connect and immediately hang up without sending a request frame.
        let sock2 = sock.clone();
        std::thread::spawn(move || {
            let _ = UnixStream::connect(&sock2).unwrap();
        })
        .join()
        .unwrap();

        // The half-open/closed connection must be dropped, not surfaced as a
        // command (read of the length prefix hits EOF).
        assert!(try_accept(&listener).is_none());
    }
}
