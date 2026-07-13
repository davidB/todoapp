//! Special title syntax: `@name` mentions (FR-32), `#tag` tags (FR-33), and
//! `[...]` due-date/recurrence brackets (FR-34) — a family sharing one scan
//! so the title (and its code-span skipping) is only walked once. Pure text
//! scan, no I/O — belongs in core.

use crate::model::{DueSpec, Id, Recurrence};

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Result of [`extract_title_syntax`]: the cleaned title plus whatever
/// special syntax was found, each in first-seen, deduplicated order (`due`/
/// `recurrence` keep the first successfully-parsed bracket of their kind).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtractedTitle {
    pub title: String,
    pub mentions: Vec<Id>,
    pub tags: Vec<String>,
    pub due: Option<DueSpec>,
    pub recurrence: Option<Recurrence>,
}

enum BracketValue {
    Due(DueSpec),
    Recurrence(Recurrence),
}

/// Dispatch a `[...]` bracket's trimmed content: a due-date value
/// ([`DueSpec::parse`]) or a recurrence expression ([`Recurrence::parse`]) —
/// the same relaxed grammars reused by the TUI edit form and CLI `--due`/
/// `--recurrence`. `None` if nothing matches — the bracket is then left as
/// literal text by the caller.
fn parse_bracket(content: &str) -> Option<BracketValue> {
    let content = content.trim();
    if let Ok(spec) = DueSpec::parse(content) {
        return Some(BracketValue::Due(spec));
    }
    Recurrence::parse(content)
        .ok()
        .map(BracketValue::Recurrence)
}

/// Extract `@name` mentions, `#tag` tags, and `[...]` due/recurrence
/// brackets from `title` in a single pass, skipping backtick-delimited code
/// spans (CommonMark-style: a run of N backticks opens, the next run of
/// exactly N backticks closes; an unterminated run is just literal text). A
/// `@`/`#` trigger is one not preceded by a word character, followed by 1+ of
/// `[A-Za-z0-9_-]`. A `[...]` bracket is stripped only if its content parses
/// (see [`parse_bracket`]); otherwise it's left as literal text. Returns the
/// cleaned title (triggers removed, whitespace collapsed, then trimmed).
pub fn extract_title_syntax(title: &str) -> ExtractedTitle {
    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
    let mut mentions: Vec<Id> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    let mut due: Option<DueSpec> = None;
    let mut recurrence: Option<Recurrence> = None;
    let mut prev_word = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            let start = i;
            while i < chars.len() && chars[i] == '`' {
                i += 1;
            }
            let fence_len = i - start;
            // Find the matching close run of exactly `fence_len` backticks.
            let mut j = i;
            let close = loop {
                if j >= chars.len() {
                    break None;
                }
                if chars[j] == '`' {
                    let run_start = j;
                    while j < chars.len() && chars[j] == '`' {
                        j += 1;
                    }
                    if j - run_start == fence_len {
                        break Some((run_start, j));
                    }
                } else {
                    j += 1;
                }
            };
            match close {
                Some((_, close_end)) => {
                    out.extend(&chars[start..close_end]);
                    prev_word = false;
                    i = close_end;
                }
                None => {
                    // Unterminated fence: treat the backticks as literal text.
                    out.extend(&chars[start..i]);
                    prev_word = false;
                }
            }
            continue;
        }
        if c == '[' {
            let start = i;
            if let Some(rel) = chars[i + 1..].iter().position(|&x| x == ']') {
                let content_end = i + 1 + rel;
                let content: String = chars[start + 1..content_end].iter().collect();
                if let Some(value) = parse_bracket(&content) {
                    match value {
                        BracketValue::Due(d) => {
                            due.get_or_insert(d);
                        }
                        BracketValue::Recurrence(r) => {
                            recurrence.get_or_insert(r);
                        }
                    }
                    let end = content_end + 1;
                    i = if chars.get(end) == Some(&' ') {
                        end + 1
                    } else {
                        end
                    };
                    prev_word = false;
                    continue;
                }
            }
        }
        if (c == '@' || c == '#') && !prev_word {
            let start = i;
            let mut j = i + 1;
            while j < chars.len() && is_name_char(chars[j]) {
                j += 1;
            }
            if j > start + 1 {
                let name: String = chars[start + 1..j].iter().collect();
                if c == '@' {
                    let id = Id::new(name);
                    if !mentions.contains(&id) {
                        mentions.push(id);
                    }
                } else if !tags.contains(&name) {
                    tags.push(name);
                }
                // Swallow one following plain space so removing the trigger
                // doesn't leave a double space; other whitespace (e.g. a
                // multi-line title's newlines) is left untouched.
                i = if chars.get(j) == Some(&' ') { j + 1 } else { j };
                prev_word = false;
                continue;
            }
        }
        out.push(c);
        prev_word = is_word_char(c);
        i += 1;
    }
    ExtractedTitle {
        title: out.trim().to_string(),
        mentions,
        tags,
        due,
        recurrence,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rstest::rstest;

    use super::*;
    use crate::model::{RepeatCycle, Weekday};
    use crate::temporal::{Due, Time};

    #[test]
    fn plain_mention() {
        let e = extract_title_syntax("fix @alice bug");
        assert_eq!(e.title, "fix bug");
        assert_eq!(e.mentions, vec![Id::new("alice")]);
        assert!(e.tags.is_empty());
    }

    #[test]
    fn multiple_distinct_mentions() {
        let e = extract_title_syntax("@bob review @alice please, @bob");
        assert_eq!(e.title, "review please,");
        assert_eq!(e.mentions, vec![Id::new("bob"), Id::new("alice")]);
    }

    #[test]
    fn mention_inside_inline_code_untouched() {
        let e = extract_title_syntax("use `@alice` as a var name");
        assert_eq!(e.title, "use `@alice` as a var name");
        assert!(e.mentions.is_empty());
    }

    #[test]
    fn mention_inside_fenced_span_untouched() {
        let e = extract_title_syntax("```@alice``` is not a mention");
        assert_eq!(e.title, "```@alice``` is not a mention");
        assert!(e.mentions.is_empty());
    }

    #[test]
    fn no_mention() {
        let e = extract_title_syntax("just a normal title");
        assert_eq!(e.title, "just a normal title");
        assert!(e.mentions.is_empty());
    }

    #[test]
    fn bare_at_with_no_name_chars_is_not_a_match() {
        let e = extract_title_syntax("email me @ the office");
        assert_eq!(e.title, "email me @ the office");
        assert!(e.mentions.is_empty());
    }

    #[test]
    fn email_shaped_text_is_not_a_mention() {
        let e = extract_title_syntax("contact foo@bar for details");
        assert_eq!(e.title, "contact foo@bar for details");
        assert!(e.mentions.is_empty());
    }

    #[test]
    fn unterminated_fence_is_literal_and_mentions_after_it_still_parse() {
        let e = extract_title_syntax("oops ``` @alice unterminated");
        assert_eq!(e.title, "oops ``` unterminated");
        assert_eq!(e.mentions, vec![Id::new("alice")]);
    }

    #[test]
    fn plain_tag() {
        let e = extract_title_syntax("fix bug #urgent");
        assert_eq!(e.title, "fix bug");
        assert_eq!(e.tags, vec!["urgent".to_string()]);
        assert!(e.mentions.is_empty());
    }

    #[test]
    fn multiple_distinct_tags() {
        let e = extract_title_syntax("#urgent fix #bug please, #urgent");
        assert_eq!(e.title, "fix please,");
        assert_eq!(e.tags, vec!["urgent".to_string(), "bug".to_string()]);
    }

    #[test]
    fn tag_inside_inline_code_untouched() {
        let e = extract_title_syntax("use `#1` as an id");
        assert_eq!(e.title, "use `#1` as an id");
        assert!(e.tags.is_empty());
    }

    #[test]
    fn tag_inside_fenced_span_untouched() {
        let e = extract_title_syntax("```#urgent``` is not a tag");
        assert_eq!(e.title, "```#urgent``` is not a tag");
        assert!(e.tags.is_empty());
    }

    #[test]
    fn bare_hash_with_no_name_chars_is_not_a_match() {
        let e = extract_title_syntax("c# is a language");
        assert_eq!(e.title, "c# is a language");
        assert!(e.tags.is_empty());
    }

    #[test]
    fn mention_and_tag_together_single_pass() {
        let e = extract_title_syntax("fix @alice bug #urgent");
        assert_eq!(e.title, "fix bug");
        assert_eq!(e.mentions, vec![Id::new("alice")]);
        assert_eq!(e.tags, vec!["urgent".to_string()]);
    }

    #[test]
    fn bracket_absolute_date_only() {
        let e = extract_title_syntax("Ship it [2026-07-20]");
        assert_eq!(e.title, "Ship it");
        assert_eq!(
            e.due,
            Some(DueSpec::Absolute(Due::parse("2026-07-20").unwrap()))
        );
    }

    #[test]
    fn bracket_absolute_date_and_time() {
        let e = extract_title_syntax("Ship it [2026-07-20 09:00]");
        assert_eq!(
            e.title, "Ship it",
            "the date+time bracket should be stripped, not just the date-only prefix"
        );
        assert_eq!(
            e.due,
            Some(DueSpec::Absolute(Due::parse("2026-07-20 09:00").unwrap()))
        );
    }

    #[test]
    fn bracket_time_only() {
        let e = extract_title_syntax("Standup [09:00]");
        assert_eq!(e.title, "Standup");
        assert_eq!(
            e.due,
            Some(DueSpec::TimeOnly(Time::parse("09:00").unwrap()))
        );
    }

    #[test]
    fn bracket_inside_code_span_untouched() {
        let e = extract_title_syntax("use `[2026-07-20]` as an example");
        assert_eq!(e.title, "use `[2026-07-20]` as an example");
        assert!(e.due.is_none());
    }

    #[test]
    fn unparsable_bracket_left_literal() {
        let e = extract_title_syntax("see [the docs] for details");
        assert_eq!(e.title, "see [the docs] for details");
        assert!(e.due.is_none());
        assert!(e.recurrence.is_none());
    }

    #[test]
    fn first_due_bracket_wins() {
        let e = extract_title_syntax("Standup [09:00] then [10:00]");
        assert_eq!(e.title, "Standup then");
        assert_eq!(
            e.due,
            Some(DueSpec::TimeOnly(Time::parse("09:00").unwrap()))
        );
    }

    #[test]
    fn mention_tag_and_due_together_single_pass() {
        let e = extract_title_syntax("fix @alice bug #urgent [2026-07-20]");
        assert_eq!(e.title, "fix bug");
        assert_eq!(e.mentions, vec![Id::new("alice")]);
        assert_eq!(e.tags, vec!["urgent".to_string()]);
        assert_eq!(
            e.due,
            Some(DueSpec::Absolute(Due::parse("2026-07-20").unwrap()))
        );
    }

    #[rstest]
    #[case("mon", Weekday::Mon)]
    #[case("Monday", Weekday::Mon)]
    #[case("MONDAY", Weekday::Mon)]
    #[case("fri", Weekday::Fri)]
    #[case("sunday", Weekday::Sun)]
    fn weekday_bracket_spellings(#[case] spelling: &str, #[case] expected: Weekday) {
        let e = extract_title_syntax(&format!("Review [{spelling}]"));
        assert_eq!(e.title, "Review");
        assert_eq!(e.due, Some(DueSpec::Weekday(expected)));
    }

    #[test]
    fn weekday_bracket_invalid_spelling_left_literal() {
        let e = extract_title_syntax("Review [someday]");
        assert_eq!(e.title, "Review [someday]");
        assert!(e.due.is_none());
    }

    #[rstest]
    #[case("daily", RepeatCycle::Daily { every_n_days: 1 })]
    #[case("every day", RepeatCycle::Daily { every_n_days: 1 })]
    #[case("every 3 days", RepeatCycle::Daily { every_n_days: 3 })]
    #[case("weekly", RepeatCycle::Weekly { weekdays: BTreeSet::new() })]
    #[case("every week", RepeatCycle::Weekly { weekdays: BTreeSet::new() })]
    #[case(
        "every mon,wed,fri",
        RepeatCycle::Weekly { weekdays: BTreeSet::from([Weekday::Mon, Weekday::Wed, Weekday::Fri]) }
    )]
    #[case("monthly", RepeatCycle::Monthly { every_n_months: 1 })]
    #[case("every month", RepeatCycle::Monthly { every_n_months: 1 })]
    #[case("every 2 months", RepeatCycle::Monthly { every_n_months: 2 })]
    fn recurrence_bracket_expressions(#[case] expr: &str, #[case] expected_cycle: RepeatCycle) {
        let e = extract_title_syntax(&format!("Water plants [{expr}]"));
        assert_eq!(e.title, "Water plants");
        assert_eq!(
            e.recurrence,
            Some(Recurrence {
                cycle: expected_cycle,
                time: None
            })
        );
    }

    #[rstest]
    #[case("every")]
    #[case("every fortnight")]
    #[case("every mon,someday")]
    #[case("hourly")]
    fn recurrence_bracket_invalid_expressions_left_literal(#[case] expr: &str) {
        let e = extract_title_syntax(&format!("Water plants [{expr}]"));
        assert_eq!(e.title, format!("Water plants [{expr}]"));
        assert!(e.recurrence.is_none());
    }
}
