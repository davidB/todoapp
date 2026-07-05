//! Configurable keybindings: a TOML file maps action name -> list of key
//! chords (e.g. `move_down = ["j", "down"]`). Defaults ship embedded in the
//! binary (`keybindings.default.toml`) so retuning never requires a Rust
//! change — an optional user file at `$TDA_KEYMAP` (or
//! `~/.config/tda/keybindings.toml`) overrides individual actions.

use std::collections::HashMap;

use anyhow::{Context as _, bail};
use crossterm::event::{KeyCode, KeyModifiers};

const DEFAULT_KEYMAP_TOML: &str = include_str!("../keybindings.default.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MoveDown,
    MoveUp,
    Collapse,
    Expand,
    JumpFirst,
    JumpLast,
    AddSibling,
    AddRoot,
    EditTitle,
    CycleStatus,
    Claim,
    ReorderUp,
    ReorderDown,
    ReparentIn,
    ReparentOut,
    Search,
    WhatNext,
    ToggleHelp,
    Quit,
}

impl Action {
    /// (config name, variant, help description) — single source of truth for
    /// name<->action lookup and the help view.
    const ALL: &'static [(&'static str, Action, &'static str)] = &[
        ("move_down", Action::MoveDown, "move down"),
        ("move_up", Action::MoveUp, "move up"),
        ("collapse", Action::Collapse, "collapse / jump to parent"),
        ("expand", Action::Expand, "expand"),
        ("jump_first", Action::JumpFirst, "first item"),
        ("jump_last", Action::JumpLast, "last item"),
        ("add_sibling", Action::AddSibling, "add sibling of cursor"),
        ("add_root", Action::AddRoot, "add root task"),
        ("edit_title", Action::EditTitle, "edit task"),
        (
            "cycle_status",
            Action::CycleStatus,
            "cycle status draft→todo→wip→done",
        ),
        ("claim", Action::Claim, "claim (→ wip, single-user 'me')"),
        ("reorder_up", Action::ReorderUp, "reorder up among siblings"),
        (
            "reorder_down",
            Action::ReorderDown,
            "reorder down among siblings",
        ),
        (
            "reparent_in",
            Action::ReparentIn,
            "reparent under sibling above (indent)",
        ),
        (
            "reparent_out",
            Action::ReparentOut,
            "move to parent's level (outdent)",
        ),
        ("search", Action::Search, "text search"),
        (
            "what_next",
            Action::WhatNext,
            "what-next (status:todo by priority)",
        ),
        ("toggle_help", Action::ToggleHelp, "toggle help"),
        ("quit", Action::Quit, "quit / back"),
    ];

    fn from_name(name: &str) -> Option<Action> {
        Self::ALL
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, a, _)| *a)
    }

    pub fn description(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, a, _)| *a == self)
            .map_or("", |(_, _, d)| d)
    }

    pub fn iter() -> impl Iterator<Item = Action> {
        Self::ALL.iter().map(|(_, a, _)| *a)
    }
}

type RawKeymap = HashMap<String, Vec<String>>;

fn parse_key_chord(chord: &str) -> anyhow::Result<(KeyCode, KeyModifiers)> {
    let mut parts: Vec<&str> = chord.split('+').collect();
    let key_part = parts
        .pop()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("empty key chord {chord:?}"))?;
    let mut modifiers = KeyModifiers::NONE;
    for prefix in parts {
        match prefix.to_ascii_lowercase().as_str() {
            "ctrl" => modifiers |= KeyModifiers::CONTROL,
            "alt" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            other => bail!("unknown modifier {other:?} in key chord {chord:?}"),
        }
    }
    let named = match key_part.to_ascii_lowercase().as_str() {
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "enter" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "space" => Some(KeyCode::Char(' ')),
        "tab" => Some(KeyCode::Tab),
        "backspace" => Some(KeyCode::Backspace),
        _ => None,
    };
    let code = if let Some(code) = named {
        code
    } else {
        let mut chars = key_part.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            bail!("key chord {chord:?} must be a single character or a known key name");
        };
        KeyCode::Char(c)
    };
    Ok((code, modifiers))
}

fn format_key_chord(code: KeyCode, modifiers: KeyModifiers) -> String {
    let mut parts = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl".to_string());
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt".to_string());
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift".to_string());
    }
    parts.push(match code {
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}").to_lowercase(),
    });
    parts.join("+")
}

pub struct Keymap {
    bindings: HashMap<(KeyCode, KeyModifiers), Action>,
}

impl Keymap {
    fn build(raw: &RawKeymap) -> anyhow::Result<HashMap<(KeyCode, KeyModifiers), Action>> {
        let mut bindings = HashMap::new();
        for (name, chords) in raw {
            let action =
                Action::from_name(name).with_context(|| format!("unknown action {name:?}"))?;
            for chord in chords {
                let key = parse_key_chord(chord).with_context(|| format!("action {name:?}"))?;
                if let Some(existing) = bindings.insert(key, action) {
                    bail!(
                        "key chord {chord:?} is bound to both {existing:?} and {action:?} \
                         (each key chord may map to only one action)"
                    );
                }
            }
        }
        Ok(bindings)
    }

    /// Load defaults, then apply `user_toml` overrides (if given) on top —
    /// each named action replaces its whole key list; unmentioned actions
    /// keep their default keys.
    pub fn load(user_toml: Option<&str>) -> anyhow::Result<Self> {
        let mut raw: RawKeymap =
            toml::from_str(DEFAULT_KEYMAP_TOML).context("parse embedded default keymap")?;
        if let Some(user_toml) = user_toml {
            let overrides: RawKeymap = toml::from_str(user_toml).context("parse user keymap")?;
            for (name, chords) in overrides {
                if Action::from_name(&name).is_none() {
                    bail!("unknown keybinding action {name:?}");
                }
                raw.insert(name, chords);
            }
        }
        Ok(Self {
            bindings: Self::build(&raw)?,
        })
    }

    pub fn lookup(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        self.bindings.get(&(code, modifiers)).copied()
    }

    /// The key chords currently bound to `action`, for display (e.g. help view).
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        let mut keys: Vec<String> = self
            .bindings
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(&(code, modifiers), _)| format_key_chord(code, modifiers))
            .collect();
        keys.sort();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keymap_parses_without_conflicts() {
        let keymap = Keymap::load(None).expect("default keymap must parse");
        assert_eq!(
            keymap.lookup(KeyCode::Left, KeyModifiers::ALT),
            Some(Action::ReparentOut)
        );
        assert_eq!(
            keymap.lookup(KeyCode::Up, KeyModifiers::ALT),
            Some(Action::ReorderUp)
        );
        assert_eq!(
            keymap.lookup(KeyCode::Char('j'), KeyModifiers::NONE),
            Some(Action::MoveDown)
        );
    }

    #[test]
    fn override_replaces_only_the_named_action() {
        let user = r#"move_up = ["ctrl+p"]"#;
        let keymap = Keymap::load(Some(user)).expect("override must parse");
        assert_eq!(
            keymap.lookup(KeyCode::Char('p'), KeyModifiers::CONTROL),
            Some(Action::MoveUp)
        );
        assert_eq!(keymap.lookup(KeyCode::Char('k'), KeyModifiers::NONE), None);
        // Unmentioned action keeps its default.
        assert_eq!(
            keymap.lookup(KeyCode::Char('j'), KeyModifiers::NONE),
            Some(Action::MoveDown)
        );
    }

    #[test]
    fn colliding_chord_across_actions_is_an_error() {
        let user = r#"move_up = ["j"]"#;
        assert!(Keymap::load(Some(user)).is_err());
    }
}
