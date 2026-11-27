// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Material 3 Expressive window shell (Nav Rail / Drawer + Floating Top App Bar + Side Sheet)

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Length};
use rein_core::ModeId;

use crate::app::{GuiApp, Message};
use crate::theme::palette::{ACCENT_BLUE, ACCENT_CORAL, TEXT_DIM, TEXT_MUTED, TEXT_PRIMARY};
use crate::theme::styles::{
    canvas_container, floating_app_bar, ghost_button, pill_button, tonal_pill_button,
};

use super::{composer, home, inspector, modals, settings, sidebar, transcript};

/// Renders the complete Rein GUI window in M3 Expressive layout.
#[must_use]
pub fn view(app: &GuiApp) -> Element<'_, Message> {
    let topbar = render_floating_topbar(app);
    let body: Element<'_, Message> = if app.settings_open {
        settings::view_settings(app)
    } else if app.items.is_empty() && !app.busy {
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

    let nav = if app.sidebar_open {
        sidebar::view_sidebar(app)
    } else {
        sidebar::view_nav_rail(app)
    };

    let mut root = row![nav, center_col]
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(pane) = &app.inspector {
        root = root.push(inspector::view_inspector_pane(pane));
    }

    container(root)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(canvas_container)
        .into()
}

fn render_floating_topbar(app: &GuiApp) -> Element<'_, Message> {
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
            text(mode_label).size(12),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([5, 14])
    .style(tonal_pill_button)
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

    let mut pill_row = row![
        text(session_label).size(13).color(TEXT_PRIMARY),
        text(&app.sandbox_label).size(11).color(ACCENT_BLUE),
        Space::new().width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    if !tok_label.is_empty() {
        pill_row = pill_row.push(text(tok_label).size(11).color(TEXT_DIM));
    }
    if running_tasks > 0 || app.task_drawer_open {
        let label = if running_tasks > 0 {
            format!("Tasks ({running_tasks})")
        } else {
            "Tasks".to_owned()
        };
        pill_row = pill_row.push(
            button(text(label).size(12).color(TEXT_MUTED))
                .padding([4, 12])
                .style(if app.task_drawer_open {
                    pill_button
                } else {
                    ghost_button
                })
                .on_press(Message::ToggleTaskDrawer),
        );
    }
    pill_row = pill_row.push(mode_pill);
    pill_row = pill_row.push(
        button(text("⚙").size(13).color(if app.settings_open {
            ACCENT_CORAL
        } else {
            TEXT_MUTED
        }))
        .padding([4, 10])
        .style(if app.settings_open {
            pill_button
        } else {
            ghost_button
        })
        .on_press(Message::ToggleSettings),
    );

    let bar_card = container(pill_row)
        .max_width(820.0)
        .width(Length::Fill)
        .padding([8, 18])
        .style(floating_app_bar);

    container(bar_card)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(iced::Padding {
            top: 10.0,
            right: 24.0,
            bottom: 4.0,
            left: 24.0,
        })
        .into()
}
