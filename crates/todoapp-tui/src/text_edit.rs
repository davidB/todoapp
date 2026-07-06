//! Word-wrap + vertical-scroll math for multi-line `tui_input::Input`
//! buffers (the add/search dialog, and the edit form's notes field).
//! `tui_input::Input` already gives us codepoint/grapheme-aware
//! insert/delete/word-jump for free; what it doesn't know about is
//! rendering a value that contains `\n` across multiple wrapped rows, so
//! that's what lives here. Column counting is char-count, not display
//! width — wide/combining glyphs may misalign a column, a known ceiling,
//! not a regression (single-line fields already have the same property
//! via `tui_input`'s own `visual_cursor`, which *is* width-aware, but
//! wrapping here only needs a consistent notion of "how many columns
//! fit", not perfect terminal-cell accounting).

use std::ops::Range;

use tui_input::{Input, InputRequest};

/// Wrap width (in chars) for the add/search dialog and the notes field: the
/// popup is `centered_rect(area, 60, ..)` (60% of terminal width) minus 2 for
/// the left/right border. Shared by `app.rs` (Up/Down needs to know the width
/// the dialog is actually rendered at) and `ui.rs` (renders at that same
/// width) so the two can't drift apart.
pub fn dialog_wrap_width(term_width: u16) -> usize {
    ((term_width as usize * 60 / 100).saturating_sub(2)).max(1)
}

fn codepoint_to_byte(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

fn byte_to_codepoint(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

/// Greedy word-wrap of one hard line (`line` must not contain `\n`) into
/// visual-row byte ranges of at most `width` chars. A word longer than
/// `width` is hard-broken. Never empty: `""` yields one empty range.
fn wrap_line(line: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    if line.is_empty() {
        return vec![Range::default()];
    }
    let mut ranges = Vec::new();
    let mut row_start = 0usize;
    let mut row_chars = 0usize;
    let mut pos = 0usize;

    for token in line.split_inclusive(char::is_whitespace) {
        let tok_chars = token.chars().count();
        let tok_start = pos;
        pos += token.len();

        if tok_chars > width {
            if row_chars > 0 {
                ranges.push(row_start..tok_start);
            }
            let mut chunk_start = tok_start;
            let mut chunk_chars = 0usize;
            for (byte_off, _) in token.char_indices() {
                if chunk_chars == width {
                    ranges.push(chunk_start..tok_start + byte_off);
                    chunk_start = tok_start + byte_off;
                    chunk_chars = 0;
                }
                chunk_chars += 1;
            }
            row_start = chunk_start;
            row_chars = chunk_chars;
            continue;
        }

        if row_chars + tok_chars > width {
            ranges.push(row_start..tok_start);
            row_start = tok_start;
            row_chars = 0;
        }
        row_chars += tok_chars;
    }
    if row_start < line.len() || ranges.is_empty() {
        ranges.push(row_start..line.len());
    }
    ranges
}

/// Word-wrap `value` (possibly multi-line) at `width` chars per row. One
/// entry per visual row, in order, as `(logical_line_idx, byte_range_within_that_line)`.
fn visual_row_ranges(value: &str, width: usize) -> Vec<(usize, Range<usize>)> {
    value
        .split('\n')
        .enumerate()
        .flat_map(|(i, line)| wrap_line(line, width).into_iter().map(move |r| (i, r)))
        .collect()
}

/// Visual rows as renderable text slices (for `ui.rs`).
pub fn visual_rows(input: &Input, width: usize) -> Vec<&str> {
    let value = input.value();
    let lines: Vec<&str> = value.split('\n').collect();
    visual_row_ranges(value, width)
        .into_iter()
        .map(|(i, r)| &lines[i][r])
        .collect()
}

/// `(row, col)` — in codepoints — of `input.cursor()` among
/// `visual_rows(input, width)`.
pub fn cursor_visual_pos(input: &Input, width: usize) -> (usize, usize) {
    let value = input.value();
    let cursor_byte = codepoint_to_byte(value, input.cursor());
    let rows = visual_row_ranges(value, width);
    let lines: Vec<&str> = value.split('\n').collect();

    let mut line_start = 0usize;
    let mut target_line = 0usize;
    let mut byte_in_line = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let line_end = line_start + line.len();
        if cursor_byte <= line_end {
            target_line = i;
            byte_in_line = cursor_byte - line_start;
            break;
        }
        line_start = line_end + 1;
    }

    for (row, (line_idx, range)) in rows.iter().enumerate() {
        if *line_idx == target_line && byte_in_line >= range.start && byte_in_line <= range.end {
            // At a shared row boundary, prefer the *next* row (cursor sits
            // "before" the next character) unless this is the line's last row.
            if byte_in_line == range.end
                && rows
                    .get(row + 1)
                    .is_some_and(|(li, r)| *li == target_line && r.start == byte_in_line)
            {
                continue;
            }
            let col = lines[target_line][range.start..byte_in_line]
                .chars()
                .count();
            return (row, col);
        }
    }
    (rows.len().saturating_sub(1), 0)
}

/// Codepoint `(start, end)` of the logical (hard-newline-delimited) line
/// containing `input`'s cursor. Used for per-line Home/End — `tui_input`'s
/// own `GoToStart`/`GoToEnd` operate on the whole buffer, which is right for
/// single-line fields but wrong once a field can hold several logical lines.
pub fn current_line_bounds(input: &Input) -> (usize, usize) {
    let value = input.value();
    let cursor_byte = codepoint_to_byte(value, input.cursor());
    let mut line_start = 0usize;
    for line in value.split('\n') {
        let line_end = line_start + line.len();
        if cursor_byte <= line_end {
            return (
                byte_to_codepoint(value, line_start),
                byte_to_codepoint(value, line_end),
            );
        }
        line_start = line_end + 1;
    }
    let len = value.chars().count();
    (len, len)
}

fn set_visual_pos(input: &mut Input, width: usize, row: usize, col: usize) {
    let value = input.value().to_string();
    let rows = visual_row_ranges(&value, width);
    let Some((line_idx, range)) = rows.get(row).cloned() else {
        return;
    };
    let lines: Vec<&str> = value.split('\n').collect();
    let line = lines[line_idx];
    let row_str = &line[range.clone()];
    let clamped_col = col.min(row_str.chars().count());
    let byte_in_line = row_str
        .char_indices()
        .nth(clamped_col)
        .map_or(range.end, |(b, _)| range.start + b);
    let line_start: usize = lines[..line_idx].iter().map(|l| l.len() + 1).sum();
    let codepoint = byte_to_codepoint(&value, line_start + byte_in_line);
    input.handle(InputRequest::SetCursor(codepoint));
}

/// Move `input`'s cursor to the visual row above, same column (clamped to
/// that row's length). No-op if the cursor is already on the first row.
pub fn move_visual_up(input: &mut Input, width: usize) {
    let (row, col) = cursor_visual_pos(input, width);
    if row == 0 {
        return;
    }
    set_visual_pos(input, width, row - 1, col);
}

/// Move `input`'s cursor to the visual row below, same column (clamped to
/// that row's length). No-op if the cursor is already on the last row.
pub fn move_visual_down(input: &mut Input, width: usize) {
    let (row, col) = cursor_visual_pos(input, width);
    let total = visual_row_ranges(input.value(), width).len();
    if row + 1 >= total {
        return;
    }
    set_visual_pos(input, width, row + 1, col);
}

/// Scroll offset that keeps `pos` visible in a `viewport`-sized window over
/// `total` positions. Recomputed fresh every render — no persisted scroll
/// state needed, keeping `ui.rs` a pure `&AppState -> Frame` function.
pub fn viewport_scroll(pos: usize, total: usize, viewport: usize) -> usize {
    if viewport == 0 || total <= viewport {
        return 0;
    }
    pos.saturating_sub(viewport - 1).min(total - viewport)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges_only(line: &str, width: usize) -> Vec<(usize, usize)> {
        wrap_line(line, width)
            .into_iter()
            .map(|r| (r.start, r.end))
            .collect()
    }

    #[test]
    fn wrap_line_empty() {
        assert_eq!(ranges_only("", 10), vec![(0, 0)]);
    }

    #[test]
    fn wrap_line_fits_one_row() {
        assert_eq!(ranges_only("hello world", 20), vec![(0, 11)]);
    }

    #[test]
    fn wrap_line_breaks_at_word_boundary() {
        let rows = ranges_only("hello world foo", 11);
        // "hello " (6) + "world " would be 12 > 11, so break after "hello ".
        assert_eq!(rows, vec![(0, 6), (6, 15)]);
    }

    #[test]
    fn wrap_line_hard_breaks_oversized_word() {
        // 20 chars, evenly divisible by width 5 -> exactly 4 chunks, no
        // trailing empty range.
        let rows = ranges_only("supercalifragilistic", 5);
        assert_eq!(rows, vec![(0, 5), (5, 10), (10, 15), (15, 20)]);

        // 21 chars (one longer) leaves a trailing partial chunk.
        let rows = ranges_only("supercalifragilistics", 5);
        assert_eq!(rows, vec![(0, 5), (5, 10), (10, 15), (15, 20), (20, 21)]);
    }

    #[test]
    fn cursor_visual_pos_multiline() {
        let input: Input = "hello world\nfoo".into();
        // "hello world" wraps at width 6 -> "hello " / "world"; second logical
        // line "foo" is its own row.
        let input = input.with_cursor(13); // 'o' of "foo" (h,e,l,l,o, ,w,o,r,l,d,\n,f,o -> cursor after 'o' char idx13? let's just assert shape)
        let (row, _col) = cursor_visual_pos(&input, 6);
        assert_eq!(row, 2);
    }

    #[test]
    fn move_visual_up_down_roundtrip() {
        let mut input: Input = "hello world\nfoo".into();
        // Row 0: "hello ", Row 1: "world", Row 2: "foo"
        input.handle(InputRequest::SetCursor(2)); // row 0, col 2 ('l' in hello)
        move_visual_down(&mut input, 6);
        let (row, col) = cursor_visual_pos(&input, 6);
        assert_eq!((row, col), (1, 2));
        move_visual_down(&mut input, 6);
        let (row, col) = cursor_visual_pos(&input, 6);
        assert_eq!((row, col), (2, 2));
        // No-op past the last row.
        move_visual_down(&mut input, 6);
        let (row, _) = cursor_visual_pos(&input, 6);
        assert_eq!(row, 2);

        move_visual_up(&mut input, 6);
        move_visual_up(&mut input, 6);
        let (row, _) = cursor_visual_pos(&input, 6);
        assert_eq!(row, 0);
        // No-op at the first row.
        move_visual_up(&mut input, 6);
        let (row, _) = cursor_visual_pos(&input, 6);
        assert_eq!(row, 0);
    }

    #[test]
    fn current_line_bounds_hard_newlines() {
        let input: Input = "foo\nbar\nbaz".into();
        let input = input.with_cursor(5); // 'a' in "bar"
        assert_eq!(current_line_bounds(&input), (4, 7));

        let input: Input = "foo\nbar\nbaz".into();
        let input = input.with_cursor(0);
        assert_eq!(current_line_bounds(&input), (0, 3));

        let input: Input = "foo\nbar\nbaz".into();
        let len = input.value().chars().count();
        let input = input.with_cursor(len);
        assert_eq!(current_line_bounds(&input), (8, 11));
    }

    #[test]
    fn viewport_scroll_fits_and_overflows() {
        assert_eq!(viewport_scroll(0, 3, 5), 0); // fits entirely, no scroll
        assert_eq!(viewport_scroll(0, 10, 5), 0); // top of an overflowing list
        assert_eq!(viewport_scroll(9, 10, 5), 5); // bottom: scroll to show the last row
        assert_eq!(viewport_scroll(4, 10, 5), 0); // still within the first page
        assert_eq!(viewport_scroll(6, 10, 5), 2); // just past the first page
    }
}
