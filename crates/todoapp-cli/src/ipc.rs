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
