//! tda TUI entry point (spec §10 M4): open the Turso store, run the event loop.

mod app;
mod ui;

use std::path::PathBuf;

use anyhow::Context as _;
use tda_store_turso::TursoStore;

use crate::app::AppState;

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

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create db directory")?;
    }
    let path_str = path.to_str().context("non-UTF-8 db path")?;
    let store = TursoStore::open(path_str).await.context("open database")?;

    let mut app = AppState::new(store).await?;
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app).await;
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut AppState) -> anyhow::Result<()> {
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
