// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: View module exports for rein-gui

/// Rounded composer card, mode selector, and model picker popover.
pub mod composer;
/// Empty-state centered greeting and composer layout.
pub mod home;
/// Top-level window shell combining sidebar, topbar, main view, and modals.
pub mod layout;
/// Policy approval modal, `ask_question` picker, and background task drawer.
pub mod modals;
/// Collapsible left sidebar with session list and quick actions.
pub mod sidebar;
/// Human-readable tool invocation cards (diffs, bash output, file/search results).
pub mod tool_card;
/// Scrollable conversation transcript with bubbles, markdown, and collapsible tool cards.
pub mod transcript;

pub use layout::view;
