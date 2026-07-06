//! Thin wrapper over `tui_markdown` so the render sites in `ui.rs` share one
//! parse call. No config: `tui_markdown`'s defaults cover the requested
//! syntax (bold/italic/strikethrough/inline code/code block/links/lists).

use std::borrow::Cow;

use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use tui_markdown::StyleSheet as _;

fn link_style() -> Style {
    tui_markdown::DefaultStyleSheet.link()
}

/// `tui_markdown` always emits the literal ` ```lang `/` ``` ` fence text as
/// its own line around a code block (regardless of the `highlight-code`
/// feature — the block's content already gets a distinct style, the fence
/// markers are just noise on top of that), so drop those lines ourselves.
fn is_code_fence_line(line: &Line<'_>) -> bool {
    let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let trimmed = content.trim();
    trimmed
        .strip_prefix("```")
        .is_some_and(|lang| lang.chars().all(char::is_alphanumeric))
}

/// For a `[text](url)` link (or a `<url>` autolink, where text == url),
/// `tui_markdown` renders `text (url)` — the visible text plain, the url
/// appended afterwards in link style. We want just `text`, styled as a
/// link, with the appended url dropped. `tui_markdown` emits this as three
/// trailing spans right after the text: `" ("`, the url in link style, then
/// `")"` — detect that triple and fold it into the preceding text span.
fn collapse_link_parens<'a>(spans: &[Span<'a>]) -> Vec<Span<'a>> {
    let link_style = link_style();
    let mut out: Vec<Span<'_>> = Vec::with_capacity(spans.len());
    let mut i = 0;
    while i < spans.len() {
        let is_paren_triple = i + 2 < spans.len()
            && spans[i].content.as_ref() == " ("
            && spans[i + 1].style == link_style
            && spans[i + 2].content.as_ref() == ")";
        if is_paren_triple {
            if let Some(prev) = out.last_mut() {
                prev.style = link_style;
            } else {
                // No preceding text span (link opens the line) — fall back
                // to showing the url itself, styled.
                out.push(Span::styled(spans[i + 1].content.clone(), link_style));
            }
            i += 3;
        } else {
            out.push(spans[i].clone());
            i += 1;
        }
    }
    out
}

/// Bare URLs (no Markdown link syntax at all) aren't recognized as links by
/// `tui_markdown` (it doesn't enable the GFM autolink extension), so scan
/// plain-styled spans ourselves and style any `http(s)://` run as a link.
/// `// ponytail: only default-styled spans are scanned, so a URL inside
/// **bold**/other inline styling stays unlinkified — add if that's a real complaint.`
fn linkify_bare_urls(spans: Vec<Span<'_>>) -> Vec<Span<'_>> {
    let link_style = link_style();
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if span.style != Style::default() {
            out.push(span);
            continue;
        }
        let text = span.content.into_owned();
        let mut pos = 0;
        loop {
            let Some(rel) = text[pos..]
                .find("https://")
                .or_else(|| text[pos..].find("http://"))
            else {
                if pos < text.len() {
                    out.push(Span::raw(text[pos..].to_string()));
                }
                break;
            };
            let start = pos + rel;
            if start > pos {
                out.push(Span::raw(text[pos..start].to_string()));
            }
            let mut end = text[start..]
                .find(char::is_whitespace)
                .map_or(text.len(), |w| start + w);
            while end > start
                && matches!(
                    text.as_bytes()[end - 1],
                    b'.' | b',' | b')' | b']' | b'!' | b'?'
                )
            {
                end -= 1;
            }
            out.push(Span::styled(text[start..end].to_string(), link_style));
            pos = end;
        }
    }
    out
}

fn process_spans<'a>(spans: &[Span<'a>]) -> Vec<Span<'a>> {
    linkify_bare_urls(collapse_link_parens(spans))
}

/// `CommonMark` treats a lone `\n` as a "soft break" (rendered as a space) and
/// only a trailing-double-space or a blank line as a real line break — but a
/// user who typed Enter in the TUI (title or notes) expects that newline to
/// show up as one, with no Markdown escaping knowledge required. Force every
/// typed newline to be a hard break by giving it a two-space prefix; a line
/// of just two spaces is still blank per `CommonMark`, so `\n\n` paragraph
/// breaks are unaffected. ponytail: this also pads lines inside fenced code
/// blocks with trailing spaces — invisible in practice, not worth a
/// code-fence-aware exception.
fn hard_break_newlines(input: &str) -> String {
    input.replace('\n', "  \n")
}

/// Full multi-line render, for the detail pane. Owned (`'static`) since the
/// parse happens over a locally-built hard-break-expanded copy of `input`,
/// not `input` itself.
pub fn render(input: &str) -> Text<'static> {
    // ponytail: reparse every frame, no caching; add a per-task cache if this
    // shows up as measurable render lag.
    let lines: Vec<Line<'static>> = tui_markdown::from_str(&hard_break_newlines(input))
        .lines
        .into_iter()
        .filter(|l| !is_code_fence_line(l))
        .map(|line| {
            let owned: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|s| Span {
                    style: s.style,
                    content: Cow::Owned(s.content.into_owned()),
                })
                .collect();
            Line::from(process_spans(&owned))
        })
        .collect();
    Text::from(lines)
}

/// Truncate `spans` to at most `max_width` chars total, replacing anything
/// cut off (or already hidden, per `force_ellipsis`) with a trailing `...`
/// so a reader always knows there's more rather than seeing a silently
/// clipped or dropped title.
fn truncate_with_ellipsis(
    spans: Vec<Span<'static>>,
    max_width: usize,
    force_ellipsis: bool,
) -> Vec<Span<'static>> {
    let total_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if !force_ellipsis && total_len <= max_width {
        return spans;
    }
    let budget = max_width.saturating_sub(3); // room for "..."
    let mut out = Vec::with_capacity(spans.len() + 1);
    let mut remaining = budget;
    for span in spans {
        if remaining == 0 {
            break;
        }
        let taken: String = span.content.chars().take(remaining).collect();
        remaining -= taken.chars().count();
        if !taken.is_empty() {
            out.push(Span {
                style: span.style,
                content: Cow::Owned(taken),
            });
        }
    }
    out.push(Span::raw("..."));
    out
}

/// Inline render for a tree/list row title: only the first line (a tree row
/// or list row is one line — additional lines would break the layout), with
/// a trailing `...` whenever content is hidden, either because the title has
/// more lines or its first line alone doesn't fit in `max_width` chars.
pub fn render_inline(input: &str, max_width: usize) -> Vec<Span<'static>> {
    let mut rest = input.splitn(2, '\n');
    let first_line = rest.next().unwrap_or_default();
    let has_more_lines = rest.next().is_some();

    let mut lines = tui_markdown::from_str(first_line).lines;
    lines.retain(|l| !is_code_fence_line(l));
    let owned: Vec<Span<'static>> = lines
        .into_iter()
        .next()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|s| Span {
                    style: s.style,
                    content: Cow::Owned(s.content.into_owned()),
                })
                .collect()
        })
        .unwrap_or_default();
    let spans = process_spans(&owned);
    truncate_with_ellipsis(spans, max_width, has_more_lines)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;

    #[test]
    fn bold_renders_with_modifier() {
        let spans = render_inline("**bold**", 80);
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let spans = render_inline("just a title", 80);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "just a title");
    }

    #[test]
    fn code_block_fences_are_stripped_but_content_kept() {
        let text = render("```rust\nfn main() {}\n```");
        let lines: Vec<String> = text
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(lines.iter().all(|l: &String| !l.trim().starts_with("```")));
        assert!(lines.iter().any(|l| l.contains("fn main() {}")));
    }

    #[test]
    fn render_treats_a_single_typed_newline_as_a_real_line_break() {
        let text = render("line one\nline two");
        let lines: Vec<String> = text
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(lines, vec!["line one", "line two"]);
    }

    #[test]
    fn render_still_separates_blank_line_paragraphs() {
        let text = render("para one\n\npara two");
        let joined = text
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(joined.iter().any(|l| l == "para one"));
        assert!(joined.iter().any(|l| l == "para two"));
    }

    #[test]
    fn render_inline_shows_only_first_line_with_ellipsis_when_more_follow() {
        let spans = render_inline("line one\nline two", 80);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "line one...");
    }

    #[test]
    fn render_inline_truncates_a_too_long_single_line_with_ellipsis() {
        let spans = render_inline("this is a much too long title for the column", 10);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "this is...");
        assert_eq!(joined.chars().count(), 10);
    }

    #[test]
    fn render_inline_does_not_truncate_when_it_fits_and_has_no_more_lines() {
        let spans = render_inline("short title", 80);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "short title");
    }

    #[test]
    fn markdown_link_shows_only_text_styled_as_link() {
        let spans = render_inline("[a link](https://example.com)", 80);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "a link");
        assert!(spans.iter().any(|s| s.style == link_style()));
    }

    #[test]
    fn bare_url_is_linkified() {
        let spans = render_inline("see https://example.com now", 80);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "see https://example.com now");
        assert!(
            spans
                .iter()
                .any(|s| s.content.as_ref() == "https://example.com" && s.style == link_style())
        );
    }
}
