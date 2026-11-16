// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Markdown block and inline parser rendering styled Ratatui lines for assistant turns

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Renders Markdown `source` into styled Ratatui [`Line`]s with `● ` on the first line
/// and `  ` hanging indentation on subsequent lines, inspired by Claude Code's terminal UI.
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

/// Renders Markdown blocks into standalone styled [`Line`]s without leading bullet prefixes.
#[must_use]
pub fn render_markdown_blocks(source: &str, max_width: u16) -> Vec<Line<'static>> {
    let raw_lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < raw_lines.len() {
        let line = raw_lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if !out.is_empty() {
                out.push(Line::default());
            }
            i += 1;
            continue;
        }

        if let Some(lang) = parse_fence_open(trimmed) {
            i = consume_code_block(&raw_lines, i + 1, lang, &mut out);
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

        if let Some(heading) = render_heading_line(trimmed) {
            out.push(heading);
            i += 1;
            continue;
        }

        if let Some(quote) = trimmed.strip_prefix('>') {
            let quote_body = quote.strip_prefix(' ').unwrap_or(quote);
            let mut spans = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
            spans.extend(parse_inline_spans(
                quote_body,
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
            ));
            out.push(Line::from(spans));
            i += 1;
            continue;
        }

        if let Some(list_line) = render_list_line(line) {
            out.push(list_line);
            i += 1;
            continue;
        }

        out.push(Line::from(parse_inline_spans(trimmed, Style::default())));
        i += 1;
    }

    while out.last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

fn parse_fence_open(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("```")?;
    Some(rest.trim())
}

fn consume_code_block(
    lines: &[&str],
    mut idx: usize,
    lang: &str,
    out: &mut Vec<Line<'static>>,
) -> usize {
    let mut state = super::highlight::HighlightState::default();
    while idx < lines.len() {
        let raw = lines[idx];
        if raw.trim().starts_with("```") {
            idx += 1;
            break;
        }
        out.push(super::highlight::highlight_code_line(raw, lang, &mut state));
        idx += 1;
    }
    idx
}

fn is_horizontal_rule(trimmed: &str) -> bool {
    let clean: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    clean.len() >= 3
        && (clean.chars().all(|c| c == '-')
            || clean.chars().all(|c| c == '*')
            || clean.chars().all(|c| c == '_'))
}

fn render_heading_line(trimmed: &str) -> Option<Line<'static>> {
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) {
        let rest = trimmed[hashes..].trim_start();
        if !rest.is_empty() {
            let color = match hashes {
                1 => Color::Cyan,
                2 => Color::LightBlue,
                _ => Color::White,
            };
            return Some(Line::from(parse_inline_spans(
                rest,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
        }
    }
    // Treat standalone **Section Header** lines (common in Claude/Gemini output) as clean bold headings.
    if let Some(inner) = trimmed
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        && !inner.is_empty()
        && !inner.contains("**")
    {
        return Some(Line::from(Span::styled(
            inner.to_owned(),
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )));
    }
    None
}

fn render_list_line(raw: &str) -> Option<Line<'static>> {
    let indent_len = raw.len() - raw.trim_start().len();
    let indent_spaces = " ".repeat((indent_len / 2).min(4) * 2);
    let trimmed = raw.trim_start();

    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        let mut spans = vec![
            Span::raw(indent_spaces),
            Span::styled("• ", Style::default().fg(Color::Cyan)),
        ];
        spans.extend(parse_inline_spans(rest, Style::default()));
        return Some(Line::from(spans));
    }

    let digit_count = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count > 0 {
        let after_digits = &trimmed[digit_count..];
        if let Some(rest) = after_digits
            .strip_prefix(". ")
            .or_else(|| after_digits.strip_prefix(") "))
        {
            let marker = format!("{}. ", &trimmed[..digit_count]);
            let mut spans = vec![
                Span::raw(indent_spaces),
                Span::styled(marker, Style::default().fg(Color::Cyan)),
            ];
            spans.extend(parse_inline_spans(rest, Style::default()));
            return Some(Line::from(spans));
        }
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
    let mut idx = start;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() || !trimmed.contains('|') {
            break;
        }
        if !is_table_separator(trimmed) {
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

    // Clamp total table width gracefully if very narrow terminal.
    let budget = usize::from(max_width.max(24));
    let overhead = col_count.saturating_sub(1) * 3;
    let sum_widths: usize = widths.iter().sum();
    if sum_widths + overhead > budget && sum_widths > 0 {
        let avail = budget.saturating_sub(overhead).max(col_count * 4);
        for w in &mut widths {
            *w = ((*w * avail) / sum_widths).max(4);
        }
    }

    for (row_idx, row) in raw_rows.iter().enumerate() {
        let base_style = if row_idx == 0 {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let mut spans = Vec::new();
        for (col, col_width) in widths.iter().enumerate() {
            if col > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            }
            let cell_raw = row.get(col).map(String::as_str).unwrap_or("");
            let cell_spans = parse_inline_spans(cell_raw, base_style);
            let cell_len = visible_inline_text(cell_raw).chars().count();
            spans.extend(cell_spans);
            if *col_width > cell_len {
                spans.push(Span::raw(" ".repeat(*col_width - cell_len)));
            }
        }
        out.push(Line::from(spans));

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

fn visible_inline_text(raw: &str) -> String {
    parse_inline_spans(raw, Style::default())
        .into_iter()
        .map(|s| s.content.into_owned())
        .collect()
}

/// Parses inline Markdown spans (`**bold**`, `*italic*`, `` `code` ``, `~~strike~~`, `[text](url)`).
#[must_use]
pub fn parse_inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'`'
            && let Some(end) = find_closing(&text[i + 1..], "`")
        {
            flush_plain(&mut plain, base, &mut spans);
            let code = &text[i + 1..i + 1 + end];
            spans.push(Span::styled(
                code.to_owned(),
                base.fg(Color::Rgb(125, 207, 255)),
            ));
            i += 1 + end + 1;
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
            && let Some(end) = find_single_asterisk_close(&text[i + 1..])
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

fn flush_plain(buf: &mut String, base: Style, out: &mut Vec<Span<'static>>) {
    if !buf.is_empty() {
        out.push(Span::styled(std::mem::take(buf), base));
    }
}

fn find_closing(slice: &str, delim: &str) -> Option<usize> {
    let pos = slice.find(delim)?;
    (pos > 0).then_some(pos)
}

fn find_single_asterisk_close(slice: &str) -> Option<usize> {
    if slice.starts_with(' ') {
        return None;
    }
    let bytes = slice.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'*' && bytes.get(idx + 1) != Some(&b'*') && idx > 0 {
            return Some(idx);
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
        assert!(!joined.contains("```rust"));
        let code_line = lines.iter().find(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .contains("let x = 1;")
        });
        assert!(code_line.is_some_and(|l| {
            l.spans
                .iter()
                .any(|s| s.content == "let" && s.style.fg == Some(Color::Magenta))
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
        assert!(joined.contains("• First item with docs (https://example.com)"));
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
}
