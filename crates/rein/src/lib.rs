// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-18
// Purpose: Library surface for Rein exposing agent loop, modes, config, commands, memory, and UI

//! Rein library surface for the CLI entrypoint and integration tests.
#![forbid(unsafe_code)]

/// In-process event-sourced agent loop.
pub mod agent;
/// Built-in slash commands and `~/.rein/skills` + `<workspace>/.rein/skills` loader.
pub mod commands;
/// Dual-scope (`~/.rein` and `<workspace>/.rein`) config and command allowlist loader.
pub mod config;
/// Automatic memory consolidation (`AutoDream`), vault tools, and Ebbinghaus salience decay.
pub mod memory;
/// Mode profiles, context strategy, and runtime switcher.
pub mod modes;
/// Claude-compatible plugin and marketplace manager (`~/.rein/plugins`).
pub mod plugins;
/// In-process Ratatui terminal interface.
pub mod ui;
