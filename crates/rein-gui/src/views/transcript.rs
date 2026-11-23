// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Scrollable conversation transcript view with user bubbles, markdown, and human tool cards

use iced::widget::{Space, button, column, container, markdown, row, scrollable, text};
use iced::{Alignment, Element, Length};

use crate::app::{GuiApp, Message, TranscriptItem};
use crate::theme::palette::{ACCENT_CORAL, STATUS_RED, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::{disclosure_card, ghost_button, surface_card};

use super::tool_card;

const SPINNER_FRAMES: [&str; 10] = ["✶", "✸", "✹", "✺", "✹", "✸", "✶", "✢", "✽", "✻"];

/// Renders the scrollable, centered conversation column.
#[must_use]
pub fn view_transcript(app: &GuiApp) -> Element<'_, Message> {
    let mut col = column![].spacing(14).max_width(780.0).width(Length::Fill);

    for (idx, item) in app.items.iter().enumerate() {
        col = col.push(render_item(app, idx, item));
    }

    if app.busy {
        col = col.push(render_busy_line(app));
    }

    let centered = container(col)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([12, 24]);

    scrollable(centered)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn render_item<'a>(_app: &'a GuiApp, idx: usize, item: &'a TranscriptItem) -> Element<'a, Message> {
    match item {
        TranscriptItem::User(prompt) => {
            let bubble = container(text(prompt).size(14).color(TEXT_PRIMARY))
                .padding([10, 16])
                .max_width(540.0)
                .style(surface_card);
            row![Space::new().width(Length::Fill), bubble]
                .width(Length::Fill)
                .into()
        }
        TranscriptItem::Thinking {
            content,
            elapsed_secs,
            active,
            expanded,
        } => {
            let label = if *active {
                "Thinking…".to_owned()
            } else {
                format!("Thought for {elapsed_secs}s")
            };
            let arrow = if *expanded || *active { "⌄" } else { "›" };
            let header = button(text(format!("{label}  {arrow}")).size(13).color(TEXT_MUTED))
                .padding([4, 8])
                .style(ghost_button)
                .on_press(Message::ToggleExpand(idx));
            if *expanded || *active {
                let body = container(text(content).size(12).color(TEXT_MUTED))
                    .padding(12)
                    .width(Length::Fill)
                    .style(disclosure_card);
                column![header, body].spacing(4).into()
            } else {
                let preview: String = content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(100)
                    .collect();
                if preview.is_empty() {
                    header.into()
                } else {
                    column![
                        header,
                        text(format!("  └ {preview}…")).size(12).color(TEXT_DIM),
                    ]
                    .spacing(2)
                    .into()
                }
            }
        }
        TranscriptItem::Assistant { parsed, .. } => {
            let md = markdown::view(
                parsed.items(),
                markdown::Settings::with_text_size(14, crate::theme::rein_theme()),
            )
            .map(|url| Message::LinkClicked(url.to_string()));
            container(md).width(Length::Fill).into()
        }
        TranscriptItem::Tool {
            name,
            arguments,
            state,
            background_task_id,
            output,
            expanded,
            ..
        } => tool_card::render_tool_card(
            idx,
            name,
            arguments,
            *state,
            background_task_id.as_deref(),
            output,
            *expanded,
        ),
        TranscriptItem::System(note) => text(format!("· {note}")).size(12).color(TEXT_DIM).into(),
        TranscriptItem::Error(err) => text(format!("! {err}")).size(13).color(STATUS_RED).into(),
    }
}

fn render_busy_line(app: &GuiApp) -> Element<'_, Message> {
    let glyph = SPINNER_FRAMES[(app.spinner_tick / 2) % SPINNER_FRAMES.len()];
    let elapsed = app.busy_started_at.map_or(0, |t| t.elapsed().as_secs());
    let elapsed_str = if elapsed >= 1 {
        format!(" · {elapsed}s")
    } else {
        String::new()
    };
    row![
        text(glyph).size(15).color(ACCENT_CORAL),
        text(&app.busy_status).size(13).color(ACCENT_CORAL),
        text(format!("(Esc to stop){elapsed_str}"))
            .size(12)
            .color(TEXT_DIM),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}
