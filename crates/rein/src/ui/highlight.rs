// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Zero-dependency syntax highlighter for unboxed fenced code blocks in the TUI

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Persistent multi-line state tracked across lines within a single fenced code block.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HighlightState {
    in_block_comment: bool,
}

/// Highlights a single source line according to `lang`, preserving content as-is without box chrome.
#[must_use]
pub fn highlight_code_line(raw: &str, lang: &str, state: &mut HighlightState) -> Line<'static> {
    let norm = normalize_lang(lang);
    if norm == "diff" {
        return highlight_diff_line(raw);
    }
    let spans = tokenize_line(raw, norm, state);
    if spans.is_empty() {
        return Line::from(Span::raw(""));
    }
    Line::from(spans)
}

fn normalize_lang(lang: &str) -> &str {
    match lang.trim().to_ascii_lowercase().as_str() {
        "rs" | "rust" => "rust",
        "py" | "python" => "python",
        "js" | "jsx" | "ts" | "tsx" | "javascript" | "typescript" => "js",
        "sh" | "bash" | "zsh" | "shell" => "shell",
        "toml" | "yaml" | "yml" => "config",
        "json" => "json",
        "sql" => "sql",
        "diff" | "patch" => "diff",
        "go" | "c" | "cpp" | "cc" | "h" | "hpp" => "clike",
        _ => "generic",
    }
}

fn highlight_diff_line(raw: &str) -> Line<'static> {
    let style = if raw.starts_with("+++") || raw.starts_with("---") {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if raw.starts_with("@@") {
        Style::default().fg(Color::Magenta)
    } else if raw.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if raw.starts_with('-') {
        Style::default().fg(Color::LightRed)
    } else {
        Style::default().fg(Color::Gray)
    };
    Line::from(Span::styled(raw.to_owned(), style))
}

fn tokenize_line(raw: &str, lang: &str, state: &mut HighlightState) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut pos = 0;

    while pos < raw.len() {
        if state.in_block_comment {
            pos = consume_block_comment(raw, pos, state, &mut spans);
            continue;
        }
        if let Some(next) = try_consume_comment(raw, pos, lang, state, &mut spans) {
            pos = next;
            continue;
        }
        if let Some(next) = try_consume_string_or_lifetime(raw, pos, lang, &mut spans) {
            pos = next;
            continue;
        }
        if let Some(next) = try_consume_attr_or_number(raw, pos, lang, &mut spans) {
            pos = next;
            continue;
        }
        if let Some(next) = try_consume_word(raw, pos, lang, &mut spans) {
            pos = next;
            continue;
        }
        pos = consume_punct_or_space(raw, pos, &mut spans);
    }
    spans
}

fn push_span(spans: &mut Vec<Span<'static>>, text: String, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(&text);
        return;
    }
    spans.push(Span::styled(text, style));
}

fn comment_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC)
}

fn consume_block_comment(
    raw: &str,
    pos: usize,
    state: &mut HighlightState,
    spans: &mut Vec<Span<'static>>,
) -> usize {
    let rest = &raw[pos..];
    if let Some(end) = rest.find("*/") {
        state.in_block_comment = false;
        let next = pos + end + 2;
        push_span(spans, raw[pos..next].to_owned(), comment_style());
        next
    } else {
        push_span(spans, rest.to_owned(), comment_style());
        raw.len()
    }
}

fn try_consume_comment(
    raw: &str,
    pos: usize,
    lang: &str,
    state: &mut HighlightState,
    spans: &mut Vec<Span<'static>>,
) -> Option<usize> {
    let rest = &raw[pos..];
    if rest.starts_with("/*") && lang != "python" && lang != "shell" && lang != "config" {
        state.in_block_comment = true;
        return Some(consume_block_comment(raw, pos, state, spans));
    }
    let is_line_comment = rest.starts_with("//")
        || (lang == "sql" && rest.starts_with("--"))
        || (rest.starts_with('#') && is_hash_comment(raw, pos, rest, lang));
    if is_line_comment {
        push_span(spans, rest.to_owned(), comment_style());
        return Some(raw.len());
    }
    None
}

fn is_hash_comment(raw: &str, pos: usize, rest: &str, lang: &str) -> bool {
    if rest.starts_with("#[") || rest.starts_with("#![") {
        return false;
    }
    match lang {
        "python" | "shell" | "config" => true,
        _ => raw[..pos].trim().is_empty() && (rest.starts_with("# ") || rest == "#"),
    }
}

fn try_consume_string_or_lifetime(
    raw: &str,
    pos: usize,
    lang: &str,
    spans: &mut Vec<Span<'static>>,
) -> Option<usize> {
    let rest = &raw[pos..];
    let quote = rest.chars().next()?;
    if !matches!(quote, '"' | '\'' | '`') {
        return None;
    }
    if quote == '\'' && lang == "rust" && !is_rust_char_literal(rest) {
        let len = 1 + rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len() - 1);
        push_span(
            spans,
            raw[pos..pos + len].to_owned(),
            Style::default().fg(Color::Yellow),
        );
        return Some(pos + len);
    }
    let end = find_closing_quote(rest, quote);
    let next = pos + end;
    push_span(
        spans,
        raw[pos..next].to_owned(),
        Style::default().fg(Color::Green),
    );
    Some(next)
}

fn is_rust_char_literal(rest: &str) -> bool {
    let mut chars = rest[1..].chars();
    match chars.next() {
        Some('\\') => {
            chars.nth(1) == Some('\'') || rest[2..].starts_with("x") || rest[2..].starts_with("u{")
        }
        Some(ch) if ch != '\'' => chars.next() == Some('\''),
        _ => false,
    }
}

fn find_closing_quote(rest: &str, quote: char) -> usize {
    let mut escaped = false;
    for (idx, ch) in rest.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != '`' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return idx + ch.len_utf8();
        }
    }
    rest.len()
}

fn try_consume_attr_or_number(
    raw: &str,
    pos: usize,
    lang: &str,
    spans: &mut Vec<Span<'static>>,
) -> Option<usize> {
    let rest = &raw[pos..];
    if rest.starts_with("#[") || rest.starts_with("#![") {
        let end = rest.find(']').map_or(rest.len(), |i| i + 1);
        push_span(
            spans,
            raw[pos..pos + end].to_owned(),
            Style::default().fg(Color::Yellow),
        );
        return Some(pos + end);
    }
    if rest.starts_with('@') && matches!(lang, "python" | "js" | "generic") {
        let len = 1 + rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
            .unwrap_or(rest.len() - 1);
        push_span(
            spans,
            raw[pos..pos + len].to_owned(),
            Style::default().fg(Color::Yellow),
        );
        return Some(pos + len);
    }
    let first = rest.chars().next()?;
    if first.is_ascii_digit() && prev_char_breaks_token(raw, pos) {
        let len = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
            .unwrap_or(rest.len());
        push_span(
            spans,
            raw[pos..pos + len].to_owned(),
            Style::default().fg(Color::LightCyan),
        );
        return Some(pos + len);
    }
    None
}

fn prev_char_breaks_token(raw: &str, pos: usize) -> bool {
    raw[..pos]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
}

fn try_consume_word(
    raw: &str,
    pos: usize,
    lang: &str,
    spans: &mut Vec<Span<'static>>,
) -> Option<usize> {
    let rest = &raw[pos..];
    let first = rest.chars().next()?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }
    let len = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    let word = &raw[pos..pos + len];
    let after = &raw[pos + len..];

    if after.starts_with('!') && lang == "rust" {
        let end = pos + len + 1;
        push_span(
            spans,
            raw[pos..end].to_owned(),
            Style::default().fg(Color::Cyan),
        );
        return Some(end);
    }

    let style = classify_word_style(word, after, lang);
    push_span(spans, word.to_owned(), style);
    Some(pos + len)
}

fn classify_word_style(word: &str, after: &str, lang: &str) -> Style {
    if is_keyword(word, lang) {
        return Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD);
    }
    if is_literal_word(word) {
        return Style::default().fg(Color::LightCyan);
    }
    if is_type_word(word) {
        return Style::default().fg(Color::Yellow);
    }
    if after.starts_with('(') {
        return Style::default().fg(Color::Cyan);
    }
    Style::default().fg(Color::Gray)
}

fn is_keyword(word: &str, lang: &str) -> bool {
    if matches!(
        word,
        "fn" | "let"
            | "mut"
            | "pub"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "match"
            | "if"
            | "else"
            | "for"
            | "in"
            | "while"
            | "loop"
            | "return"
            | "use"
            | "mod"
            | "crate"
            | "self"
            | "Self"
            | "super"
            | "async"
            | "await"
            | "move"
            | "where"
            | "type"
            | "const"
            | "static"
            | "unsafe"
            | "dyn"
            | "ref"
            | "as"
            | "break"
            | "continue"
            | "def"
            | "class"
            | "import"
            | "from"
            | "pass"
            | "raise"
            | "with"
            | "yield"
            | "lambda"
            | "elif"
            | "func"
            | "package"
            | "defer"
            | "chan"
            | "go"
            | "interface"
            | "switch"
            | "case"
            | "default"
            | "function"
            | "export"
            | "extends"
            | "implements"
            | "try"
            | "catch"
            | "finally"
            | "throw"
            | "new"
            | "typeof"
            | "local"
            | "then"
            | "fi"
            | "done"
            | "esac"
    ) {
        return true;
    }
    if lang == "sql" {
        return matches!(
            word.to_ascii_uppercase().as_str(),
            "SELECT"
                | "FROM"
                | "WHERE"
                | "INSERT"
                | "INTO"
                | "VALUES"
                | "UPDATE"
                | "DELETE"
                | "JOIN"
                | "LEFT"
                | "RIGHT"
                | "INNER"
                | "ON"
                | "CREATE"
                | "TABLE"
                | "ALTER"
                | "DROP"
                | "AND"
                | "OR"
                | "NOT"
                | "ORDER"
                | "BY"
                | "GROUP"
                | "LIMIT"
        );
    }
    false
}

fn is_literal_word(word: &str) -> bool {
    matches!(
        word,
        "true" | "false" | "True" | "False" | "null" | "nil" | "None" | "Some" | "Ok" | "Err"
    )
}

fn is_type_word(word: &str) -> bool {
    if matches!(
        word,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "str"
            | "char"
            | "String"
            | "Vec"
            | "Option"
            | "Result"
            | "Box"
            | "Arc"
            | "Rc"
            | "HashMap"
            | "HashSet"
            | "BTreeMap"
            | "Path"
            | "PathBuf"
    ) {
        return true;
    }
    let mut chars = word.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && word.chars().any(|c| c.is_ascii_lowercase())
}

fn consume_punct_or_space(raw: &str, pos: usize, spans: &mut Vec<Span<'static>>) -> usize {
    let rest = &raw[pos..];
    for two in ["->", "=>", "::", "..", "&&", "||", "==", "!=", "<=", ">="] {
        if rest.starts_with(two) {
            push_span(spans, two.to_owned(), Style::default().fg(Color::DarkGray));
            return pos + two.len();
        }
    }
    let Some(ch) = rest.chars().next() else {
        return raw.len();
    };
    let next = pos + ch.len_utf8();
    let style = if matches!(ch, '{' | '}' | '(' | ')' | '[' | ']' | ';' | ',' | ':') {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Gray)
    };
    push_span(spans, ch.to_string(), style);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_keywords_strings_types_and_comments() {
        let mut state = HighlightState::default();
        let line = highlight_code_line("let name: String = \"rein\"; // ok", "rust", &mut state);
        assert!(
            line.spans
                .iter()
                .any(|s| s.content == "let" && s.style.fg == Some(Color::Magenta))
        );
        assert!(
            line.spans
                .iter()
                .any(|s| s.content == "String" && s.style.fg == Some(Color::Yellow))
        );
        assert!(
            line.spans
                .iter()
                .any(|s| s.content == "\"rein\"" && s.style.fg == Some(Color::Green))
        );
        assert!(
            line.spans
                .iter()
                .any(|s| s.content == "// ok" && s.style.fg == Some(Color::DarkGray))
        );
    }

    #[test]
    fn tracks_multiline_block_comments_and_rust_lifetimes() {
        let mut state = HighlightState::default();
        let l1 = highlight_code_line("/* start", "rust", &mut state);
        let l2 = highlight_code_line("end */ fn f<'a>() {}", "rust", &mut state);
        assert_eq!(l1.spans[0].style.fg, Some(Color::DarkGray));
        assert!(
            l2.spans
                .iter()
                .any(|s| s.content == "'a" && s.style.fg == Some(Color::Yellow))
        );
        assert!(
            l2.spans
                .iter()
                .any(|s| s.content == "fn" && s.style.fg == Some(Color::Magenta))
        );
    }
}
