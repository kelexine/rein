// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Top-level Iced view shell (sidebar + topbar + home/transcript + split inspector + overlays)

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Font, Length};
use rein_core::ModeId;

use crate::app::{GuiApp, InspectorPane, Message};
use crate::theme::palette::{ACCENT_BLUE, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::{
    canvas_container, ghost_button, inspector_container, pill_button, primary_button,
};

use super::{composer, home, modals, sidebar, transcript};

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
        root = root.push(render_inspector_pane(pane));
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

    let mode_segmented = row![
        mode_segment(app.mode(), ModeId::Coding, "Coding"),
        mode_segment(app.mode(), ModeId::Plan, "Plan"),
        mode_segment(app.mode(), ModeId::Personal, "Personal"),
    ]
    .spacing(4);

    let running_tasks = app
        .tasks_snapshot
        .iter()
        .filter(|t| !t.status.is_done())
        .count();
    let tasks_label = if running_tasks > 0 {
        format!("Tasks ({running_tasks})")
    } else {
        "Tasks".to_owned()
    };
    let tasks_btn = button(text(tasks_label).size(12).color(TEXT_MUTED))
        .padding([4, 10])
        .style(ghost_button)
        .on_press(Message::ToggleTaskDrawer);

    let tok_label = if app.usage.total_tokens > 0 {
        format!("{:.1}k tok", app.usage.total_tokens as f64 / 1000.0)
    } else {
        String::new()
    };

    row![
        sidebar_btn,
        text(session_label).size(13).color(TEXT_PRIMARY),
        text(&app.sandbox_label).size(11).color(ACCENT_BLUE),
        Space::new().width(Length::Fill),
        text(tok_label).size(12).color(TEXT_DIM),
        tasks_btn,
        mode_segmented,
    ]
    .spacing(8)
    .padding([8, 14])
    .align_y(Alignment::Center)
    .into()
}

fn render_inspector_pane(pane: &InspectorPane) -> Element<'_, Message> {
    let header = row![
        text("‹").size(16).color(TEXT_MUTED),
        text(&pane.title).size(13).color(TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        button(text("Copy").size(12).color(TEXT_MUTED))
            .padding([4, 8])
            .style(ghost_button)
            .on_press(Message::CopyText(pane.content.clone())),
        button(text("✕").size(13).color(TEXT_MUTED))
            .padding([4, 8])
            .style(ghost_button)
            .on_press(Message::CloseInspector),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let meta = text(&pane.subtitle).size(11).color(TEXT_DIM);

    let body = scrollable(
        container(
            text(&pane.content)
                .size(12)
                .font(Font::MONOSPACE)
                .color(TEXT_PRIMARY),
        )
        .padding([8, 4])
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill);

    let inner = column![header, meta, body]
        .spacing(10)
        .padding(14)
        .width(Length::Fixed(440.0))
        .height(Length::Fill);

    container(inner)
        .height(Length::Fill)
        .style(inspector_container)
        .into()
}

fn mode_segment(active: ModeId, target: ModeId, label: &'static str) -> Element<'static, Message> {
    let is_active = active == target;
    let btn = button(
        text(label)
            .size(12)
            .color(if is_active { TEXT_PRIMARY } else { TEXT_DIM }),
    )
    .padding([4, 10])
    .on_press(Message::SwitchMode(target));
    if is_active {
        btn.style(primary_button).into()
    } else {
        btn.style(pill_button).into()
    }
}
