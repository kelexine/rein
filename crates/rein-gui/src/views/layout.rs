// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Minimalist top-level Iced view shell (sidebar + topbar + home/transcript + split inspector)

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Length};
use rein_core::ModeId;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{ACCENT_BLUE, ACCENT_CORAL, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::{canvas_container, ghost_button, pill_button};

use super::{composer, home, inspector, modals, sidebar, transcript};

/// Renders the complete Rein GUI window.
#[must_use]
pub fn view(app: &GuiApp) -> Element<'_, Message> {
    let topbar = render_topbar(app);
    let body: Element<'_, Message> = if app.items.is_empty() && !app.busy {
        home::view_home(app)
    } else {
        column![
            transcript::view_transcript(app),
            composer::view_bottom_composer(app),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    };

    let mut center_col = column![topbar, body]
        .width(Length::Fill)
        .height(Length::Fill);

    if app.task_drawer_open {
        center_col = center_col.push(modals::view_task_drawer(app));
    }
    if let Some(pending) = &app.pending_approval {
        center_col = center_col.push(modals::view_approval_banner(pending));
    }
    if let Some(question) = &app.pending_question {
        center_col = center_col.push(modals::view_question_banner(app, question));
    }

    let mut root = row![].width(Length::Fill).height(Length::Fill);
    if app.sidebar_open {
        root = root.push(sidebar::view_sidebar(app));
    }
    root = root.push(center_col);
    if let Some(pane) = &app.inspector {
        root = root.push(inspector::view_inspector_pane(pane));
    }

    container(root)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(canvas_container)
        .into()
}

fn render_topbar(app: &GuiApp) -> Element<'_, Message> {
    let sidebar_btn = button(text("◧").size(15).color(TEXT_MUTED))
        .padding([5, 9])
        .style(ghost_button)
        .on_press(Message::ToggleSidebar);

    let session_label = if app.items.is_empty() {
        app.workspace_label.clone()
    } else {
        format!("{} · {}", app.workspace_label, app.session_uid)
    };

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
    let mode_pill = button(
        row![
            text("●").size(8).color(ACCENT_CORAL),
            text(mode_label).size(12).color(TEXT_MUTED),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([4, 10])
    .style(pill_button)
    .on_press(Message::SwitchMode(next_mode));

    let running_tasks = app
        .tasks_snapshot
        .iter()
        .filter(|t| !t.status.is_done())
        .count();

    let tok_label = if app.usage.total_tokens > 0 {
        format!("{:.1}k tok", app.usage.total_tokens as f64 / 1000.0)
    } else {
        String::new()
    };

    let mut bar = row![
        sidebar_btn,
        text(session_label).size(13).color(TEXT_PRIMARY),
        text(&app.sandbox_label).size(11).color(ACCENT_BLUE),
        Space::new().width(Length::Fill),
    ]
    .spacing(10)
    .padding([10, 16])
    .align_y(Alignment::Center);

    if !tok_label.is_empty() {
        bar = bar.push(text(tok_label).size(11).color(TEXT_DIM));
    }
    if running_tasks > 0 || app.task_drawer_open {
        let label = if running_tasks > 0 {
            format!("Tasks ({running_tasks})")
        } else {
            "Tasks".to_owned()
        };
        bar = bar.push(
            button(text(label).size(12).color(TEXT_MUTED))
                .padding([4, 9])
                .style(ghost_button)
                .on_press(Message::ToggleTaskDrawer),
        );
    }
    bar.push(mode_pill).into()
}
