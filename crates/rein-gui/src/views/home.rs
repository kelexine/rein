// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Material 3 Expressive Home screen with squircle Citadel Owl badge and suggestion chips

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Font, Length};

use crate::app::{GuiApp, Message};
use crate::theme::palette::{ACCENT_CORAL, TEXT_DIM, TEXT_PRIMARY};
use crate::theme::styles::{hero_emblem_badge, suggestion_chip};
use crate::theme::tokens::DISPLAY_SMALL;

use super::composer;

const CITADEL_OWL: &str = "▜▙████▟▛\n▐▛▄██▄▜▌\n ▝▜▀▀▛▘ ";

/// Renders Rein's M3 Expressive home view with squircle emblem badge, floating composer, and suggestion chips.
#[must_use]
pub fn view_home(app: &GuiApp) -> Element<'_, Message> {
    let owl_badge = container(
        text(CITADEL_OWL)
            .size(15)
            .font(Font::MONOSPACE)
            .color(ACCENT_CORAL),
    )
    .padding([12, 14])
    .style(hero_emblem_badge);

    let greeting = column![
        text(format!("Ready when you are, {}", app.user_name))
            .size(DISPLAY_SMALL)
            .color(TEXT_PRIMARY),
        text(format!("{}  ·  {}", app.workspace_label, app.sandbox_label))
            .size(12)
            .color(TEXT_DIM),
    ]
    .spacing(4);

    let header = row![owl_badge, greeting]
        .spacing(18)
        .align_y(Alignment::Center);

    let card =
        composer::view_composer_card(app, "Ask Rein to inspect code, plan changes, or run tasks…");

    let chips = row![
        quick_chip(
            "✦  Analyze codebase",
            "Analyze the codebase structure and summarize key components."
        ),
        quick_chip(
            "⎇  Check git status",
            "Check git status and recent commits in this workspace."
        ),
        quick_chip(
            "▷  Run tests",
            "Run the project test suite and report any failures."
        ),
        quick_chip(
            "◈  Plan a change",
            "Switch to plan mode and help me design a change."
        ),
    ]
    .spacing(10);

    let mut center_col = column![
        header,
        Space::new().height(26),
        card,
        Space::new().height(14),
        chips,
    ]
    .max_width(740.0)
    .width(Length::Fill)
    .align_x(Alignment::Center);

    if app.plus_menu_open {
        center_col = center_col
            .push(Space::new().height(10))
            .push(composer::view_plus_menu(app));
    }
    if app.model_picker_open {
        center_col = center_col
            .push(Space::new().height(10))
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
    button(text(label).size(12))
        .padding([8, 16])
        .style(suggestion_chip)
        .on_press(Message::QuickPrompt(prompt.to_owned()))
        .into()
}
