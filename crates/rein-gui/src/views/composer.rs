// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Rounded Claude Desktop-style composer card and model picker popover

use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use rein_core::ModeId;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{ACCENT_CORAL, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::{
    composer_input, danger_button, elevated_card, ghost_button, pill_button, primary_button,
    surface_card,
};

/// Renders the bottom-pinned composer used during active conversations.
#[must_use]
pub fn view_bottom_composer(app: &GuiApp) -> Element<'_, Message> {
    let mut inner = column![].spacing(8).max_width(760.0).width(Length::Fill);
    if app.model_picker_open {
        inner = inner.push(view_model_picker(app));
    }
    inner = inner.push(view_composer_card(app, "Write a message…"));
    inner = inner.push(
        row![
            text("Rein can make mistakes. Verify tool actions before approving.")
                .size(11)
                .color(TEXT_DIM),
            Space::new().width(Length::Fill),
            text(&app.model_label).size(11).color(TEXT_DIM),
        ]
        .width(Length::Fill),
    );

    container(inner)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(iced::Padding {
            top: 8.0,
            right: 24.0,
            bottom: 14.0,
            left: 24.0,
        })
        .into()
}

/// Renders the rounded composer card shared by Home and Active views.
#[must_use]
pub fn view_composer_card<'a>(app: &'a GuiApp, placeholder: &'a str) -> Element<'a, Message> {
    let input = text_input(placeholder, &app.input)
        .on_input(Message::InputChanged)
        .on_submit(Message::SubmitPrompt)
        .padding([10, 12])
        .size(14)
        .style(composer_input);

    let mode_label = match app.mode() {
        ModeId::Coding => "Coding",
        ModeId::Plan => "Plan",
        ModeId::Personal => "Personal",
    };
    let next_mode = match app.mode() {
        ModeId::Coding => ModeId::Plan,
        ModeId::Plan => ModeId::Personal,
        ModeId::Personal => ModeId::Coding,
    };
    let mode_btn = button(text(format!("+ {mode_label}")).size(12).color(TEXT_MUTED))
        .padding([5, 10])
        .style(ghost_button)
        .on_press(Message::SwitchMode(next_mode));

    let model_btn = button(
        text(format!("{} ⌄", app.model_label))
            .size(12)
            .color(TEXT_MUTED),
    )
    .padding([5, 10])
    .style(pill_button)
    .on_press(Message::ToggleModelPicker);

    let action_btn = if app.busy {
        button(text("■ Stop").size(12).color(TEXT_PRIMARY))
            .padding([6, 12])
            .style(danger_button)
            .on_press(Message::CancelTurn)
    } else {
        let btn = button(text("↑").size(14).color(TEXT_PRIMARY))
            .padding([6, 12])
            .style(primary_button);
        if app.input.trim().is_empty() {
            btn
        } else {
            btn.on_press(Message::SubmitPrompt)
        }
    };

    let bottom_row = row![
        mode_btn,
        Space::new().width(Length::Fill),
        model_btn,
        action_btn,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(column![input, bottom_row].spacing(6).padding([8, 10]))
        .width(Length::Fill)
        .style(surface_card)
        .into()
}

/// Renders the model picker popover card.
#[must_use]
pub fn view_model_picker(app: &GuiApp) -> Element<'_, Message> {
    let mut list = column![text("Select Model").size(12).color(TEXT_DIM)].spacing(4);
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
                .padding([6, 10])
                .style(ghost_button)
                .on_press(Message::SelectModel(m.id.clone()));
            list = list.push(item);
        }
    }

    container(scrollable(list).height(Length::Shrink))
        .padding(10)
        .max_width(380.0)
        .max_height(240.0)
        .style(elevated_card)
        .into()
}
