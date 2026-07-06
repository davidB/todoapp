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

/// Full multi-line render, for the detail pane.
pub fn render(input: &str) -> Text<'_> {
    // ponytail: reparse every frame, no caching; add a per-task cache if this
    // shows up as measurable render lag.
    let mut text = tui_markdown::from_str(input);
    text.lines.retain(|l| !is_code_fence_line(l));
    for line in &mut text.lines {
        line.spans = process_spans(&line.spans);
    }
    text
}

/// Inline render for a single-line field (tree/list row title): take just
/// the first parsed line's spans, owned so callers can build `'static`
/// `Row`/`Cell`/`ListItem` values from them. Titles are single-line input, so
/// block elements (lists/code blocks) never apply here.
pub fn render_inline(input: &str) -> Vec<Span<'static>> {
    let mut lines = tui_markdown::from_str(input).lines;
    lines.retain(|l| !is_code_fence_line(l));
    lines
        .into_iter()
        .next()
        .map(|line| {
            let owned: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|s| Span {
                    style: s.style,
                    content: Cow::Owned(s.content.into_owned()),
                })
                .collect();
            process_spans(&owned)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;

    #[test]
    fn bold_renders_with_modifier() {
        let spans = render_inline("**bold**");
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let spans = render_inline("just a title");
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
    fn markdown_link_shows_only_text_styled_as_link() {
        let spans = render_inline("[a link](https://example.com)");
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "a link");
        assert!(spans.iter().any(|s| s.style == link_style()));
    }

    #[test]
    fn bare_url_is_linkified() {
        let spans = render_inline("see https://example.com now");
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "see https://example.com now");
        assert!(
            spans
                .iter()
                .any(|s| s.content.as_ref() == "https://example.com" && s.style == link_style())
        );
    }
}
