// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Iced desktop GUI entrypoint for Rein reusing rein::runtime::ReinSession

//! Native Iced desktop GUI for Rein.
#![forbid(unsafe_code)]

/// Elm application state, messages, and update logic.
pub mod app;
/// Claude Desktop-inspired dark palette and widget styles.
pub mod theme;
/// Modular Iced UI views (sidebar, home greeting, transcript, composer, modals).
pub mod views;
/// Background agent worker bridge and Iced subscription.
pub mod worker;

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rein::runtime::ReinSession;

use app::{GuiApp, update};
use theme::rein_theme;
use views::layout::view;
use worker::app_subscription;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let workspace_root: PathBuf =
        std::env::current_dir().context("failed to resolve workspace root")?;
    let session = ReinSession::bootstrap(&workspace_root, &args)?;
    let boot_slot = Mutex::new(Some(session));

    iced::application(
        move || {
            let taken = boot_slot.lock().ok().and_then(|mut guard| guard.take());
            match taken {
                Some(s) => GuiApp::from_session(s),
                None => GuiApp::fallback(&workspace_root, &args),
            }
        },
        update,
        view,
    )
    .title(|app: &GuiApp| format!("Rein — {}", app.workspace_label))
    .theme(|_app: &GuiApp| rein_theme())
    .subscription(|app: &GuiApp| {
        app_subscription(
            std::sync::Arc::clone(&app.busy_flag),
            app.worker.wake_flag(),
        )
    })
    .run()
    .map_err(|err| anyhow::anyhow!("iced GUI failed: {err}"))
}
