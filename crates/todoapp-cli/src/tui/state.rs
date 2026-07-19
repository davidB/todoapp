//! Persisted UI state — which tasks are unfolded, the cursor task, and whether
//! the details pane is open — reloaded on the next launch when
//! `behavior.restore_state` is on (default).
//! Stored as JSON next to the db (`tui.state.json`, like the IPC socket).
//! Best-effort: any read/parse/write error just means we start (or exit) fresh.
//!
//! ponytail: only the Tree view's state is durable. List views (search /
//! what-next results) are query-derived and Help-on-launch is worse UX than
//! landing on the tree, so `view` isn't persisted — restore always lands in Tree.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use todoapp_core::Id;

#[derive(Default, Serialize, Deserialize)]
pub struct UiState {
    /// Ids of expanded (unfolded) tree tasks. Stale ids (deleted since) are
    /// harmless — they simply never match a live task on rebuild.
    #[serde(default)]
    pub expanded: std::collections::HashSet<Id>,
    /// Cursor task id, restored by finding it in the rebuilt tree.
    #[serde(default)]
    pub cursor: Option<Id>,
    /// Whether the details pane was toggled on.
    #[serde(default)]
    pub detail_shown: bool,
}

fn path_for(db: &Path) -> PathBuf {
    db.with_file_name("tui.state.json")
}

pub fn load(db: &Path) -> UiState {
    std::fs::read(path_for(db))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(db: &Path, state: &UiState) {
    if let Ok(json) = serde_json::to_vec(state) {
        let _ = std::fs::write(path_for(db), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("tda-state-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("tda.db");

        // Missing file loads as empty.
        assert!(load(&db).cursor.is_none());

        let mut state = UiState::default();
        state.expanded.insert(Id::new("a"));
        state.cursor = Some(Id::new("a"));
        state.detail_shown = true;
        save(&db, &state);

        let loaded = load(&db);
        assert!(loaded.expanded.contains(&Id::new("a")));
        assert_eq!(loaded.cursor, Some(Id::new("a")));
        assert!(loaded.detail_shown);

        std::fs::remove_dir_all(&dir).ok();
    }
}
