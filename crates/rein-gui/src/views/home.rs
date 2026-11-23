// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Rein Citadel Owl empty-state greeting, workspace badge, quick actions, and composer

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Font, Length};

use crate::app::{GuiApp, Message};
use crate::theme::palette::{ACCENT_BLUE, ACCENT_CORAL, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::pill_button;

use super::composer;

const CITADEL_OWL: &str = "▜▙████▟▛\n▐▛▄██▄▜▌\n ▝▜▀▀▛▘ ";

/// Renders Rein's Citadel Owl home view with workspace status, quick-action chips, and composer.
#[must_use]
pub fn view_home(app: &GuiApp) -> Element<'_, Message> {
    let owl = text(CITADEL_OWL)
        .size(15)
        .font(Font::MONOSPACE)
        .color(ACCENT_CORAL);

    let title_col = column![
        row![
            text("Rein").size(28).color(TEXT_PRIMARY),
            text(format!("v{}", env!("CARGO_PKG_VERSION")))
                .size(12)
                .color(TEXT_DIM),
        ]
        .spacing(8)
        .align_y(iced::Bottom),
        row![
            text(&app.workspace_label).size(12).color(TEXT_MUTED),
            text("·").size(12).color(TEXT_DIM),
            text(&app.sandbox_label).size(12).color(ACCENT_BLUE),
        ]
        .spacing(6),
    ]
    .spacing(4);

    let header = row![owl, title_col].spacing(14).align_y(Alignment::Center);

    let card =
        composer::view_composer_card(app, "Ask Rein to inspect code, plan changes, or run tasks…");

    let chips = row![
        quick_chip(
            "Analyze codebase",
            "Analyze the codebase structure and summarize key components."
        ),
        quick_chip(
            "Check git status",
            "Check git status and recent commits in this workspace."
        ),
        quick_chip(
            "Run tests",
            "Run the project test suite and report any failures."
        ),
        quick_chip(
            "Plan a change",
            "Switch to plan mode and help me design a change."
        ),
    ]
    .spacing(8);

    let mut center_col = column![
        header,
        Space::new().height(20),
        card,
        Space::new().height(12),
        chips,
    ]
    .max_width(720.0)
    .width(Length::Fill)
    .align_x(Alignment::Center);

    if app.model_picker_open {
        center_col = center_col
            .push(Space::new().height(8))
            .push(composer::view_model_picker(app));
    }

    container(center_col)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding([0, 24])
        .into()
}

fn quick_chip<'a>(label: &'a str, prompt: &'a str) -> Element<'a, Message> {
    button(text(label).size(12).color(TEXT_MUTED))
        .padding([6, 12])
        .style(pill_button)
        .on_press(Message::QuickPrompt(prompt.to_owned()))
        .into()
}
