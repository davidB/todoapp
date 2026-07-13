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
use todoapp_store_turso::TursoStore;

use crate::app::AppState;
pub use crate::app::{SystemClock, UlidGen, make_svc};
use crate::clipboard::{Clipboard, SystemClipboard};
use crate::config::Config;
use crate::keymap::Keymap;

/// DB path resolution, shared with `todoapp-cli`: explicit override, else the
/// nearest ancestor `.tda/tda.db` (walking up from `cwd`, like git — created
/// by `tda db init`), else the global db in the OS-standard data dir.
#[must_use]
pub fn resolve_db_path(cwd: &Path, override_: Option<PathBuf>) -> PathBuf {
    if let Some(path) = override_ {
        return path;
    }
    for dir in cwd.ancestors() {
        let marker = dir.join(".tda");
        if marker.is_dir() {
            return marker.join("tda.db");
        }
    }
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

/// `[workspaces]` table of the config file: workspace name → per-machine local
/// path override. The stored `Workspace.path` is only a default — a shared DB
/// stays portable, folder mappings are local. Missing file/table ⇒ empty.
#[must_use]
pub fn workspace_overrides() -> std::collections::BTreeMap<String, String> {
    #[derive(serde::Deserialize, Default)]
    struct Ws {
        #[serde(default)]
        workspaces: std::collections::BTreeMap<String, String>,
    }
    std::fs::read_to_string(tui_config_path())
        .ok()
        .and_then(|s| toml::from_str::<Ws>(&s).ok())
        .unwrap_or_default()
        .workspaces
}

fn load_tui_config() -> anyhow::Result<(Config, Keymap)> {
    let user_toml = std::fs::read_to_string(tui_config_path()).ok();
    Ok((
        Config::load(user_toml.as_deref())?,
        Keymap::load(user_toml.as_deref())?,
    ))
}

/// Opens the `TursoStore` at the [`resolve_db_path`] result, creating its
/// parent directory if needed. Shared by `todoapp-cli`, which opens the same
/// store for its non-TUI commands; `db` is the `--db` flag override.
pub async fn open_store(db: Option<PathBuf>) -> anyhow::Result<TursoStore> {
    let cwd = std::env::current_dir().context("current dir")?;
    let path = resolve_db_path(&cwd, db);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create db directory")?;
    }
    let path_str = path.to_str().context("non-UTF-8 db path")?;
    TursoStore::open(path_str).await.context("open database")
}

pub async fn run(db: Option<PathBuf>) -> anyhow::Result<()> {
    let store = open_store(db).await?;
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

#[cfg(test)]
mod db_path_tests {
    use super::resolve_db_path;
    use std::path::PathBuf;

    #[test]
    fn override_wins() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".tda")).unwrap();
        let explicit = PathBuf::from("/elsewhere/x.db");
        assert_eq!(
            resolve_db_path(tmp.path(), Some(explicit.clone())),
            explicit
        );
    }

    #[test]
    fn finds_ancestor_marker() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".tda")).unwrap();
        let nested = tmp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            resolve_db_path(&nested, None),
            tmp.path().join(".tda/tda.db")
        );
    }

    #[test]
    fn falls_back_to_global() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_db_path(tmp.path(), None);
        assert!(resolved.ends_with("tda/tda.db"), "{}", resolved.display());
    }
}
