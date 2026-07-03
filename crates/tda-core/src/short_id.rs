//! Git/jj-style id abbreviation: pure prefix logic shared by every adapter
//! that lets a human type or display an [`Id`] (TUI column, future CLI
//! subcommands) so both resolve prefixes the same way.

use std::collections::HashMap;

use crate::model::Id;

/// Shortest prefix of each id that's unique among `ids` — the length is
/// picked so the prefix differs from both lexical neighbours once sorted.
pub fn shortest_unique_prefixes(ids: &[Id]) -> HashMap<Id, String> {
    let mut sorted: Vec<&Id> = ids.iter().collect();
    sorted.sort();
    sorted
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let len = [i.checked_sub(1), (i + 1 < sorted.len()).then_some(i + 1)]
                .into_iter()
                .flatten()
                .map(|j| common_prefix_len(id.as_str(), sorted[j].as_str()) + 1)
                .max()
                .unwrap_or(1)
                .min(id.as_str().len());
            ((*id).clone(), id.as_str()[..len].to_string())
        })
        .collect()
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// Result of resolving a user-typed id or prefix against the full id set.
pub enum ResolvedId {
    Found(Id),
    NotFound,
    Ambiguous(Vec<Id>),
}

/// Resolve a typed id/prefix (e.g. from a CLI arg or TUI lookup) the way
/// git/jj do: an exact full-id match always wins, even if it's also a prefix
/// of other ids; otherwise the prefix must match exactly one id.
/// Case-insensitive: ids are generated lowercase, but older stores may still
/// hold uppercase ULIDs, and typing case shouldn't matter either way.
pub fn resolve_id_prefix(ids: &[Id], typed: &str) -> ResolvedId {
    let typed = typed.to_lowercase();
    if let Some(id) = ids
        .iter()
        .find(|id| id.as_str().eq_ignore_ascii_case(&typed))
    {
        return ResolvedId::Found(id.clone());
    }
    let matches: Vec<Id> = ids
        .iter()
        .filter(|id| id.as_str().to_lowercase().starts_with(&typed))
        .cloned()
        .collect();
    match matches.len() {
        0 => ResolvedId::NotFound,
        1 => ResolvedId::Found(matches.into_iter().next().unwrap_or_else(Id::root)),
        _ => ResolvedId::Ambiguous(matches),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_unique_prefixes_distinguishes_shared_start() {
        let ids = vec![Id::new("abc123"), Id::new("abc456"), Id::new("xyz789")];
        let short = shortest_unique_prefixes(&ids);
        assert_eq!(short[&Id::new("abc123")], "abc1");
        assert_eq!(short[&Id::new("abc456")], "abc4");
        assert_eq!(short[&Id::new("xyz789")], "x");
    }

    #[test]
    fn resolve_prefix_exact_match_wins_over_being_a_prefix() {
        let ids = vec![Id::new("abc"), Id::new("abcdef")];
        assert!(matches!(
            resolve_id_prefix(&ids, "abc"),
            ResolvedId::Found(id) if id == Id::new("abc")
        ));
    }

    #[test]
    fn resolve_prefix_ambiguous_when_multiple_match() {
        let ids = vec![Id::new("abc123"), Id::new("abc456")];
        assert!(matches!(
            resolve_id_prefix(&ids, "abc"),
            ResolvedId::Ambiguous(m) if m.len() == 2
        ));
    }

    #[test]
    fn resolve_prefix_not_found() {
        let ids = vec![Id::new("abc123")];
        assert!(matches!(
            resolve_id_prefix(&ids, "zzz"),
            ResolvedId::NotFound
        ));
    }

    #[test]
    fn resolve_prefix_is_case_insensitive() {
        // Ids are generated lowercase, but older stores may still hold
        // uppercase ULIDs — typing case must not matter either way.
        let ids = vec![Id::new("ABC123")];
        assert!(matches!(
            resolve_id_prefix(&ids, "abc"),
            ResolvedId::Found(id) if id == Id::new("ABC123")
        ));
        assert!(matches!(
            resolve_id_prefix(&ids, "ABC123"),
            ResolvedId::Found(id) if id == Id::new("ABC123")
        ));
    }
}
