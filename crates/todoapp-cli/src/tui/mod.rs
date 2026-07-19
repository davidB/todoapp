//! tda TUI (spec §10 M4): open the Turso store, run the event loop. Called from todoapp-cli's `tui` subcommand.

mod app;
mod clipboard;
mod config;
mod human_duration;
mod keymap;
mod markdown;
mod schedule;
mod text_edit;
mod ui;

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use todoapp_config::{open_store, read_toml, resolve_db_path, tui_config_path};

use self::app::AppState;
use self::clipboard::{Clipboard, SystemClipboard};
use self::config::Config;
use self::keymap::Keymap;
#[cfg(unix)]
use crate::command::run_command;
#[cfg(unix)]
use crate::ipc;
#[cfg(unix)]
use crate::svc::make_svc;

fn load_tui_config() -> anyhow::Result<(Config, Keymap)> {
    let user = read_toml(&tui_config_path());
    Ok((Config::load(user.as_ref())?, Keymap::load(user.as_ref())?))
}

pub async fn run(db: Option<PathBuf>) -> anyhow::Result<()> {
    // Resolve the db path up front so we can put the IPC socket next to it.
    let cwd = std::env::current_dir().context("current dir")?;
    let db_path = resolve_db_path(&cwd, db);
    let store = open_store(Some(db_path.clone())).await?;
    let (config, keymap) = load_tui_config().context("load tui config")?;

    let clipboard: Box<dyn Clipboard> = Box::new(SystemClipboard::new());

    let mut app = AppState::new(store, keymap, config, clipboard).await?;
    let mut terminal = ratatui::init();
    // `submit_on_enter` needs Shift+Enter distinguishable from Enter, which
    // only the keyboard-enhancement protocol provides. Push the disambiguation
    // flag on terminals that support it; a no-op elsewhere (Shift+Enter then
    // just reads as Enter and submits).
    let pushed_kbd_flags = app.config.submit_on_enter && push_keyboard_flags();
    let result = run_loop(&mut terminal, &mut app, &db_path).await;
    if pushed_kbd_flags {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
    }
    ratatui::restore();
    result
}

/// Enable Shift+Enter (and other modified keys) reporting where supported.
/// Returns whether the flags were actually pushed (so we can pop on exit).
fn push_keyboard_flags() -> bool {
    use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    if !matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) {
        return false;
    }
    crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

/// While the TUI is up it holds the exclusively-locked db, so other `tda`
/// invocations can't open it — they connect to our socket instead and we run
/// their command in-process, then `rebuild()` so the change shows immediately.
/// The socket is polled between terminal events (see [`ipc::try_accept`]); the
/// terminal poll itself is unchanged, so no keystrokes are missed.
#[cfg_attr(not(unix), allow(unused_variables))]
async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut AppState,
    db_path: &Path,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    let sock = ipc::sock_path_for(db_path);
    // If binding fails (e.g. an unwritable dir), run the TUI without a server
    // rather than refusing to start.
    #[cfg(unix)]
    let listener = ipc::bind(&sock).ok();

    let result = async {
        loop {
            terminal.draw(|f| ui::render(f, app))?;
            // spawn_blocking keeps the current_thread runtime unblocked while waiting
            // for input; poll-with-timeout (rather than a blocking read) lets the
            // loop redraw periodically even without a keypress, animating the `wip`
            // spinner. Timeout is the configured spinner interval.
            let timeout = app.config.spinner_interval;
            let event = tokio::task::spawn_blocking(
                move || -> anyhow::Result<Option<crossterm::event::Event>> {
                    if crossterm::event::poll(timeout)? {
                        Ok(Some(crossterm::event::read()?))
                    } else {
                        Ok(None)
                    }
                },
            )
            .await
            .context("event thread")??;
            app.throbber_state.calc_next();

            // Serve any commands sent by other `tda` processes since the last
            // cycle, refreshing after each so external changes appear live.
            #[cfg(unix)]
            if let Some(listener) = &listener {
                while let Some((stream, req)) = ipc::try_accept(listener) {
                    // Scope the borrow of app's fields so `rebuild` can take
                    // `&mut app` right after.
                    let reply = {
                        let svc = make_svc(&app.store, &app.clock, &app.ids);
                        run_command(&svc, &req).await
                    };
                    app.rebuild().await;
                    let _ = ipc::reply(stream, &reply);
                }
            }

            let term_width = terminal.size()?.width;
            if let Some(event) = event {
                if !app.handle_event(event, term_width).await? {
                    break;
                }
                // Keep the details pane in sync with the selection after a
                // handled keystroke (cursor move, edit, ...). Human-paced, so
                // one snapshot per keypress is negligible.
                // ponytail: refresh per-keystroke, not per-frame — skips idle
                // spinner ticks, avoiding needless DB queries.
                app.refresh_detail().await;
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    #[cfg(unix)]
    if listener.is_some() {
        let _ = std::fs::remove_file(&sock);
    }

    result
}
