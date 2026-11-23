// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Policy approval prompt, ask_question modal, and background task drawer

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Element, Length};
use rein::agent::ApprovalAction;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{
    ACCENT_CORAL, STATUS_GREEN, STATUS_RED, STATUS_YELLOW, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme::styles::{
    composer_input, danger_button, elevated_card, ghost_button, pill_button, primary_button,
};
use crate::worker::{PendingApprovalPrompt, PendingQuestionPrompt};

/// Renders the interactive policy approval banner when a tool requests confirmation.
#[must_use]
pub fn view_approval_banner(pending: &PendingApprovalPrompt) -> Element<'_, Message> {
    let header = row![
        text("Permission required").size(13).color(ACCENT_CORAL),
        text(&pending.request.excerpt).size(12).color(TEXT_DIM),
    ]
    .spacing(10);

    let prompt = text(&pending.request.title).size(13).color(TEXT_PRIMARY);
    let fb_input = text_input("Optional feedback if denying…", &pending.feedback)
        .on_input(Message::ApprovalFeedbackChanged)
        .padding([6, 10])
        .size(13)
        .style(composer_input);

    let pat = &pending.request.pattern;
    let buttons = row![
        button(text("Allow once").size(12))
            .padding([6, 12])
            .style(primary_button)
            .on_press(Message::ResolveApproval(ApprovalAction::AllowOnce)),
        button(text(format!("Allow in CWD ({pat})")).size(12))
            .padding([6, 12])
            .style(pill_button)
            .on_press(Message::ResolveApproval(ApprovalAction::AllowCwd(
                pat.clone()
            ))),
        button(text("Allow Global").size(12))
            .padding([6, 12])
            .style(pill_button)
            .on_press(Message::ResolveApproval(ApprovalAction::AllowGlobal(
                pat.clone()
            ))),
        button(text("Deny").size(12))
            .padding([6, 12])
            .style(danger_button)
            .on_press(Message::ResolveApproval(ApprovalAction::Deny)),
    ]
    .spacing(8);

    let card = container(
        column![header, prompt, fb_input, buttons]
            .spacing(8)
            .padding(14),
    )
    .max_width(760.0)
    .width(Length::Fill)
    .style(elevated_card);

    container(card)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([6, 24])
        .into()
}

/// Renders the interactive `ask_question` picker banner.
#[must_use]
pub fn view_question_banner<'a>(
    app: &'a GuiApp,
    pending: &'a PendingQuestionPrompt,
) -> Element<'a, Message> {
    let Some(q) = pending.questions.get(app.question_step) else {
        return Space::new().into();
    };
    let title = text(format!(
        "Question {}/{}: {}",
        app.question_step + 1,
        pending.questions.len(),
        q.question
    ))
    .size(13)
    .color(TEXT_PRIMARY);

    let mut opts_col = column![].spacing(4);
    for (idx, opt) in q.options.iter().enumerate() {
        let selected = app.question_selected == Some(idx);
        let prefix = if selected { "● " } else { "○ " };
        let color = if selected { ACCENT_CORAL } else { TEXT_MUTED };
        opts_col = opts_col.push(
            button(text(format!("{prefix}{opt}")).size(13).color(color))
                .width(Length::Fill)
                .padding([5, 10])
                .style(ghost_button)
                .on_press(Message::SelectQuestionOption(idx)),
        );
    }

    let actions = row![
        button(text("Confirm").size(12))
            .padding([6, 12])
            .style(primary_button)
            .on_press(Message::ConfirmQuestion),
        button(text("Cancel").size(12))
            .padding([6, 12])
            .style(danger_button)
            .on_press(Message::CancelQuestion),
    ]
    .spacing(8);

    let card = container(column![title, opts_col, actions].spacing(8).padding(14))
        .max_width(760.0)
        .width(Length::Fill)
        .style(elevated_card);

    container(card)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([6, 24])
        .into()
}

/// Renders the background task drawer listing active/recent `TaskRegistry` items.
#[must_use]
pub fn view_task_drawer(app: &GuiApp) -> Element<'_, Message> {
    let mut col = column![text("Background Tasks").size(13).color(TEXT_PRIMARY)].spacing(6);
    if app.tasks_snapshot.is_empty() {
        col = col.push(
            text("No background tasks or schedules.")
                .size(12)
                .color(TEXT_DIM),
        );
    } else {
        for task in &app.tasks_snapshot {
            let active = !task.status.is_done();
            let color = if active {
                STATUS_YELLOW
            } else if matches!(task.status, rein_tools::TaskStatus::Completed) {
                STATUS_GREEN
            } else {
                STATUS_RED
            };
            let mut r = row![
                text("●").size(10).color(color),
                text(&task.id).size(12).color(TEXT_PRIMARY),
                text(&task.label).size(12).color(TEXT_MUTED),
                text(task.status.label()).size(11).color(TEXT_DIM),
                Space::new().width(Length::Fill),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            if active {
                r = r.push(
                    button(text("Kill").size(11))
                        .padding([3, 8])
                        .style(danger_button)
                        .on_press(Message::KillBackgroundTask(task.id.clone())),
                );
            }
            col = col.push(r);
        }
    }

    let card = container(col.padding(12))
        .max_width(760.0)
        .width(Length::Fill)
        .style(elevated_card);

    container(card)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding([4, 24])
        .into()
}
