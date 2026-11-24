// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Split-view right inspector drawer with line-number gutter and syntax highlighting

use iced::widget::{Space, button, column, container, rich_text, row, scrollable, span, text};
use iced::{Alignment, Color, Element, Font, Length};

use crate::app::{InspectorPane, Message};
use crate::theme::palette::{
    SYN_ACCENT, SYN_COMMENT, SYN_KEYWORD, SYN_NUMBER, SYN_STRING, TEXT_DIM, TEXT_MUTED,
    TEXT_PRIMARY,
};
use crate::theme::styles::{ghost_button, inspector_container};

const MAX_INSPECTOR_LINES: usize = 500;

/// Renders the right-hand split inspector drawer with line-number gutter and precomputed syntax spans.
#[must_use]
pub fn view_inspector_pane(pane: &InspectorPane) -> Element<'_, Message> {
    let header = row![
        text("‹").size(16).color(TEXT_MUTED),
        text(&pane.title).size(13).color(TEXT_PRIMARY),
        text(format!("· {}", pane.kind_badge))
            .size(12)
            .color(TEXT_DIM),
        Space::new().width(Length::Fill),
        button(text("Copy").size(12).color(TEXT_MUTED))
            .padding([4, 8])
            .style(ghost_button)
            .on_press(Message::CopyText(pane.content.clone())),
        button(text("✕").size(13).color(TEXT_MUTED))
            .padding([4, 8])
            .style(ghost_button)
            .on_press(Message::CloseInspector),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let meta = text(&pane.subtitle).size(11).color(TEXT_DIM);

    let mut lines_col = column![].spacing(2).width(Length::Fill);
    for (idx, line_tokens) in pane.highlighted_lines.iter().enumerate() {
        let line_no = format!("{:>4}", idx + 1);
        let spans: Vec<_> = line_tokens
            .iter()
            .map(|(tok, color)| span::<(), Font>(tok.as_str()).color(*color))
            .collect();
        let row_line = row![
            text(line_no).size(12).font(Font::MONOSPACE).color(TEXT_DIM),
            rich_text(spans).size(12).font(Font::MONOSPACE),
        ]
        .spacing(14)
        .align_y(Alignment::Start);
        lines_col = lines_col.push(row_line);
    }
    if pane.total_lines > MAX_INSPECTOR_LINES {
        lines_col = lines_col.push(
            text(format!(
                "… (preview capped at 500 of {} lines; use Copy for full content)",
                pane.total_lines
            ))
            .size(11)
            .color(TEXT_DIM),
        );
    }

    let body = scrollable(container(lines_col).padding([8, 4]).width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill);

    let inner = column![header, meta, body]
        .spacing(10)
        .padding(14)
        .width(Length::Fixed(480.0))
        .height(Length::Fill);

    container(inner)
        .height(Length::Fill)
        .style(inspector_container)
        .into()
}

/// Precomputes `(kind_badge, total_lines, highlighted_lines)` once when opening the inspector.
#[must_use]
pub fn tokenize_inspector_lines(
    title: &str,
    content: &str,
) -> (&'static str, usize, Vec<Vec<(String, Color)>>) {
    let kind_badge = infer_kind_badge(title);
    let total_lines = content.lines().count();
    let highlighted_lines = content
        .lines()
        .take(MAX_INSPECTOR_LINES)
        .map(highlight_line_tokens)
        .collect();
    (kind_badge, total_lines, highlighted_lines)
}

fn infer_kind_badge(title: &str) -> &'static str {
    let lower = title.to_ascii_lowercase();
    if lower.contains("bash") || lower.ends_with(".sh") {
        "SH"
    } else if lower.ends_with(".rs") {
        "RS"
    } else if lower.ends_with(".toml") {
        "TOML"
    } else if lower.ends_with(".json") || lower.ends_with(".jsonl") {
        "JSON"
    } else if lower.ends_with(".md") {
        "MD"
    } else {
        "TEXT"
    }
}

fn highlight_line_tokens(line: &str) -> Vec<(String, Color)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return vec![(line.to_owned(), SYN_COMMENT)];
    }

    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;

    while i < bytes.len() {
        let b = bytes[i];
        let is_word_apostrophe = b == b'\'' && i > 0 && bytes[i - 1].is_ascii_alphabetic();
        if (b == b'"' || b == b'\'') && !is_word_apostrophe {
            if plain_start < i
                && let Some(prefix) = line.get(plain_start..i)
            {
                push_tokenized_plain(prefix, &mut out);
            }
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 1;
                    if let Some(ch) = line.get(i..).and_then(|s| s.chars().next()) {
                        i += ch.len_utf8();
                    }
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else if let Some(ch) = line.get(i..).and_then(|s| s.chars().next()) {
                    i += ch.len_utf8();
                } else {
                    i += 1;
                }
            }
            if let Some(seg) = line.get(start..i) {
                out.push((seg.to_owned(), SYN_STRING));
            }
            plain_start = i;
            continue;
        }
        if let Some(ch) = line.get(i..).and_then(|s| s.chars().next()) {
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }

    if plain_start < line.len()
        && let Some(tail) = line.get(plain_start..)
    {
        push_tokenized_plain(tail, &mut out);
    }
    if out.is_empty() {
        out.push((" ".to_owned(), TEXT_PRIMARY));
    }
    out
}

fn push_tokenized_plain(chunk: &str, out: &mut Vec<(String, Color)>) {
    let mut idx = 0;
    for part in chunk.split_inclusive(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
    {
        let token =
            part.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
        let sep_len = part.len() - token.len();
        if !token.is_empty() {
            let color = classify_token(token);
            out.push((token.to_owned(), color));
        }
        if sep_len > 0 {
            let sep = &part[token.len()..];
            out.push((sep.to_owned(), TEXT_MUTED));
        }
        idx += part.len();
    }
    if idx < chunk.len() {
        out.push((chunk[idx..].to_owned(), TEXT_PRIMARY));
    }
}

fn classify_token(token: &str) -> Color {
    if matches!(
        token,
        "fn" | "pub"
            | "let"
            | "mut"
            | "const"
            | "use"
            | "mod"
            | "struct"
            | "enum"
            | "impl"
            | "if"
            | "else"
            | "match"
            | "for"
            | "while"
            | "return"
            | "set"
            | "readonly"
            | "local"
            | "export"
            | "then"
            | "fi"
            | "do"
            | "done"
    ) {
        SYN_KEYWORD
    } else if token.starts_with("--") || (token.starts_with('-') && token.len() > 1) {
        SYN_ACCENT
    } else if token.chars().all(|c| c.is_ascii_digit()) {
        SYN_NUMBER
    } else {
        TEXT_PRIMARY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_kind_badges_and_highlights_tokens() {
        assert_eq!(infer_kind_badge("bash output"), "SH");
        assert_eq!(infer_kind_badge("src/main.rs"), "RS");
        assert_eq!(infer_kind_badge("Cargo.toml"), "TOML");

        let comment = highlight_line_tokens("  // a comment");
        assert_eq!(comment.len(), 1);
        let unicode_escape = highlight_line_tokens("\"hello \\λ");
        assert_eq!(unicode_escape.len(), 1);
        let prose = highlight_line_tokens("don't highlight prose as string");
        assert!(prose.iter().all(|(_, color)| *color != SYN_STRING));
        assert_eq!(classify_token("readonly"), SYN_KEYWORD);
        assert_eq!(classify_token("--no-db"), SYN_ACCENT);
        assert_eq!(classify_token("120"), SYN_NUMBER);
    }
}
