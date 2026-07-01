//! tda TUI (spec §10 M4): open the Turso store, run the event loop. Called from tda-cli's `tui` subcommand.

mod app;
mod keymap;
mod ui;

use std::path::PathBuf;

use anyhow::Context as _;
use tda_store_turso::TursoStore;

use crate::app::AppState;
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

pub async fn run() -> anyhow::Result<()> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create db directory")?;
    }
    let path_str = path.to_str().context("non-UTF-8 db path")?;
    let store = TursoStore::open(path_str).await.context("open database")?;
    let keymap = load_keymap().context("load keybindings")?;

    let mut app = AppState::new(store, keymap).await?;
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
        // for input; crossterm::event::read is a free fn so it's 'static + Send.
        let event = tokio::task::spawn_blocking(crossterm::event::read)
            .await
            .context("event thread")??;
        if !app.handle_event(event).await? {
            break;
        }
    }
    Ok(())
}
