//! DB path resolution and `tda` config-file helpers, shared by `todoapp-cli`
//! and `todoapp-tui` so neither depends on the other for this.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use todoapp_store_turso::TursoStore;

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

/// `tda` cross-app config path (currently just `[workspaces]`; shared by
/// `todoapp-cli` and `todoapp-tui`), in the OS-standard config dir. TUI-only
/// settings (columns/schedule/status/styles/keybindings) live in a separate
/// `tui.toml`, owned by `todoapp-tui`.
#[must_use]
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tda/config.toml")
}

/// TUI-only settings path (columns/schedule/status/styles/keybindings/
/// behavior) — separate from [`config_path`] (cross-app `[workspaces]`),
/// but colocated here so path resolution + generic TOML parsing live in one
/// place regardless of who owns the typed schema.
#[must_use]
pub fn tui_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tda/tui.toml")
}

/// Reads and parses the TOML file at `path` into a generic value, for the
/// caller to deserialize its own typed sub-tables from (`serde::Deserialize`
/// over `&toml::Value` works like over any other `Deserializer`). Returns
/// `None` if the file is missing or unreadable/unparseable.
#[must_use]
pub fn read_toml(path: &Path) -> Option<toml::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
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
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| toml::from_str::<Ws>(&s).ok())
        .unwrap_or_default()
        .workspaces
}

/// Sets (or removes, if `path` is `None`) the `[workspaces].<name>` override
/// in the config file, creating the file/table if absent. Uses `toml_edit` so
/// the rest of the file (keymap, columns, comments) round-trips untouched.
pub fn set_workspace_override(name: &str, path: Option<&str>) -> anyhow::Result<()> {
    set_override_at(&config_path(), name, path)
}

/// Writes `[columns].order` in `tui.toml`, creating file/table if absent.
/// Uses `toml_edit` so the rest of the file (keymap, status, styles, comments)
/// round-trips untouched — the interactive column editor's write-back.
pub fn set_tui_columns(order: &[&str]) -> anyhow::Result<()> {
    set_columns_at(&tui_config_path(), order)
}

fn set_columns_at(file_path: &Path, order: &[&str]) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(file_path).unwrap_or_default();
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .context("parse tui.toml")?;

    if !doc.contains_key("columns") {
        doc["columns"] = toml_edit::table();
    }
    let mut arr = toml_edit::Array::new();
    for name in order {
        arr.push(*name);
    }
    doc["columns"]["order"] = toml_edit::value(arr);

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).context("create config directory")?;
    }
    std::fs::write(file_path, doc.to_string()).context("write tui.toml")
}

fn set_override_at(file_path: &Path, name: &str, path: Option<&str>) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(file_path).unwrap_or_default();
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .context("parse config file")?;

    if !doc.contains_key("workspaces") {
        doc["workspaces"] = toml_edit::table();
    }
    let workspaces = doc["workspaces"]
        .as_table_like_mut()
        .context("[workspaces] is not a table")?;
    match path {
        Some(path) => workspaces.insert(name, toml_edit::value(path)),
        None => workspaces.remove(name),
    };

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).context("create config directory")?;
    }
    std::fs::write(file_path, doc.to_string()).context("write config file")
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

#[cfg(test)]
mod override_tests {
    use super::set_override_at;

    #[test]
    fn round_trip_preserves_other_content() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("config.toml");
        std::fs::write(
            &file,
            "# a user comment\n[columns]\nwidth = 10\n\n[workspaces]\nother = \"/kept\"\n",
        )
        .unwrap();

        set_override_at(&file, "proj", Some("/home/me/proj")).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        assert!(result.contains("# a user comment"));
        assert!(result.contains("width = 10"));
        assert!(result.contains("other = \"/kept\""));
        assert!(result.contains("proj = \"/home/me/proj\""));

        set_override_at(&file, "other", None).unwrap();
        let result = std::fs::read_to_string(&file).unwrap();
        assert!(!result.contains("other ="));
        assert!(result.contains("proj = \"/home/me/proj\""));
    }

    #[test]
    fn creates_table_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("config.toml");
        std::fs::write(&file, "[columns]\nwidth = 10\n").unwrap();

        set_override_at(&file, "proj", Some("/x")).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        assert!(result.contains("[workspaces]"));
        assert!(result.contains("proj = \"/x\""));
    }
}

#[cfg(test)]
mod columns_tests {
    use super::set_columns_at;

    #[test]
    fn round_trip_preserves_other_content() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tui.toml");
        std::fs::write(
            &file,
            "# a user comment\n[columns]\norder = [\"status\", \"id\"]\n\n[keybindings]\nquit = [\"q\"]\n",
        )
        .unwrap();

        set_columns_at(&file, &["due", "status"]).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        assert!(result.contains("# a user comment"));
        assert!(result.contains("quit = [\"q\"]"));
        assert!(result.contains(r#"order = ["due", "status"]"#), "{result}");
    }

    #[test]
    fn creates_table_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("tui.toml");
        // File exists but has no [columns] table.
        std::fs::write(&file, "[keybindings]\nquit = [\"q\"]\n").unwrap();

        set_columns_at(&file, &["status", "due"]).unwrap();

        let result = std::fs::read_to_string(&file).unwrap();
        assert!(result.contains("[columns]"));
        assert!(result.contains(r#"order = ["status", "due"]"#), "{result}");
    }
}
