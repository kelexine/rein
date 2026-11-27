// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Material 3 Navigation Rail (collapsed) and Expressive Navigation Drawer (expanded)

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length};
use rein_core::ModeId;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{
    ACCENT_CORAL, ON_PRIMARY_CONTAINER, ON_SECONDARY_CONTAINER, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme::styles::{
    active_sidebar_button, avatar_badge, extended_fab, ghost_button, nav_rail_container,
    sidebar_container, tonal_icon_button,
};

/// Renders the compact M3 Navigation Rail (`68px` wide) when the drawer is collapsed.
#[must_use]
pub fn view_nav_rail(app: &GuiApp) -> Element<'_, Message> {
    let menu_btn = button(text("☰").size(16).color(TEXT_PRIMARY))
        .padding([8, 12])
        .style(ghost_button)
        .on_press(Message::ToggleSidebar);

    let new_fab = button(text("+").size(18).color(ON_PRIMARY_CONTAINER))
        .padding([10, 14])
        .style(extended_fab)
        .on_press(Message::NewSession);

    let next_mode = match app.mode() {
        ModeId::Coding => ModeId::Plan,
        ModeId::Plan => ModeId::Personal,
        ModeId::Personal => ModeId::Coding,
    };
    let mode_glyph = match app.mode() {
        ModeId::Coding => "⌘",
        ModeId::Plan => "◈",
        ModeId::Personal => "✦",
    };
    let mode_btn = button(text(mode_glyph).size(15).color(ON_SECONDARY_CONTAINER))
        .padding([8, 14])
        .style(active_sidebar_button)
        .on_press(Message::SwitchMode(next_mode));

    let tasks_btn = button(text("◫").size(15).color(TEXT_MUTED))
        .padding([8, 12])
        .style(if app.task_drawer_open {
            active_sidebar_button
        } else {
            ghost_button
        })
        .on_press(Message::ToggleTaskDrawer);

    let compact_btn = button(text("⟳").size(15).color(TEXT_MUTED))
        .padding([8, 12])
        .style(ghost_button)
        .on_press(Message::CompactHistory);

    let settings_btn = button(text("⚙").size(15).color(if app.settings_open {
        ON_SECONDARY_CONTAINER
    } else {
        TEXT_MUTED
    }))
    .padding([8, 12])
    .style(if app.settings_open {
        active_sidebar_button
    } else {
        ghost_button
    })
    .on_press(Message::ToggleSettings);

    let badge = render_user_monogram(&app.user_name, 32.0);

    let inner = column![
        menu_btn,
        Space::new().height(4),
        new_fab,
        Space::new().height(12),
        mode_btn,
        tasks_btn,
        compact_btn,
        settings_btn,
        Space::new().height(Length::Fill),
        badge,
    ]
    .spacing(8)
    .padding([14, 8])
    .width(Length::Fixed(68.0))
    .height(Length::Fill)
    .align_x(Alignment::Center);

    container(inner)
        .height(Length::Fill)
        .style(nav_rail_container)
        .into()
}

/// Renders the expanded M3 Expressive Navigation Drawer (`268px` wide, rounded trailing corners).
#[must_use]
pub fn view_sidebar(app: &GuiApp) -> Element<'_, Message> {
    let top_row = row![
        button(text("☰").size(15).color(TEXT_MUTED))
            .padding([6, 10])
            .style(ghost_button)
            .on_press(Message::ToggleSidebar),
        text("Rein").size(15).color(TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        button(text("⟳").size(13).color(TEXT_MUTED))
            .padding([5, 9])
            .style(tonal_icon_button)
            .on_press(Message::CompactHistory),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let new_btn = button(
        row![
            text("+").size(16).color(ON_PRIMARY_CONTAINER),
            text("New chat").size(13).color(ON_PRIMARY_CONTAINER),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([10, 16])
    .style(extended_fab)
    .on_press(Message::NewSession);

    let mut session_col = column![text("Recents").size(11).color(TEXT_DIM)].spacing(4);
    for s in app.sessions.iter().take(24) {
        let is_active = s.uid == app.session_uid;
        let label_color = if is_active {
            ON_SECONDARY_CONTAINER
        } else {
            TEXT_MUTED
        };
        let item = button(text(&s.label).size(12).color(label_color))
            .width(Length::Fill)
            .padding([8, 14])
            .style(if is_active {
                active_sidebar_button
            } else {
                ghost_button
            })
            .on_press(Message::ResumeSession(s.uid.clone()));
        session_col = session_col.push(item);
    }

    let badge = render_user_monogram(&app.user_name, 32.0);
    let footer = row![
        badge,
        column![
            text(&app.user_name).size(12).color(TEXT_PRIMARY),
            text(&app.workspace_label).size(11).color(TEXT_DIM),
        ]
        .spacing(2),
        Space::new().width(Length::Fill),
        button(text("⚙  Settings").size(12).color(if app.settings_open {
            ON_SECONDARY_CONTAINER
        } else {
            TEXT_MUTED
        }))
        .padding([5, 10])
        .style(if app.settings_open {
            active_sidebar_button
        } else {
            ghost_button
        })
        .on_press(Message::ToggleSettings),
        text("●").size(8).color(ACCENT_CORAL),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let inner = column![
        top_row,
        new_btn,
        Space::new().height(4),
        scrollable(session_col).height(Length::Fill),
        Space::new().height(8),
        footer,
    ]
    .spacing(10)
    .padding(14)
    .width(Length::Fixed(268.0))
    .height(Length::Fill);

    container(inner)
        .height(Length::Fill)
        .style(sidebar_container)
        .into()
}

fn render_user_monogram(user_name: &str, size: f32) -> Element<'static, Message> {
    let initial = user_name
        .chars()
        .next()
        .map_or('R', |c| c.to_ascii_uppercase());
    container(text(initial.to_string()).size(13))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center_x(Length::Fixed(size))
        .center_y(Length::Fixed(size))
        .style(avatar_badge)
        .into()
}
