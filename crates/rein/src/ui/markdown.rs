// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Markdown block and inline parser rendering styled, word-wrapped Ratatui lines for assistant turns

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const BG_CODE: Color = Color::Rgb(24, 26, 36);
const BG_INLINE_CODE: Color = Color::Rgb(34, 38, 52);
const FG_INLINE_CODE: Color = Color::Rgb(125, 207, 255);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnAlign {
    Left,
    Center,
    Right,
}

/// Renders Markdown `source` into styled Ratatui [`Line`]s with `● ` on the first line
/// and `  ` hanging indentation on subsequent lines.
#[must_use]
pub fn render_assistant_markdown(source: &str, max_width: u16) -> Vec<Line<'static>> {
    let inner_width = max_width.saturating_sub(2).max(20);
    let body = render_markdown_blocks(source, inner_width);
    if body.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(body.len());
    for (idx, line) in body.into_iter().enumerate() {
        let prefix = if idx == 0 {
            Span::styled(
                "● ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("  ")
        };
        let mut spans = Vec::with_capacity(line.spans.len() + 1);
        spans.push(prefix);
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    out
}

/// Renders Markdown blocks into standalone styled, word-wrapped [`Line`]s without leading bullet prefixes.
#[must_use]
pub fn render_markdown_blocks(source: &str, max_width: u16) -> Vec<Line<'static>> {
    let raw_lines: Vec<&str> = source.lines().collect();
    let width = usize::from(max_width.max(20));
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < raw_lines.len() {
        let line = raw_lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if !out.is_empty() && !out.last().is_some_and(|l: &Line<'_>| l.spans.is_empty()) {
                out.push(Line::default());
            }
            i += 1;
            continue;
        }

        if let Some((fence_ch, fence_len, lang)) = parse_fence_open(trimmed) {
            i = consume_code_block(&raw_lines, i + 1, fence_ch, fence_len, lang, &mut out);
            continue;
        }

        if is_table_start(&raw_lines, i) {
            i = consume_table(&raw_lines, i, max_width, &mut out);
            continue;
        }

        if is_horizontal_rule(trimmed) {
            let rule_len = usize::from(max_width.clamp(12, 48));
            out.push(Line::from(Span::styled(
                "─".repeat(rule_len),
                Style::default().fg(Color::DarkGray),
            )));
            i += 1;
            continue;
        }

        if let Some(heading_lines) = render_heading_lines(trimmed, width) {
            out.extend(heading_lines);
            i += 1;
            continue;
        }

        if trimmed.starts_with('>') {
            out.extend(render_blockquote_line(trimmed, width));
            i += 1;
            continue;
        }

        if let Some(list_lines) = render_list_lines(line, width) {
            out.extend(list_lines);
            i += 1;
            continue;
        }

        let spans = parse_inline_spans(trimmed, Style::default());
        out.extend(wrap_styled_spans(&spans, width, &[]));
        i += 1;
    }

    while out.last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

fn parse_fence_open(trimmed: &str) -> Option<(char, usize, &str)> {
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|c| *c == first).count();
    if count < 3 {
        return None;
    }
    let rest = trimmed[count..].trim();
    if first == '`' && rest.contains('`') {
        return None;
    }
    Some((first, count, rest))
}

fn is_fence_close(trimmed: &str, fence_ch: char, min_len: usize) -> bool {
    let count = trimmed.chars().take_while(|c| *c == fence_ch).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn consume_code_block(
    lines: &[&str],
    mut idx: usize,
    fence_ch: char,
    fence_len: usize,
    lang: &str,
    out: &mut Vec<Line<'static>>,
) -> usize {
    if !lang.is_empty() {
        out.push(Line::from(Span::styled(
            lang.to_owned(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    let mut state = super::highlight::HighlightState::default();
    while idx < lines.len() {
        let raw = lines[idx];
        if is_fence_close(raw.trim(), fence_ch, fence_len) {
            idx += 1;
            break;
        }
        let highlighted = super::highlight::highlight_code_line(raw, lang, &mut state);
        let mut styled_spans: Vec<Span<'static>> = highlighted
            .spans
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.bg(BG_CODE)))
            .collect();
        if styled_spans.is_empty() {
            styled_spans.push(Span::styled(" ", Style::default().bg(BG_CODE)));
        }
        out.push(Line::from(styled_spans));
        idx += 1;
    }
    idx
}

fn is_horizontal_rule(trimmed: &str) -> bool {
    let clean: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    clean.len() >= 3
        && (clean.chars().all(|c| c == '-')
            || clean.chars().all(|c| c == '*')
            || clean.chars().all(|c| c == '_')
            || clean.chars().all(|c| c == '='))
}

fn render_heading_lines(trimmed: &str, width: usize) -> Option<Vec<Line<'static>>> {
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) {
        let rest = trimmed[hashes..].trim_start();
        if !rest.is_empty() {
            let color = match hashes {
                1 => Color::Cyan,
                2 => Color::LightBlue,
                3 => Color::White,
                _ => Color::Gray,
            };
            let spans = parse_inline_spans(
                rest,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            return Some(wrap_styled_spans(&spans, width, &[]));
        }
    }
    // Treat standalone **Section Header** lines as clean bold headings.
    if let Some(inner) = trimmed
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        && !inner.is_empty()
        && !inner.contains("**")
    {
        let spans = vec![Span::styled(
            inner.to_owned(),
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )];
        return Some(wrap_styled_spans(&spans, width, &[]));
    }
    None
}

fn render_blockquote_line(trimmed: &str, width: usize) -> Vec<Line<'static>> {
    let mut depth = 0usize;
    let mut rest = trimmed;
    while let Some(after) = rest.strip_prefix('>') {
        depth += 1;
        rest = after.strip_prefix(' ').unwrap_or(after).trim_start();
    }
    let bar = "│ ".repeat(depth.min(4));
    let prefix = [Span::styled(bar, Style::default().fg(Color::DarkGray))];
    let quote_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::ITALIC);
    let spans = parse_inline_spans(rest, quote_style);
    if spans.is_empty() {
        return vec![Line::from(prefix.to_vec())];
    }
    wrap_styled_spans(&spans, width, &prefix)
}

fn render_list_lines(raw: &str, width: usize) -> Option<Vec<Line<'static>>> {
    let indent_len = raw.len() - raw.trim_start().len();
    let indent_spaces = " ".repeat((indent_len / 2).min(4) * 2);
    let trimmed = raw.trim_start();

    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        if let Some((checked, task_body)) = parse_task_checkbox(rest) {
            let (glyph, glyph_color, body_style) = if checked {
                (
                    "☑ ",
                    Color::Green,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::CROSSED_OUT),
                )
            } else {
                ("☐ ", Color::Cyan, Style::default())
            };
            let first_prefix = vec![
                Span::raw(indent_spaces.clone()),
                Span::styled(glyph, Style::default().fg(glyph_color)),
            ];
            let hang = vec![Span::raw(format!("{indent_spaces}  "))];
            let spans = parse_inline_spans(task_body, body_style);
            return Some(wrap_with_first_and_hang(
                &spans,
                width,
                &first_prefix,
                &hang,
            ));
        }

        let first_prefix = vec![
            Span::raw(indent_spaces.clone()),
            Span::styled("• ", Style::default().fg(Color::Cyan)),
        ];
        let hang = vec![Span::raw(format!("{indent_spaces}  "))];
        let spans = parse_inline_spans(rest, Style::default());
        return Some(wrap_with_first_and_hang(
            &spans,
            width,
            &first_prefix,
            &hang,
        ));
    }

    let digit_count = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count > 0 {
        let after_digits = &trimmed[digit_count..];
        if let Some(rest) = after_digits
            .strip_prefix(". ")
            .or_else(|| after_digits.strip_prefix(") "))
        {
            let marker = format!("{}. ", &trimmed[..digit_count]);
            let hang_pad = " ".repeat(marker.chars().count());
            let first_prefix = vec![
                Span::raw(indent_spaces.clone()),
                Span::styled(marker, Style::default().fg(Color::Cyan)),
            ];
            let hang = vec![Span::raw(format!("{indent_spaces}{hang_pad}"))];
            let spans = parse_inline_spans(rest, Style::default());
            return Some(wrap_with_first_and_hang(
                &spans,
                width,
                &first_prefix,
                &hang,
            ));
        }
    }
    None
}

fn parse_task_checkbox(rest: &str) -> Option<(bool, &str)> {
    if let Some(body) = rest.strip_prefix("[ ] ") {
        return Some((false, body));
    }
    if let Some(body) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        return Some((true, body));
    }
    None
}

fn is_table_start(lines: &[&str], idx: usize) -> bool {
    let Some(first) = lines.get(idx).map(|s| s.trim()) else {
        return false;
    };
    let Some(second) = lines.get(idx + 1).map(|s| s.trim()) else {
        return false;
    };
    first.contains('|') && is_table_separator(second)
}

fn is_table_separator(trimmed: &str) -> bool {
    let cells = split_table_cells(trimmed);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let clean = c.trim();
            !clean.is_empty() && clean.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
        })
        && trimmed.contains('-')
}

fn parse_table_alignments(trimmed: &str) -> Vec<ColumnAlign> {
    split_table_cells(trimmed)
        .into_iter()
        .map(|cell| {
            let c = cell.trim();
            match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => ColumnAlign::Center,
                (false, true) => ColumnAlign::Right,
                _ => ColumnAlign::Left,
            }
        })
        .collect()
}

fn split_table_cells(trimmed: &str) -> Vec<String> {
    let stripped = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed.strip_prefix('|').unwrap_or(trimmed));
    stripped
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn consume_table(
    lines: &[&str],
    start: usize,
    max_width: u16,
    out: &mut Vec<Line<'static>>,
) -> usize {
    let mut raw_rows = Vec::new();
    let mut alignments = Vec::new();
    let mut idx = start;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() || !trimmed.contains('|') {
            break;
        }
        if is_table_separator(trimmed) {
            if alignments.is_empty() {
                alignments = parse_table_alignments(trimmed);
            }
        } else {
            raw_rows.push(split_table_cells(trimmed));
        }
        idx += 1;
    }
    if raw_rows.is_empty() {
        return idx;
    }

    let col_count = raw_rows.iter().map(Vec::len).max().unwrap_or(1);
    let mut widths = vec![3usize; col_count];
    for row in &raw_rows {
        for (col, cell) in row.iter().enumerate() {
            let visible_len = visible_inline_text(cell).chars().count();
            widths[col] = widths[col].max(visible_len);
        }
    }

    let budget = usize::from(max_width.max(24));
    let overhead = col_count.saturating_sub(1) * 3;
    let avail = budget.saturating_sub(overhead).max(col_count * 4);
    fit_table_column_widths(&mut widths, avail);

    for (row_idx, row) in raw_rows.iter().enumerate() {
        let base_style = if row_idx == 0 {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let wrapped_cols: Vec<Vec<Vec<Span<'static>>>> = widths
            .iter()
            .enumerate()
            .map(|(col, col_width)| {
                let cell_raw = row.get(col).map(String::as_str).unwrap_or("");
                let cell_spans = parse_inline_spans(cell_raw, base_style);
                wrap_styled_spans(&cell_spans, *col_width, &[])
                    .into_iter()
                    .map(|l| l.spans)
                    .collect()
            })
            .collect();
        let row_height = wrapped_cols.iter().map(Vec::len).max().unwrap_or(1);

        for sub_idx in 0..row_height {
            let mut spans = Vec::new();
            for (col, col_width) in widths.iter().enumerate() {
                if col > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                }
                let align = alignments.get(col).copied().unwrap_or(ColumnAlign::Left);
                let cell_spans = wrapped_cols[col].get(sub_idx).cloned().unwrap_or_default();
                let cell_len: usize = cell_spans.iter().map(|s| s.content.chars().count()).sum();
                let pad = col_width.saturating_sub(cell_len);
                push_aligned_cell(&mut spans, cell_spans, pad, align);
            }
            out.push(Line::from(spans));
        }

        if row_idx == 0 {
            let mut sep = String::new();
            for (col, col_width) in widths.iter().enumerate() {
                if col > 0 {
                    sep.push_str("─┼─");
                }
                sep.push_str(&"─".repeat(*col_width));
            }
            out.push(Line::from(Span::styled(
                sep,
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    idx
}

fn fit_table_column_widths(widths: &mut [usize], avail: usize) {
    let total: usize = widths.iter().sum();
    if total <= avail || widths.is_empty() {
        return;
    }
    // Preserve narrow columns (up to 16 chars) so labels like "Crate" or "Lines" never truncate.
    const PROTECTED_CAP: usize = 16;
    let protected_sum: usize = widths.iter().map(|w| (*w).min(PROTECTED_CAP)).sum();
    if protected_sum >= avail {
        for w in widths.iter_mut() {
            *w = ((*w * avail) / total).max(4);
        }
        return;
    }
    let remaining = avail - protected_sum;
    let excess_sum: usize = widths.iter().map(|w| w.saturating_sub(PROTECTED_CAP)).sum();
    if excess_sum == 0 {
        return;
    }
    for w in widths.iter_mut() {
        let base = (*w).min(PROTECTED_CAP);
        let excess = w.saturating_sub(PROTECTED_CAP);
        *w = base + (excess * remaining) / excess_sum;
    }
}

fn push_aligned_cell(
    out: &mut Vec<Span<'static>>,
    cell: Vec<Span<'static>>,
    pad: usize,
    align: ColumnAlign,
) {
    match align {
        ColumnAlign::Left => {
            out.extend(cell);
            if pad > 0 {
                out.push(Span::raw(" ".repeat(pad)));
            }
        }
        ColumnAlign::Right => {
            if pad > 0 {
                out.push(Span::raw(" ".repeat(pad)));
            }
            out.extend(cell);
        }
        ColumnAlign::Center => {
            let left = pad / 2;
            let right = pad - left;
            if left > 0 {
                out.push(Span::raw(" ".repeat(left)));
            }
            out.extend(cell);
            if right > 0 {
                out.push(Span::raw(" ".repeat(right)));
            }
        }
    }
}

fn visible_inline_text(raw: &str) -> String {
    parse_inline_spans(raw, Style::default())
        .into_iter()
        .map(|s| s.content.into_owned())
        .collect()
}

fn wrap_styled_spans(
    spans: &[Span<'static>],
    max_width: usize,
    prefix: &[Span<'static>],
) -> Vec<Line<'static>> {
    wrap_with_first_and_hang(spans, max_width, prefix, prefix)
}

fn wrap_with_first_and_hang(
    spans: &[Span<'static>],
    max_width: usize,
    first_prefix: &[Span<'static>],
    hang_prefix: &[Span<'static>],
) -> Vec<Line<'static>> {
    let words = split_spans_into_words(spans);
    if words.is_empty() {
        return vec![Line::from(first_prefix.to_vec())];
    }

    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = first_prefix.to_vec();
    let mut col: usize = first_prefix.iter().map(|s| s.content.chars().count()).sum();
    let hang_len: usize = hang_prefix.iter().map(|s| s.content.chars().count()).sum();

    for word in words {
        let word_len: usize = word.iter().map(|s| s.content.chars().count()).sum();
        let is_space = word
            .iter()
            .all(|s| s.content.chars().all(char::is_whitespace));

        if col > hang_len && col + word_len > max_width {
            trim_trailing_whitespace(&mut current);
            lines.push(Line::from(current));
            current = hang_prefix.to_vec();
            col = hang_len;
            if is_space {
                continue;
            }
        }
        col += word_len;
        for seg in word {
            merge_or_push_span(&mut current, seg);
        }
    }

    if !current.is_empty() {
        trim_trailing_whitespace(&mut current);
        lines.push(Line::from(current));
    }
    lines
}

fn split_spans_into_words(spans: &[Span<'static>]) -> Vec<Vec<Span<'static>>> {
    let mut words: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut in_space: Option<bool> = None;

    for span in spans {
        let mut buf = String::new();
        for ch in span.content.chars() {
            let ch_space = ch.is_whitespace();
            if let Some(prev_space) = in_space
                && prev_space != ch_space
            {
                if !buf.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut buf), span.style));
                }
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            in_space = Some(ch_space);
            buf.push(ch);
        }
        if !buf.is_empty() {
            current.push(Span::styled(buf, span.style));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn merge_or_push_span(out: &mut Vec<Span<'static>>, span: Span<'static>) {
    if span.content.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.style == span.style
    {
        last.content.to_mut().push_str(&span.content);
        return;
    }
    out.push(span);
}

fn trim_trailing_whitespace(spans: &mut Vec<Span<'static>>) {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.content.trim_end_matches(' ');
        if trimmed.is_empty() {
            spans.pop();
        } else {
            let owned = trimmed.to_owned();
            last.content = owned.into();
            break;
        }
    }
}

/// Parses inline Markdown spans (`**bold**`, `__bold__`, `*italic*`, `_italic_`, `` `code` ``, `~~strike~~`, `[text](url)`, and `\*` escapes).
#[must_use]
pub fn parse_inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'\\'
            && let Some(next_ch) = text[i + 1..].chars().next()
            && is_escapable(next_ch)
        {
            plain.push(next_ch);
            i += 1 + next_ch.len_utf8();
            continue;
        }

        if bytes[i] == b'`'
            && let Some((code, consumed)) = parse_backtick_code(&text[i..])
        {
            flush_plain(&mut plain, base, &mut spans);
            spans.push(Span::styled(
                code.to_owned(),
                base.fg(FG_INLINE_CODE).bg(BG_INLINE_CODE),
            ));
            i += consumed;
            continue;
        }

        if text[i..].starts_with("**")
            && let Some(end) = find_closing(&text[i + 2..], "**")
        {
            flush_plain(&mut plain, base, &mut spans);
            let inner = &text[i + 2..i + 2 + end];
            spans.extend(parse_inline_spans(inner, base.add_modifier(Modifier::BOLD)));
            i += 2 + end + 2;
            continue;
        }

        if text[i..].starts_with("__")
            && let Some(end) = find_closing(&text[i + 2..], "__")
        {
            flush_plain(&mut plain, base, &mut spans);
            let inner = &text[i + 2..i + 2 + end];
            spans.extend(parse_inline_spans(inner, base.add_modifier(Modifier::BOLD)));
            i += 2 + end + 2;
            continue;
        }

        if text[i..].starts_with("~~")
            && let Some(end) = find_closing(&text[i + 2..], "~~")
        {
            flush_plain(&mut plain, base, &mut spans);
            let inner = &text[i + 2..i + 2 + end];
            spans.extend(parse_inline_spans(
                inner,
                base.fg(Color::DarkGray).add_modifier(Modifier::CROSSED_OUT),
            ));
            i += 2 + end + 2;
            continue;
        }

        if bytes[i] == b'['
            && let Some((label, url, consumed)) = parse_markdown_link(&text[i..])
        {
            flush_plain(&mut plain, base, &mut spans);
            spans.push(Span::styled(
                label.to_owned(),
                base.fg(Color::LightBlue).add_modifier(Modifier::UNDERLINED),
            ));
            spans.push(Span::styled(format!(" ({url})"), base.fg(Color::DarkGray)));
            i += consumed;
            continue;
        }

        if bytes[i] == b'*'
            && !text[i..].starts_with("**")
            && let Some(end) = find_single_delim_close(&text[i + 1..], b'*')
        {
            flush_plain(&mut plain, base, &mut spans);
            let inner = &text[i + 1..i + 1 + end];
            spans.extend(parse_inline_spans(
                inner,
                base.add_modifier(Modifier::ITALIC),
            ));
            i += 1 + end + 1;
            continue;
        }

        if bytes[i] == b'_'
            && !text[i..].starts_with("__")
            && is_underscore_open_boundary(text, i)
            && let Some(end) = find_underscore_close(&text[i + 1..])
        {
            flush_plain(&mut plain, base, &mut spans);
            let inner = &text[i + 1..i + 1 + end];
            spans.extend(parse_inline_spans(
                inner,
                base.add_modifier(Modifier::ITALIC),
            ));
            i += 1 + end + 1;
            continue;
        }

        let Some(ch) = text[i..].chars().next() else {
            break;
        };
        plain.push(ch);
        i += ch.len_utf8();
    }

    flush_plain(&mut plain, base, &mut spans);
    spans
}

const fn is_escapable(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '~'
            | '|'
    )
}

fn parse_backtick_code(slice: &str) -> Option<(&str, usize)> {
    let ticks = slice.bytes().take_while(|b| *b == b'`').count();
    if ticks == 0 {
        return None;
    }
    let delim = &slice[..ticks];
    let rest = &slice[ticks..];
    let end = rest.find(delim)?;
    if end == 0 {
        return None;
    }
    let code = rest[..end].trim();
    Some((code, ticks + end + ticks))
}

fn flush_plain(buf: &mut String, base: Style, out: &mut Vec<Span<'static>>) {
    if !buf.is_empty() {
        out.push(Span::styled(std::mem::take(buf), base));
    }
}

fn find_closing(slice: &str, delim: &str) -> Option<usize> {
    let pos = slice.find(delim)?;
    (pos > 0).then_some(pos)
}

fn find_single_delim_close(slice: &str, delim: u8) -> Option<usize> {
    if slice.starts_with(' ') {
        return None;
    }
    let bytes = slice.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        if *b == delim && bytes.get(idx + 1) != Some(&delim) && idx > 0 {
            return Some(idx);
        }
    }
    None
}

fn is_underscore_open_boundary(text: &str, idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    text[..idx]
        .chars()
        .next_back()
        .is_some_and(|ch| !ch.is_alphanumeric())
}

fn find_underscore_close(slice: &str) -> Option<usize> {
    if slice.starts_with(' ') {
        return None;
    }
    let bytes = slice.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'_' && bytes.get(idx + 1) != Some(&b'_') && idx > 0 {
            let after_ok = bytes
                .get(idx + 1)
                .is_none_or(|next| !next.is_ascii_alphanumeric());
            if after_ok {
                return Some(idx);
            }
        }
    }
    None
}

fn parse_markdown_link(slice: &str) -> Option<(&str, &str, usize)> {
    let close_bracket = slice.find("](")?;
    let label = &slice[1..close_bracket];
    let after = &slice[close_bracket + 2..];
    let close_paren = after.find(')')?;
    let url = &after[..close_paren];
    if label.is_empty() || url.is_empty() {
        return None;
    }
    Some((label, url, close_bracket + 2 + close_paren + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headings_inline_code_tables_and_code_blocks_without_raw_markdown() {
        let md = "\
## Workspace

Here is **Rein**, with `rein-core` inside.

| Crate | Role |
|---|---|
| `rein-core` | Turn loop |
| `rein-tools` | Tools |

```rust
// demo
let x = 1;
```";

        let lines = render_assistant_markdown(md, 90);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let joined = rendered.join("\n");

        assert!(joined.starts_with("● Workspace"));
        assert!(!joined.contains("**Rein**"));
        assert!(!joined.contains("`rein-core`"));
        assert!(!joined.contains("|---|"));
        assert!(joined.contains("Crate"));
        assert!(joined.contains("─┼─"));
        assert!(!joined.contains("╭─"));
        assert!(!joined.contains("│ let x"));
        assert!(!joined.contains("╰"));
        assert!(rendered.iter().any(|line| line == "  let x = 1;"));
        assert!(rendered.iter().any(|line| line == "  rust"));
        assert!(!joined.contains("```rust"));
        let code_line = lines.iter().find(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .contains("let x = 1;")
        });
        assert!(code_line.is_some_and(|l| {
            l.spans.iter().any(|s| {
                s.content == "let"
                    && s.style.fg == Some(Color::Magenta)
                    && s.style.bg == Some(BG_CODE)
            })
        }));
    }

    #[test]
    fn renders_links_lists_blockquotes_and_clamps_rules_to_narrow_width() {
        let md = "\
> Important security note

---

- First item with [docs](https://example.com)
1. Ordered step";
        let lines = render_assistant_markdown(md, 40);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let joined = rendered.join("\n");

        assert!(joined.contains("│ Important security note"));
        assert!(joined.contains("• First item with docs"));
        assert!(joined.contains("(https://example.com)"));
        assert!(!joined.contains("[docs]("));
        assert!(joined.contains("1. Ordered step"));
        let rule_line = rendered
            .iter()
            .find(|l| l.contains('─'))
            .expect("horizontal rule rendered");
        assert!(rule_line.chars().count() <= 40);
        let link_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content == "docs"))
            .expect("link line rendered");
        assert!(link_line.spans.iter().any(|s| {
            s.content == "docs"
                && s.style.fg == Some(Color::LightBlue)
                && s.style.add_modifier.contains(Modifier::UNDERLINED)
        }));
    }

    #[test]
    fn wraps_lists_with_hanging_indent_and_renders_task_checkboxes() {
        let md = "\
- [ ] Unchecked item that should wrap cleanly onto a second line
- [x] Completed item";
        let lines = render_markdown_blocks(md, 34);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(rendered[0].starts_with("☐ Unchecked"));
        assert!(rendered[1].starts_with("  "));
        assert!(rendered.iter().any(|l| l.starts_with("☑ Completed")));
    }

    #[test]
    fn preserves_snake_case_and_renders_escapes() {
        let md = "\
Use `snake_case_ident` or _emphasized_ and \\*escaped\\* and ``inline `tick` code``.";
        let lines = render_markdown_blocks(md, 80);
        let joined: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(joined.contains("snake_case_ident"));
        assert!(joined.contains("*escaped*"));
        assert!(joined.contains("inline `tick` code"));
        let emph_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content == "emphasized"))
            .expect("emphasized span");
        assert!(
            emph_line
                .spans
                .iter()
                .any(|s| s.content == "emphasized"
                    && s.style.add_modifier.contains(Modifier::ITALIC))
        );
    }

    #[test]
    fn aligns_and_clamps_table_columns() {
        let md = "\
| Name | Count |
| :--- | ---: |
| alpha | 42 |";
        let lines = render_markdown_blocks(md, 60);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rendered[2].contains("42"));
        assert!(rendered[2].ends_with("   42"));
    }

    #[test]
    fn wraps_wide_table_cells_without_truncation_or_squishing_short_columns() {
        let md = "\
| Crate | Lines | Role |
|---|---|---|
| `rein-core` | ~4.1k | Event-sourced core with a typestate loop and provider traits |";
        let lines = render_markdown_blocks(md, 48);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let joined = rendered.join("\n");

        assert!(!joined.contains('…'));
        assert!(joined.contains("rein-core"));
        assert!(joined.contains("~4.1k"));
        assert!(joined.contains("provider"));
        assert!(joined.contains("traits"));
        assert!(rendered.len() >= 4, "expected multi-line wrapped table row");
    }
}
