// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Material 3 Expressive floating pill composer, Primary Send FAB, and elevated popovers

use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use rein_core::ModeId;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{
    ACCENT_CORAL, STATUS_GREEN, STATUS_YELLOW, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme::styles::{
    composer_card, composer_input, danger_button, disclosure_card, elevated_card, ghost_button,
    pill_button, primary_button, tonal_icon_button, tonal_pill_button,
};

/// Renders the bottom-floating M3 pill composer used during active conversations.
#[must_use]
pub fn view_bottom_composer(app: &GuiApp) -> Element<'_, Message> {
    let mut inner = column![].spacing(10).max_width(780.0).width(Length::Fill);
    if !app.todos.is_empty() {
        inner = inner.push(view_todo_strip(app.todo_title.as_deref(), &app.todos));
    }
    if app.plus_menu_open {
        inner = inner.push(view_plus_menu(app));
    }
    if app.model_picker_open {
        inner = inner.push(view_model_picker(app));
    }
    let placeholder = if app.busy {
        "Steer active turn, or press ■ / Esc to stop…"
    } else {
        "Write a message… (/new, /compact, /plan, /tasks)"
    };
    inner = inner.push(view_composer_card(app, placeholder));
    inner = inner.push(
        row![
            text("Rein can make mistakes. Verify tool actions before approving.")
                .size(11)
                .color(TEXT_DIM),
            Space::new().width(Length::Fill),
            text(&app.model_label).size(11).color(TEXT_DIM),
        ]
        .padding([0, 8])
        .width(Length::Fill),
    );

    container(inner)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(iced::Padding {
            top: 6.0,
            right: 24.0,
            bottom: 14.0,
            left: 24.0,
        })
        .into()
}

/// Renders the M3 Expressive floating composer card (`28px` radius, Level 3 shadow, Send FAB).
#[must_use]
pub fn view_composer_card<'a>(app: &'a GuiApp, placeholder: &'a str) -> Element<'a, Message> {
    let input = text_input(placeholder, &app.input)
        .on_input(Message::InputChanged)
        .on_submit(Message::SubmitPrompt)
        .padding([10, 14])
        .size(14)
        .style(composer_input);

    let plus_btn = button(text("+").size(16))
        .padding([6, 12])
        .style(tonal_icon_button)
        .on_press(Message::TogglePlusMenu);

    let model_btn = button(text(format!("{} ⌄", app.model_label)).size(12))
        .padding([6, 14])
        .style(tonal_pill_button)
        .on_press(Message::ToggleModelPicker);

    let mut bottom_row = row![plus_btn, Space::new().width(Length::Fill), model_btn]
        .spacing(8)
        .align_y(Alignment::Center);

    if app.busy {
        if !app.input.trim().is_empty() {
            bottom_row = bottom_row.push(
                button(text("↑").size(14))
                    .padding([7, 14])
                    .style(primary_button)
                    .on_press(Message::SubmitPrompt),
            );
        }
        bottom_row = bottom_row.push(
            button(text("■").size(12))
                .padding([7, 14])
                .style(danger_button)
                .on_press(Message::CancelTurn),
        );
    } else {
        let btn = button(text("↑").size(14))
            .padding([7, 14])
            .style(primary_button);
        let btn = if app.input.trim().is_empty() {
            btn
        } else {
            btn.on_press(Message::SubmitPrompt)
        };
        bottom_row = bottom_row.push(btn);
    }

    container(column![input, bottom_row].spacing(6).padding([10, 14]))
        .width(Length::Fill)
        .style(composer_card)
        .into()
}

/// Renders the M3 elevated `+` action menu sheet above the composer.
#[must_use]
pub fn view_plus_menu(app: &GuiApp) -> Element<'_, Message> {
    let active_mode = app.mode();
    let mode_row = |mode: ModeId, label: &'static str| {
        let check = if active_mode == mode { "✓ " } else { "   " };
        let color = if active_mode == mode {
            ACCENT_CORAL
        } else {
            TEXT_PRIMARY
        };
        button(text(format!("{check}Mode: {label}")).size(13).color(color))
            .width(Length::Fill)
            .padding([7, 12])
            .style(ghost_button)
            .on_press(Message::SwitchMode(mode))
    };

    let running_tasks = app
        .tasks_snapshot
        .iter()
        .filter(|t| !t.status.is_done())
        .count();
    let tasks_label = if running_tasks > 0 {
        format!("   Background tasks ({running_tasks})")
    } else {
        "   Background tasks".to_owned()
    };

    let list = column![
        mode_row(ModeId::Coding, "Coding"),
        mode_row(ModeId::Plan, "Plan (read-only)"),
        mode_row(ModeId::Personal, "Personal"),
        button(text(tasks_label).size(13).color(TEXT_MUTED))
            .width(Length::Fill)
            .padding([7, 12])
            .style(ghost_button)
            .on_press(Message::ToggleTaskDrawer),
        button(text("   Compact history").size(13).color(TEXT_MUTED))
            .width(Length::Fill)
            .padding([7, 12])
            .style(ghost_button)
            .on_press(Message::CompactHistory),
        button(text("   New chat").size(13).color(TEXT_MUTED))
            .width(Length::Fill)
            .padding([7, 12])
            .style(ghost_button)
            .on_press(Message::NewSession),
    ]
    .spacing(3);

    container(list)
        .padding(10)
        .max_width(250.0)
        .style(elevated_card)
        .into()
}

/// Renders the M3 elevated model picker sheet.
#[must_use]
pub fn view_model_picker(app: &GuiApp) -> Element<'_, Message> {
    let mut list = column![text("Select Model").size(12).color(TEXT_DIM)].spacing(5);
    if app.available_models.is_empty() {
        list = list.push(text("Fetching models…").size(12).color(TEXT_MUTED));
    } else {
        for m in &app.available_models {
            let quota = m
                .quota_percent
                .map_or_else(String::new, |q| format!(" · {q}%"));
            let label = format!("{}{quota}", m.display_name);
            let is_active = app.model_label.contains(&m.id);
            let color = if is_active {
                ACCENT_CORAL
            } else {
                TEXT_PRIMARY
            };
            let item = button(text(label).size(13).color(color))
                .width(Length::Fill)
                .padding([7, 14])
                .style(if is_active {
                    tonal_pill_button
                } else {
                    pill_button
                })
                .on_press(Message::SelectModel(m.id.clone()));
            list = list.push(item);
        }
    }

    container(scrollable(list).height(Length::Shrink))
        .padding(12)
        .max_width(400.0)
        .max_height(260.0)
        .style(elevated_card)
        .into()
}

fn view_todo_strip<'a>(
    todo_title: Option<&'a str>,
    todos: &'a [rein_tools::TodoItem],
) -> Element<'a, Message> {
    let completed = todos
        .iter()
        .filter(|t| matches!(t.status, rein_tools::TodoStatus::Completed))
        .count();
    let total = todos.len();
    let title = todo_title.unwrap_or("Tasks");
    let mut col = column![
        row![
            text(format!("{title} ({completed}/{total})"))
                .size(12)
                .color(TEXT_PRIMARY),
        ]
        .align_y(Alignment::Center)
    ]
    .spacing(3);

    for item in todos.iter().take(5) {
        let (glyph, color) = match item.status {
            rein_tools::TodoStatus::Completed => ("✓", STATUS_GREEN),
            rein_tools::TodoStatus::InProgress => ("●", STATUS_YELLOW),
            rein_tools::TodoStatus::Pending => ("○", TEXT_DIM),
        };
        col = col.push(
            row![
                text(glyph).size(11).color(color),
                text(&item.content).size(12).color(TEXT_MUTED),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }

    container(col.padding([8, 14]))
        .width(Length::Fill)
        .style(disclosure_card)
        .into()
}
