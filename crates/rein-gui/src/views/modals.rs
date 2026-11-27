// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Policy approval prompt, ask_question modal, and background task drawer

use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Font, Length};
use rein::agent::ApprovalAction;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{
    ACCENT_CORAL, STATUS_GREEN, STATUS_RED, STATUS_YELLOW, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme::styles::{
    composer_input, danger_button, disclosure_card, elevated_card, ghost_button, pill_button,
    primary_button,
};
use crate::worker::{PendingApprovalPrompt, PendingQuestionPrompt};

/// Renders the interactive policy approval banner when a tool requests confirmation.
#[must_use]
pub fn view_approval_banner(pending: &PendingApprovalPrompt) -> Element<'_, Message> {
    let header = row![
        text("●").size(9).color(ACCENT_CORAL),
        text("Permission required").size(13).color(ACCENT_CORAL),
        text(clip_label(&pending.request.excerpt, 64))
            .size(12)
            .font(Font::MONOSPACE)
            .color(TEXT_DIM),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let prompt_box = container(
        scrollable(text(&pending.request.title).size(13).color(TEXT_PRIMARY))
            .height(Length::Shrink),
    )
    .max_height(130.0)
    .width(Length::Fill)
    .padding([8, 10])
    .style(disclosure_card);

    let pat = &pending.request.pattern;
    let pattern_line = row![
        text("Allowlist pattern:").size(11).color(TEXT_DIM),
        text(clip_label(pat, 72))
            .size(11)
            .font(Font::MONOSPACE)
            .color(TEXT_MUTED),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let fb_box = container(
        text_input("Optional feedback if denying…", &pending.feedback)
            .on_input(Message::ApprovalFeedbackChanged)
            .padding([6, 10])
            .size(12)
            .style(composer_input),
    )
    .width(Length::Fill)
    .style(disclosure_card);

    let buttons = row![
        button(text("Allow once").size(12))
            .padding([6, 12])
            .style(primary_button)
            .on_press(Message::ResolveApproval(ApprovalAction::AllowOnce)),
        button(text("Allow in CWD").size(12))
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
        Space::new().width(Length::Fill),
        button(text("Deny").size(12))
            .padding([6, 14])
            .style(danger_button)
            .on_press(Message::ResolveApproval(ApprovalAction::Deny)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let card = container(
        column![header, prompt_box, pattern_line, fb_box, buttons]
            .spacing(10)
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

fn clip_label(raw: &str, max_chars: usize) -> String {
    let single: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut iter = single.chars();
    let head: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
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
    let hint = if q.is_multi_select {
        " (select one or more)"
    } else {
        ""
    };
    let title = text(format!(
        "Question {}/{}{hint}: {}",
        app.question_step + 1,
        pending.questions.len(),
        q.question
    ))
    .size(13)
    .color(TEXT_PRIMARY);

    let mut opts_col = column![].spacing(4);
    for (idx, opt) in q.options.iter().enumerate() {
        let selected = app.question_selected.contains(&idx);
        let prefix = if q.is_multi_select {
            if selected { "☑ " } else { "☐ " }
        } else if selected {
            "● "
        } else {
            "○ "
        };
        let color = if selected { ACCENT_CORAL } else { TEXT_MUTED };
        opts_col = opts_col.push(
            button(text(format!("{prefix}{opt}")).size(13).color(color))
                .width(Length::Fill)
                .padding([5, 10])
                .style(ghost_button)
                .on_press(Message::SelectQuestionOption(idx)),
        );
    }

    let custom_input = text_input("Optional custom answer…", &app.question_custom_text)
        .on_input(Message::QuestionCustomTextChanged)
        .on_submit(Message::ConfirmQuestion)
        .padding([6, 10])
        .size(13)
        .style(composer_input);

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

    let card = container(
        column![title, opts_col, custom_input, actions]
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
