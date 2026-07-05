//! tda TUI (spec §10 M4): open the Turso store, run the event loop. Called from todoapp-cli's `tui` subcommand.

mod app;
mod config;
mod human_duration;
mod keymap;
mod schedule;
mod ui;

use std::path::PathBuf;

use anyhow::Context as _;
use todoapp_store_turso::TursoStore;

use crate::app::AppState;
use crate::config::Config;
use crate::keymap::Keymap;

fn db_path() -> PathBuf {
    std::env::var("TDA_DB").map_or_else(
        |_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("tda/tda.db")
        },
        PathBuf::from,
    )
}

fn keymap_path() -> PathBuf {
    std::env::var("TDA_KEYMAP").map_or_else(
        |_| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("tda/keybindings.toml")
        },
        PathBuf::from,
    )
}

fn load_keymap() -> anyhow::Result<Keymap> {
    let user_toml = std::fs::read_to_string(keymap_path()).ok();
    Keymap::load(user_toml.as_deref())
}

fn config_path() -> PathBuf {
    std::env::var("TDA_CONFIG").map_or_else(
        |_| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("tda/config.toml")
        },
        PathBuf::from,
    )
}

fn load_config() -> anyhow::Result<Config> {
    let user_toml = std::fs::read_to_string(config_path()).ok();
    Config::load(user_toml.as_deref())
}

pub async fn run() -> anyhow::Result<()> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create db directory")?;
    }
    let path_str = path.to_str().context("non-UTF-8 db path")?;
    let store = TursoStore::open(path_str).await.context("open database")?;
    let keymap = load_keymap().context("load keybindings")?;
    let config = load_config().context("load config")?;

    let mut app = AppState::new(store, keymap, config).await?;
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
        if let Some(event) = event
            && !app.handle_event(event).await?
        {
            break;
        }
    }
    Ok(())
}
