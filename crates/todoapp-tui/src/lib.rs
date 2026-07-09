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

use std::path::PathBuf;

use anyhow::Context as _;
use todoapp_store_turso::TursoStore;

use crate::app::AppState;
pub use crate::app::{SystemClock, UlidGen, make_svc};
use crate::clipboard::{Clipboard, SystemClipboard};
use crate::config::Config;
use crate::keymap::Keymap;

/// DB path, in the OS-standard data dir. Same resolution used by `todoapp-cli`.
#[must_use]
pub fn db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tda/tda.db")
}

/// TUI config path (columns/schedule/status/styles/keybindings, all in one
/// file), in the OS-standard config dir.
fn tui_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tda/tui.toml")
}

fn load_tui_config() -> anyhow::Result<(Config, Keymap)> {
    let user_toml = std::fs::read_to_string(tui_config_path()).ok();
    Ok((
        Config::load(user_toml.as_deref())?,
        Keymap::load(user_toml.as_deref())?,
    ))
}

/// Opens the `TursoStore` at [`db_path`], creating its parent directory if needed.
/// Shared by `todoapp-cli`, which opens the same store for its non-TUI commands.
pub async fn open_store() -> anyhow::Result<TursoStore> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create db directory")?;
    }
    let path_str = path.to_str().context("non-UTF-8 db path")?;
    TursoStore::open(path_str).await.context("open database")
}

pub async fn run() -> anyhow::Result<()> {
    let store = open_store().await?;
    let (config, keymap) = load_tui_config().context("load tui config")?;

    let clipboard: Box<dyn Clipboard> = Box::new(SystemClipboard::new());

    let mut app = AppState::new(store, keymap, config, clipboard).await?;
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &mut app).await;
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut AppState,
) -> anyhow::Result<()> {
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
        let term_width = terminal.size()?.width;
        if let Some(event) = event
            && !app.handle_event(event, term_width).await?
        {
            break;
        }
    }
    Ok(())
}
