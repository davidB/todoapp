//! `@name` title syntax (first of a planned family of special title syntax):
//! extracts assignee mentions out of a task title so `todoapp-app` can turn
//! them into `Assign` commands. Pure text scan, no I/O — belongs in core.

use crate::model::Id;

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Extract `@name` mentions from `title`, skipping backtick-delimited code
/// spans (CommonMark-style: a run of N backticks opens, the next run of
/// exactly N backticks closes; an unterminated run is just literal text). A
/// mention is `@` not preceded by a word character, followed by 1+ of
/// `[A-Za-z0-9_-]`. Returns the cleaned title (mentions removed, whitespace
/// collapsed, then trimmed) and the distinct actor ids found, in
/// first-seen order.
pub fn extract_mentions(title: &str) -> (String, Vec<Id>) {
    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
    let mut mentions: Vec<Id> = Vec::new();
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
        if c == '@' && !prev_word {
            let start = i;
            let mut j = i + 1;
            while j < chars.len() && is_name_char(chars[j]) {
                j += 1;
            }
            if j > start + 1 {
                let name: String = chars[start + 1..j].iter().collect();
                let id = Id::new(name);
                if !mentions.contains(&id) {
                    mentions.push(id);
                }
                // Swallow one following plain space so removing the mention
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
    (out.trim().to_string(), mentions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mention() {
        let (title, ids) = extract_mentions("fix @alice bug");
        assert_eq!(title, "fix bug");
        assert_eq!(ids, vec![Id::new("alice")]);
    }

    #[test]
    fn multiple_distinct_mentions() {
        let (title, ids) = extract_mentions("@bob review @alice please, @bob");
        assert_eq!(title, "review please,");
        assert_eq!(ids, vec![Id::new("bob"), Id::new("alice")]);
    }

    #[test]
    fn mention_inside_inline_code_untouched() {
        let (title, ids) = extract_mentions("use `@alice` as a var name");
        assert_eq!(title, "use `@alice` as a var name");
        assert!(ids.is_empty());
    }

    #[test]
    fn mention_inside_fenced_span_untouched() {
        let (title, ids) = extract_mentions("```@alice``` is not a mention");
        assert_eq!(title, "```@alice``` is not a mention");
        assert!(ids.is_empty());
    }

    #[test]
    fn no_mention() {
        let (title, ids) = extract_mentions("just a normal title");
        assert_eq!(title, "just a normal title");
        assert!(ids.is_empty());
    }

    #[test]
    fn bare_at_with_no_name_chars_is_not_a_match() {
        let (title, ids) = extract_mentions("email me @ the office");
        assert_eq!(title, "email me @ the office");
        assert!(ids.is_empty());
    }

    #[test]
    fn email_shaped_text_is_not_a_mention() {
        let (title, ids) = extract_mentions("contact foo@bar for details");
        assert_eq!(title, "contact foo@bar for details");
        assert!(ids.is_empty());
    }

    #[test]
    fn unterminated_fence_is_literal_and_mentions_after_it_still_parse() {
        let (title, ids) = extract_mentions("oops ``` @alice unterminated");
        assert_eq!(title, "oops ``` unterminated");
        assert_eq!(ids, vec![Id::new("alice")]);
    }
}
