//! System clipboard access, behind a port so tests never touch the real OS
//! clipboard (headless CI has no X11/Wayland/Windows/macOS session).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as base64_engine;

pub trait Clipboard {
    fn set_text(&mut self, text: String) -> anyhow::Result<()>;
    fn get_text(&mut self) -> anyhow::Result<String>;
}

/// Writes both to the OS clipboard (via `arboard`, when a backend is
/// available) and via an OSC 52 terminal escape sequence. OSC 52 is a
/// belt-and-suspenders fallback: most terminal emulators (and tmux, wrapped
/// in its DCS passthrough) intercept it and forward the text to the real
/// clipboard themselves, so `set_text` still works over SSH, in sandboxes, or
/// on Wayland compositors that don't implement the data-control protocol
/// (e.g. GNOME/KDE) — anywhere `arboard` alone can't reach. OSC 52 has no
/// readback, so `get_text` is `arboard`-only.
pub struct SystemClipboard {
    arboard: Option<arboard::Clipboard>,
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            arboard: arboard::Clipboard::new().ok(),
        }
    }
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

/// ponytail: no readback handling (terminals may echo an OSC 52 query
/// response, but parsing that racily off the input stream isn't worth it
/// here) — this is a fire-and-forget write, upgrade only if paste-from-OSC52
/// is ever requested.
fn write_osc52(text: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let encoded = base64_engine.encode(text.as_bytes());
    let osc = format!("\x1b]52;c;{encoded}\x07");
    // Inside tmux, escape sequences must be wrapped in a DCS passthrough
    // (with embedded Esc doubled) or tmux swallows them instead of
    // forwarding to the outer terminal.
    let payload = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    };
    let mut stdout = std::io::stdout();
    stdout.write_all(payload.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

impl Clipboard for SystemClipboard {
    fn set_text(&mut self, text: String) -> anyhow::Result<()> {
        let arboard_ok = self
            .arboard
            .as_mut()
            .is_some_and(|c| c.set_text(text.clone()).is_ok());
        let osc52_ok = write_osc52(&text).is_ok();
        if arboard_ok || osc52_ok {
            Ok(())
        } else {
            anyhow::bail!("no clipboard backend available")
        }
    }

    fn get_text(&mut self) -> anyhow::Result<String> {
        self.arboard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no clipboard backend available"))?
            .get_text()
            .map_err(Into::into)
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct FakeClipboard {
    pub text: Option<String>,
}

#[cfg(test)]
impl Clipboard for FakeClipboard {
    fn set_text(&mut self, text: String) -> anyhow::Result<()> {
        self.text = Some(text);
        Ok(())
    }
    fn get_text(&mut self) -> anyhow::Result<String> {
        self.text
            .clone()
            .ok_or_else(|| anyhow::anyhow!("clipboard empty"))
    }
}
