// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Inline markdown span parser and word-wrapping helpers

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::blocks::{BG_INLINE_CODE, FG_INLINE_CODE};

pub(super) fn visible_inline_text(raw: &str) -> String {
    parse_inline_spans(raw, Style::default())
        .into_iter()
        .map(|s| s.content.into_owned())
        .collect()
}

pub(super) fn wrap_styled_spans(
    spans: &[Span<'static>],
    max_width: usize,
    prefix: &[Span<'static>],
) -> Vec<Line<'static>> {
    wrap_with_first_and_hang(spans, max_width, prefix, prefix)
}

pub(super) fn wrap_with_first_and_hang(
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
