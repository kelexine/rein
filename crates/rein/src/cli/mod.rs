// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Comprehensive CLI subcommand parser and executor module exports

/// CLI argument parser, flags, and help/version renderers.
pub mod args;
/// Subcommand and one-shot prompt executors.
pub mod commands;

pub use args::{
    AllowlistAction, AuthAction, CliCommand, ConfigAction, McpAction, ModelsAction, OneShotOpts,
    PluginsAction, SessionsAction, format_help_text, format_version_text, parse_cli_args,
};
pub use commands::execute_cli_command;
