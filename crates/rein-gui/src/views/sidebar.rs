// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Collapsible left sidebar with New Chat, session history, and workspace footer

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length};

use crate::app::{GuiApp, Message};
use crate::theme::palette::{ACCENT_CORAL, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::{ghost_button, pill_button, sidebar_container};

/// Renders the collapsible left sidebar (260px wide).
#[must_use]
pub fn view_sidebar(app: &GuiApp) -> Element<'_, Message> {
    let new_btn = button(
        row![
            text("+").size(15).color(ACCENT_CORAL),
            text("New chat").size(13).color(TEXT_PRIMARY),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([8, 12])
    .style(pill_button)
    .on_press(Message::NewSession);

    let compact_btn = button(text("Compact history").size(12).color(TEXT_MUTED))
        .width(Length::Fill)
        .padding([6, 12])
        .style(ghost_button)
        .on_press(Message::CompactHistory);

    let mut session_col = column![text("Chats").size(11).color(TEXT_DIM)].spacing(4);
    for s in app.sessions.iter().take(24) {
        let preview = s
            .preview
            .as_deref()
            .unwrap_or("Empty session")
            .chars()
            .take(30)
            .collect::<String>();
        let is_active = s.uid == app.session_uid;
        let label_color = if is_active { ACCENT_CORAL } else { TEXT_MUTED };
        let item = button(text(preview).size(12).color(label_color))
            .width(Length::Fill)
            .padding([6, 10])
            .style(ghost_button)
            .on_press(Message::ResumeSession(s.uid.clone()));
        session_col = session_col.push(item);
    }

    let footer = column![
        text(&app.user_name).size(12).color(TEXT_PRIMARY),
        text(&app.workspace_label).size(11).color(TEXT_DIM),
    ]
    .spacing(2);

    let inner = column![
        new_btn,
        compact_btn,
        scrollable(session_col).height(Length::Fill),
        Space::new().height(8),
        footer,
    ]
    .spacing(10)
    .padding(12)
    .width(Length::Fixed(250.0))
    .height(Length::Fill);

    container(inner)
        .height(Length::Fill)
        .style(sidebar_container)
        .into()
}
