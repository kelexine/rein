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

/// Renders the right-hand split inspector drawer with line-number gutter and syntax highlighting.
#[must_use]
pub fn view_inspector_pane(pane: &InspectorPane) -> Element<'_, Message> {
    let kind_badge = infer_kind_badge(&pane.title);
    let header = row![
        text("‹").size(16).color(TEXT_MUTED),
        text(&pane.title).size(13).color(TEXT_PRIMARY),
        text(format!("· {kind_badge}")).size(12).color(TEXT_DIM),
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
    for (idx, line) in pane.content.lines().take(MAX_INSPECTOR_LINES).enumerate() {
        let line_no = format!("{:>4}", idx + 1);
        let highlighted = highlight_line_spans(line);
        let row_line = row![
            text(line_no)
                .size(12)
                .font(Font::MONOSPACE)
                .color(TEXT_DIM),
            rich_text(highlighted).size(12).font(Font::MONOSPACE),
        ]
        .spacing(14)
        .align_y(Alignment::Start);
        lines_col = lines_col.push(row_line);
    }
    if pane.content.lines().count() > MAX_INSPECTOR_LINES {
        lines_col = lines_col.push(
            text("… (preview capped at 500 lines; use Copy for full content)")
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

fn highlight_line_spans<'a>(line: &'a str) -> Vec<iced::widget::text::Span<'a, (), Font>> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return vec![span(line).color(SYN_COMMENT)];
    }

    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            if plain_start < i {
                push_tokenized_plain(&line[plain_start..i], &mut out);
            }
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            out.push(span(&line[start..i]).color(SYN_STRING));
            plain_start = i;
            continue;
        }
        i += 1;
    }

    if plain_start < line.len() {
        push_tokenized_plain(&line[plain_start..], &mut out);
    }
    if out.is_empty() {
        out.push(span(" ").color(TEXT_PRIMARY));
    }
    out
}

fn push_tokenized_plain<'a>(chunk: &'a str, out: &mut Vec<iced::widget::text::Span<'a, (), Font>>) {
    let mut idx = 0;
    for part in chunk.split_inclusive(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        let token = part.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
        let sep_len = part.len() - token.len();
        if !token.is_empty() {
            let color = classify_token(token);
            out.push(span(token).color(color));
        }
        if sep_len > 0 {
            let sep = &part[token.len()..];
            out.push(span(sep).color(TEXT_MUTED));
        }
        idx += part.len();
    }
    if idx < chunk.len() {
        out.push(span(&chunk[idx..]).color(TEXT_PRIMARY));
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

        let comment = highlight_line_spans("  // a comment");
        assert_eq!(comment.len(), 1);
        assert_eq!(classify_token("readonly"), SYN_KEYWORD);
        assert_eq!(classify_token("--no-db"), SYN_ACCENT);
        assert_eq!(classify_token("120"), SYN_NUMBER);
    }
}
