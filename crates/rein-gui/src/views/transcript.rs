// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Scrollable conversation transcript view with Claude Desktop bubbles, pasted cards, and disclosures

use iced::widget::{Space, button, column, container, markdown, row, scrollable, text};
use iced::{Alignment, Element, Font, Length};

use crate::app::{GuiApp, Message, TranscriptItem};
use crate::theme::palette::{
    ACCENT_CORAL, STATUS_RED, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY, rein_markdown_settings,
};
use crate::theme::styles::{disclosure_card, ghost_button, pasted_badge_card, surface_card};

use super::tool_card;

const SPINNER_FRAMES: [&str; 10] = ["✶", "✸", "✹", "✺", "✹", "✸", "✶", "✢", "✽", "✻"];

/// Renders the scrollable, centered conversation column.
#[must_use]
pub fn view_transcript(app: &GuiApp) -> Element<'_, Message> {
    let mut col = column![].spacing(16).max_width(760.0).width(Length::Fill);

    for (idx, item) in app.items.iter().enumerate() {
        col = col.push(render_item(idx, item));
    }

    if app.busy {
        col = col.push(render_busy_line(app));
    }

    let centered = container(col)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([12, 24]);

    scrollable(centered)
        .id(app.transcript_id.clone())
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn render_item<'a>(idx: usize, item: &'a TranscriptItem) -> Element<'a, Message> {
    match item {
        TranscriptItem::User {
            raw,
            preview,
            subtitle,
            is_long,
            is_pasted,
            expanded,
        } => render_user_item(idx, raw, preview, subtitle, *is_long, *is_pasted, *expanded),
        TranscriptItem::Thinking {
            content,
            preview,
            elapsed_secs,
            active,
            expanded,
        } => render_thinking_item(idx, content, preview, *elapsed_secs, *active, *expanded),
        TranscriptItem::Assistant { raw, parsed } => {
            let md = markdown::view(parsed.items(), rein_markdown_settings())
                .map(|url| Message::LinkClicked(url.to_string()));
            let actions = row![
                button(text("Copy").size(11).color(TEXT_DIM))
                    .padding([2, 6])
                    .style(ghost_button)
                    .on_press(Message::CopyText(raw.clone())),
            ]
            .spacing(6);
            column![container(md).width(Length::Fill), actions]
                .spacing(6)
                .width(Length::Fill)
                .into()
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

fn render_user_item<'a>(
    idx: usize,
    raw: &'a str,
    preview: &'a str,
    subtitle: &'a str,
    is_long: bool,
    is_pasted: bool,
    expanded: bool,
) -> Element<'a, Message> {
    if is_pasted {
        let card_content = column![
            text(preview)
                .size(12)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
            row![
                text("PASTED").size(10).color(TEXT_DIM),
                Space::new().width(Length::Fill),
                text(subtitle).size(10).color(TEXT_DIM),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(8)
        .width(Length::Fixed(240.0));

        let badge = button(container(card_content).padding([10, 12]).style(pasted_badge_card))
            .padding(0)
            .style(ghost_button)
            .on_press(Message::OpenInspector {
                title: "Pasted content".to_owned(),
                subtitle: subtitle.to_owned(),
                content: raw.to_owned(),
            });

        return row![Space::new().width(Length::Fill), badge]
            .width(Length::Fill)
            .into();
    }

    let body: Element<'a, Message> = if is_long && !expanded {
        column![
            text(format!("{preview}…")).size(14).color(TEXT_PRIMARY),
            row![
                button(text("Show more").size(12).color(TEXT_MUTED))
                    .padding([2, 6])
                    .style(ghost_button)
                    .on_press(Message::ToggleUserExpand(idx)),
                button(text("Inspect").size(12).color(TEXT_DIM))
                    .padding([2, 6])
                    .style(ghost_button)
                    .on_press(Message::OpenInspector {
                        title: "User prompt".to_owned(),
                        subtitle: subtitle.to_owned(),
                        content: raw.to_owned(),
                    }),
            ]
            .spacing(6),
        ]
        .spacing(6)
        .into()
    } else if is_long {
        column![
            text(raw).size(14).color(TEXT_PRIMARY),
            button(text("Show less").size(12).color(TEXT_MUTED))
                .padding([2, 6])
                .style(ghost_button)
                .on_press(Message::ToggleUserExpand(idx)),
        ]
        .spacing(6)
        .into()
    } else {
        text(raw).size(14).color(TEXT_PRIMARY).into()
    };

    let bubble = container(body)
        .padding([10, 16])
        .max_width(560.0)
        .style(surface_card);
    row![Space::new().width(Length::Fill), bubble]
        .width(Length::Fill)
        .into()
}

fn render_thinking_item<'a>(
    idx: usize,
    content: &'a str,
    preview: &'a str,
    elapsed_secs: u64,
    active: bool,
    expanded: bool,
) -> Element<'a, Message> {
    let label = if active {
        "Thinking…".to_owned()
    } else {
        format!("Thought for {elapsed_secs}s")
    };
    let arrow = if expanded || active { "⌄" } else { "›" };
    let header = button(text(format!("{label} {arrow}")).size(13).color(TEXT_MUTED))
        .padding([3, 6])
        .style(ghost_button)
        .on_press(Message::ToggleExpand(idx));
    if expanded || active {
        let body = container(text(content).size(12).color(TEXT_MUTED))
            .padding(12)
            .width(Length::Fill)
            .style(disclosure_card);
        column![header, body].spacing(4).into()
    } else if preview.is_empty() {
        header.into()
    } else {
        column![
            header,
            text(format!("  {preview}…")).size(12).color(TEXT_DIM),
        ]
        .spacing(2)
        .into()
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
