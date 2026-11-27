// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-27
// Purpose: Structured CLI argument parser supporting one-shot prompts, config, models, auth, sessions, allowlist, mcp, plugins, and doctor

use std::path::PathBuf;

use anyhow::{Result, bail};
use rein_core::ModeId;

use crate::config::ProviderKind;

/// Options for non-interactive one-shot prompt execution (`rein -p "<prompt>"` or piped stdin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneShotOpts {
    /// Prompt text to execute.
    pub prompt: String,
    /// Optional operating mode override (`coding`, `plan`, `personal`).
    pub mode: Option<ModeId>,
    /// Optional provider override (`anthropic`, `antigravity`).
    pub provider: Option<ProviderKind>,
    /// Optional model ID override.
    pub model: Option<String>,
    /// Emit structured JSON output instead of plain text.
    pub json: bool,
    /// Auto-approve all tool calls (`--yolo` / `--dangerously-bypass-permission`).
    pub bypass_permissions: bool,
}

/// Actions for `rein config ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigAction {
    /// List all effective configuration settings (`--json` optional).
    List {
        /// Output as JSON.
        json: bool,
    },
    /// Print the value of a single key (`provider`, `model`, `settings.thinking_mode`, `dream.enabled`, etc.).
    Get {
        /// Dotted or top-level key name.
        key: String,
    },
    /// Persist a configuration key in global (`~/.rein/config.json`) or workspace (`.rein/config.json`) scope.
    Set {
        /// Dotted or top-level key name.
        key: String,
        /// New string/bool/integer value.
        value: String,
        /// Write to `<workspace>/.rein/config.json` instead of `~/.rein/config.json`.
        repo: bool,
    },
    /// Show resolved filesystem paths for global and workspace configurations.
    Path,
}

/// Actions for `rein models ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsAction {
    /// List cached or live-refreshed models for the active or specified provider.
    List {
        /// Force a live network refresh from the provider API.
        refresh: bool,
        /// Optional provider filter.
        provider: Option<ProviderKind>,
    },
    /// Set the active provider/model (`<provider>/<model>` or `<model>`).
    Set {
        /// Target model specification.
        spec: String,
    },
}

/// Actions for `rein auth ...` (and `rein login`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthAction {
    /// Run interactive Google OAuth browser sign-in for Antigravity.
    Login,
    /// Show credential status for both Anthropic and Antigravity providers.
    Status,
}

/// Actions for `rein sessions ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionsAction {
    /// List saved sessions in the current workspace.
    List,
    /// Display transcript turns for a saved session UID or prefix.
    Show {
        /// Session UID or prefix.
        uid: String,
    },
    /// Delete a saved session by UID or prefix.
    Delete {
        /// Session UID or prefix.
        uid: String,
    },
}

/// Actions for `rein allowlist ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowlistAction {
    /// List both workspace and global allowed command patterns.
    List,
    /// Add a command pattern to workspace (default) or global (`--global`) allowlist.
    Add {
        /// Pattern such as `cargo *` or `tool:write`.
        pattern: String,
        /// Persist to `~/.rein/commands.json` instead of workspace allowlist.
        global: bool,
    },
    /// Remove a command pattern from workspace (default) or global (`--global`) allowlist.
    Remove {
        /// Pattern to remove.
        pattern: String,
        /// Remove from `~/.rein/commands.json` instead of workspace allowlist.
        global: bool,
    },
}

/// Actions for `rein mcp ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAction {
    /// List configured stdio MCP servers.
    List,
    /// Add a stdio MCP server to `~/.rein/config.json`.
    Add {
        /// Server identifier.
        name: String,
        /// Executable command.
        command: String,
        /// Command arguments.
        args: Vec<String>,
    },
    /// Remove a configured stdio MCP server by name.
    Remove {
        /// Server identifier.
        name: String,
    },
}

/// Actions for `rein plugins ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginsAction {
    /// List installed plugins and their enabled state.
    List,
    /// List registered plugin marketplaces.
    Marketplaces,
    /// Register a new marketplace (`owner/repo` or git URL).
    MarketAdd {
        /// Marketplace specification.
        spec: String,
    },
    /// Clone or `git pull` marketplaces (`None` refreshes all).
    Refresh {
        /// Optional marketplace name.
        name: Option<String>,
    },
    /// Install a plugin by name from synced marketplace catalogs.
    Install {
        /// Plugin name.
        name: String,
    },
    /// Toggle an installed plugin between enabled and disabled.
    Toggle {
        /// Plugin name.
        name: String,
    },
    /// Uninstall an installed plugin.
    Uninstall {
        /// Plugin name.
        name: String,
    },
}

/// Top-level parsed CLI command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// Launch the interactive Ratatui TUI (default when no subcommand or `-p` is given).
    InteractiveTui,
    /// Print `--help` usage reference.
    Help,
    /// Print `--version`.
    Version,
    /// Execute a single non-interactive prompt (`-p` / `--print`).
    OneShot(OneShotOpts),
    /// Replay a recorded JSONL session event log.
    Replay {
        /// Path to session JSONL log.
        path: PathBuf,
        /// Whether `--yolo` is set.
        bypass_permissions: bool,
    },
    /// Manage configuration (`rein config ...`).
    Config(ConfigAction),
    /// Manage or inspect models (`rein models ...`).
    Models(ModelsAction),
    /// Manage or inspect authentication (`rein auth ...` or `rein login`).
    Auth(AuthAction),
    /// Inspect or delete saved sessions (`rein sessions ...`).
    Sessions(SessionsAction),
    /// Inspect or edit command allowlists (`rein allowlist ...`).
    Allowlist(AllowlistAction),
    /// Inspect or edit stdio MCP servers (`rein mcp ...`).
    Mcp(McpAction),
    /// Inspect or manage Claude-compatible plugins (`rein plugins ...`).
    Plugins(PluginsAction),
    /// Run environment, sandbox, provider, and config diagnostics (`rein doctor`).
    Doctor,
}

/// Parses `std::env::args()` into a structured [`CliCommand`].
pub fn parse_cli_args(args: &[String]) -> Result<CliCommand> {
    let tail = if args.is_empty() { &[] } else { &args[1..] };
    if tail.is_empty() {
        return Ok(CliCommand::InteractiveTui);
    }

    let first = tail[0].as_str();
    match first {
        "-h" | "--help" | "help" => return Ok(CliCommand::Help),
        "-V" | "--version" | "version" => return Ok(CliCommand::Version),
        "login" => return Ok(CliCommand::Auth(AuthAction::Login)),
        "auth" => return parse_auth_subcommand(&tail[1..]),
        "config" => return parse_config_subcommand(&tail[1..]),
        "models" | "model" => return parse_models_subcommand(&tail[1..]),
        "sessions" | "session" => return parse_sessions_subcommand(&tail[1..]),
        "allowlist" | "commands" => return parse_allowlist_subcommand(&tail[1..]),
        "mcp" => return parse_mcp_subcommand(&tail[1..]),
        "plugins" | "plugin" => return parse_plugins_subcommand(&tail[1..]),
        "doctor" => return Ok(CliCommand::Doctor),
        "replay" => {
            let Some(path) = tail.get(1) else {
                bail!("usage: rein replay <path-to-session.jsonl>");
            };
            let bypass = tail
                .iter()
                .any(|a| a == "--yolo" || a == "--dangerously-bypass-permission");
            return Ok(CliCommand::Replay {
                path: PathBuf::from(path),
                bypass_permissions: bypass,
            });
        }
        _ => {}
    }

    if let Some(opts) = try_parse_oneshot(tail)? {
        return Ok(CliCommand::OneShot(opts));
    }

    Ok(CliCommand::InteractiveTui)
}

fn try_parse_oneshot(tail: &[String]) -> Result<Option<OneShotOpts>> {
    let mut prompt: Option<String> = None;
    let mut mode: Option<ModeId> = None;
    let mut provider: Option<ProviderKind> = None;
    let mut model: Option<String> = None;
    let mut json = false;
    let mut bypass = false;

    let mut idx = 0;
    while idx < tail.len() {
        let arg = tail[idx].as_str();
        match arg {
            "-p" | "--print" | "--prompt" => {
                let Some(val) = tail.get(idx + 1) else {
                    bail!("{arg} requires a prompt string (usage: rein -p \"<prompt>\")");
                };
                prompt = Some(val.clone());
                idx += 2;
            }
            _ if arg.starts_with("--print=") || arg.starts_with("--prompt=") => {
                let (_, val) = arg.split_once('=').unwrap_or(("", ""));
                if val.trim().is_empty() {
                    bail!("--print requires a non-empty prompt string");
                }
                prompt = Some(val.to_owned());
                idx += 1;
            }
            "--mode" | "-m" => {
                let Some(val) = tail.get(idx + 1) else {
                    bail!("{arg} requires a mode (coding, plan, personal)");
                };
                mode = Some(parse_mode_id(val)?);
                idx += 2;
            }
            _ if arg.starts_with("--mode=") => {
                let (_, val) = arg.split_once('=').unwrap_or(("", ""));
                mode = Some(parse_mode_id(val)?);
                idx += 1;
            }
            "--provider" => {
                let Some(val) = tail.get(idx + 1) else {
                    bail!("--provider requires anthropic or antigravity");
                };
                provider = Some(
                    ProviderKind::parse(val)
                        .ok_or_else(|| anyhow::anyhow!("unknown provider '{val}'"))?,
                );
                idx += 2;
            }
            _ if arg.starts_with("--provider=") => {
                let (_, val) = arg.split_once('=').unwrap_or(("", ""));
                provider = Some(
                    ProviderKind::parse(val)
                        .ok_or_else(|| anyhow::anyhow!("unknown provider '{val}'"))?,
                );
                idx += 1;
            }
            "--model" => {
                let Some(val) = tail.get(idx + 1) else {
                    bail!("--model requires a model identifier");
                };
                model = Some(val.clone());
                idx += 2;
            }
            _ if arg.starts_with("--model=") => {
                let (_, val) = arg.split_once('=').unwrap_or(("", ""));
                model = Some(val.to_owned());
                idx += 1;
            }
            "--json" => {
                json = true;
                idx += 1;
            }
            "--yolo" | "--dangerously-bypass-permission" => {
                bypass = true;
                idx += 1;
            }
            _ => {
                idx += 1;
            }
        }
    }

    Ok(prompt.map(|p| OneShotOpts {
        prompt: p,
        mode,
        provider,
        model,
        json,
        bypass_permissions: bypass,
    }))
}

fn parse_mode_id(raw: &str) -> Result<ModeId> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "coding" | "code" => Ok(ModeId::Coding),
        "plan" => Ok(ModeId::Plan),
        "personal" => Ok(ModeId::Personal),
        other => bail!("unknown mode '{other}' (expected coding, plan, or personal)"),
    }
}

fn parse_auth_subcommand(sub: &[String]) -> Result<CliCommand> {
    match sub.first().map(String::as_str) {
        None | Some("status") => Ok(CliCommand::Auth(AuthAction::Status)),
        Some("login") => Ok(CliCommand::Auth(AuthAction::Login)),
        Some(other) => bail!("unknown auth subcommand '{other}' (expected `status` or `login`)"),
    }
}

fn parse_config_subcommand(sub: &[String]) -> Result<CliCommand> {
    let action = match sub.first().map(String::as_str) {
        None | Some("list") => ConfigAction::List {
            json: sub.iter().any(|a| a == "--json"),
        },
        Some("--json") => ConfigAction::List { json: true },
        Some("path" | "paths") => ConfigAction::Path,
        Some("get") => {
            let Some(key) = sub.get(1) else {
                bail!("usage: rein config get <key>");
            };
            ConfigAction::Get { key: key.clone() }
        }
        Some("set") => {
            let repo = sub.iter().any(|a| a == "--repo" || a == "--workspace");
            let positional: Vec<&String> = sub
                .iter()
                .skip(1)
                .filter(|a| *a != "--repo" && *a != "--workspace" && *a != "--global")
                .collect();
            let (Some(key), Some(val)) = (positional.first(), positional.get(1)) else {
                bail!("usage: rein config set <key> <value> [--repo]");
            };
            ConfigAction::Set {
                key: (*key).clone(),
                value: (*val).clone(),
                repo,
            }
        }
        Some(other) => {
            bail!("unknown config subcommand '{other}' (expected `list`, `get`, `set`, or `path`)")
        }
    };
    Ok(CliCommand::Config(action))
}

fn parse_models_subcommand(sub: &[String]) -> Result<CliCommand> {
    match sub.first().map(String::as_str) {
        Some("set" | "use") => {
            let Some(spec) = sub.get(1) else {
                bail!("usage: rein models set <provider/model>");
            };
            Ok(CliCommand::Models(ModelsAction::Set { spec: spec.clone() }))
        }
        _ => {
            let refresh = sub.iter().any(|a| a == "--refresh" || a == "-r");
            let mut provider = None;
            let mut i = 0;
            while i < sub.len() {
                if sub[i] == "--provider"
                    && let Some(val) = sub.get(i + 1)
                {
                    provider = ProviderKind::parse(val);
                    i += 2;
                    continue;
                }
                i += 1;
            }
            Ok(CliCommand::Models(ModelsAction::List { refresh, provider }))
        }
    }
}

fn parse_sessions_subcommand(sub: &[String]) -> Result<CliCommand> {
    let action = match sub.first().map(String::as_str) {
        None | Some("list" | "ls") => SessionsAction::List,
        Some("show" | "view") => {
            let Some(uid) = sub.get(1) else {
                bail!("usage: rein sessions show <uid>");
            };
            SessionsAction::Show { uid: uid.clone() }
        }
        Some("delete" | "rm") => {
            let Some(uid) = sub.get(1) else {
                bail!("usage: rein sessions delete <uid>");
            };
            SessionsAction::Delete { uid: uid.clone() }
        }
        Some(other) => bail!(
            "unknown sessions subcommand '{other}' (expected `list`, `show <uid>`, or `delete <uid>`)"
        ),
    };
    Ok(CliCommand::Sessions(action))
}

fn parse_allowlist_subcommand(sub: &[String]) -> Result<CliCommand> {
    let global = sub.iter().any(|a| a == "--global" || a == "-g");
    let action = match sub.first().map(String::as_str) {
        None | Some("list" | "ls") => AllowlistAction::List,
        Some("add") => {
            let pat = sub
                .iter()
                .skip(1)
                .find(|a| *a != "--global" && *a != "-g" && *a != "--repo")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("usage: rein allowlist add <pattern> [--global]"))?;
            AllowlistAction::Add {
                pattern: pat,
                global,
            }
        }
        Some("remove" | "rm") => {
            let pat = sub
                .iter()
                .skip(1)
                .find(|a| *a != "--global" && *a != "-g" && *a != "--repo")
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!("usage: rein allowlist remove <pattern> [--global]")
                })?;
            AllowlistAction::Remove {
                pattern: pat,
                global,
            }
        }
        Some(other) => bail!(
            "unknown allowlist subcommand '{other}' (expected `list`, `add <pattern>`, or `remove <pattern>`)"
        ),
    };
    Ok(CliCommand::Allowlist(action))
}

fn parse_mcp_subcommand(sub: &[String]) -> Result<CliCommand> {
    let action = match sub.first().map(String::as_str) {
        None | Some("list" | "ls") => McpAction::List,
        Some("add") => {
            let (Some(name), Some(command)) = (sub.get(1), sub.get(2)) else {
                bail!("usage: rein mcp add <name> <command> [args...]");
            };
            let args = sub.iter().skip(3).cloned().collect();
            McpAction::Add {
                name: name.clone(),
                command: command.clone(),
                args,
            }
        }
        Some("remove" | "rm") => {
            let Some(name) = sub.get(1) else {
                bail!("usage: rein mcp remove <name>");
            };
            McpAction::Remove { name: name.clone() }
        }
        Some(other) => bail!(
            "unknown mcp subcommand '{other}' (expected `list`, `add <name> <cmd> [args...]`, or `remove <name>`)"
        ),
    };
    Ok(CliCommand::Mcp(action))
}

fn parse_plugins_subcommand(sub: &[String]) -> Result<CliCommand> {
    let action = match sub.first().map(String::as_str) {
        None | Some("list" | "ls") => PluginsAction::List,
        Some("marketplaces" | "markets") => PluginsAction::Marketplaces,
        Some("market-add") => {
            let Some(spec) = sub.get(1) else {
                bail!("usage: rein plugins market-add <owner/repo|git-url>");
            };
            PluginsAction::MarketAdd { spec: spec.clone() }
        }
        Some("refresh" | "sync") => PluginsAction::Refresh {
            name: sub.get(1).cloned(),
        },
        Some("install") => {
            let Some(name) = sub.get(1) else {
                bail!("usage: rein plugins install <name>");
            };
            PluginsAction::Install { name: name.clone() }
        }
        Some("toggle") => {
            let Some(name) = sub.get(1) else {
                bail!("usage: rein plugins toggle <name>");
            };
            PluginsAction::Toggle { name: name.clone() }
        }
        Some("uninstall" | "rm") => {
            let Some(name) = sub.get(1) else {
                bail!("usage: rein plugins uninstall <name>");
            };
            PluginsAction::Uninstall { name: name.clone() }
        }
        Some(other) => bail!(
            "unknown plugins subcommand '{other}' (expected `list`, `marketplaces`, `market-add`, `refresh`, `install`, `toggle`, or `uninstall`)"
        ),
    };
    Ok(CliCommand::Plugins(action))
}

/// Returns the formatted `--version` string.
#[must_use]
pub fn format_version_text() -> String {
    format!("rein {}", env!("CARGO_PKG_VERSION"))
}

/// Returns the formatted `--help` usage documentation.
#[must_use]
pub fn format_help_text() -> &'static str {
    concat!(
        "Rein — Sandboxed, event-sourced AI coding and personal assistant\n\n",
        "USAGE:\n",
        "  rein [OPTIONS]\n",
        "  rein -p \"<prompt>\" [--mode coding|plan|personal] [--provider <p>] [--model <m>] [--json] [--yolo]\n",
        "  rein <SUBCOMMAND>\n\n",
        "OPTIONS:\n",
        "  -p, --print <PROMPT>              Execute a single prompt non-interactively and print the response\n",
        "  -m, --mode <MODE>                 Operating mode: coding, plan, or personal\n",
        "      --provider <PROVIDER>         Provider override: anthropic or antigravity\n",
        "      --model <MODEL>               Model identifier override\n",
        "      --resume <UID>                Resume a saved session by UID or unique prefix\n",
        "      --json                        Emit structured JSON in one-shot (-p) or `config list` mode\n",
        "      --yolo                        Auto-approve all policy checks (--dangerously-bypass-permission)\n",
        "  -h, --help                        Print this help message\n",
        "  -V, --version                     Print version information\n\n",
        "SUBCOMMANDS:\n",
        "  config list [--json]              Show all effective global + workspace settings\n",
        "  config get <key>                  Get a config value (e.g. provider, model, settings.thinking_mode)\n",
        "  config set <key> <val> [--repo]   Persist a config key globally (or in .rein/config.json with --repo)\n",
        "  config path                       Show resolved ~/.rein and <workspace>/.rein paths\n",
        "  models [list] [--refresh]         List cached or live models for the active provider\n",
        "  models set <provider/model>       Set the default provider and model\n",
        "  auth status | login               Check provider credentials or run Antigravity Google OAuth sign-in\n",
        "  login                             Alias for `rein auth login`\n",
        "  sessions list | show <uid> | delete <uid>\n",
        "                                    Inspect or remove saved workspace sessions\n",
        "  allowlist list | add <pat> [--global] | remove <pat> [--global]\n",
        "                                    Manage workspace and global shell/tool allowlist patterns\n",
        "  mcp list | add <name> <cmd> [args...] | remove <name>\n",
        "                                    Manage stdio Model Context Protocol (MCP) servers\n",
        "  plugins list | marketplaces | market-add <repo> | refresh [name] | install <name> | toggle <name> | uninstall <name>\n",
        "                                    Manage Claude-compatible plugins and git marketplaces\n",
        "  doctor                            Run environment, Landlock sandbox, credential, and MCP diagnostics\n",
        "  replay <session.jsonl>            Deterministically replay a recorded session log\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_help_version_and_interactive_defaults() -> Result<()> {
        assert_eq!(
            parse_cli_args(&vec_args(&["rein"]))?,
            CliCommand::InteractiveTui
        );
        assert_eq!(
            parse_cli_args(&vec_args(&["rein", "--help"]))?,
            CliCommand::Help
        );
        assert_eq!(
            parse_cli_args(&vec_args(&["rein", "-V"]))?,
            CliCommand::Version
        );
        assert_eq!(
            parse_cli_args(&vec_args(&["rein", "--resume", "a1b2c3d4"]))?,
            CliCommand::InteractiveTui
        );
        Ok(())
    }

    #[test]
    fn parses_oneshot_prompt_and_subcommands() -> Result<()> {
        let oneshot = parse_cli_args(&vec_args(&[
            "rein",
            "-p",
            "summarize Cargo.toml",
            "--mode",
            "plan",
            "--provider",
            "antigravity",
            "--model",
            "gemini-2.5-pro",
            "--json",
            "--yolo",
        ]))?;
        assert_eq!(
            oneshot,
            CliCommand::OneShot(OneShotOpts {
                prompt: "summarize Cargo.toml".to_owned(),
                mode: Some(ModeId::Plan),
                provider: Some(ProviderKind::Antigravity),
                model: Some("gemini-2.5-pro".to_owned()),
                json: true,
                bypass_permissions: true,
            })
        );

        let cfg_set = parse_cli_args(&vec_args(&[
            "rein",
            "config",
            "set",
            "settings.thinking_mode",
            "false",
        ]))?;
        assert_eq!(
            cfg_set,
            CliCommand::Config(ConfigAction::Set {
                key: "settings.thinking_mode".to_owned(),
                value: "false".to_owned(),
                repo: false,
            })
        );

        let allow_add = parse_cli_args(&vec_args(&[
            "rein",
            "allowlist",
            "add",
            "cargo *",
            "--global",
        ]))?;
        assert_eq!(
            allow_add,
            CliCommand::Allowlist(AllowlistAction::Add {
                pattern: "cargo *".to_owned(),
                global: true,
            })
        );

        let mcp_add = parse_cli_args(&vec_args(&[
            "rein",
            "mcp",
            "add",
            "filesystem",
            "npx",
            "-y",
            "@modelcontextprotocol/server-filesystem",
        ]))?;
        assert_eq!(
            mcp_add,
            CliCommand::Mcp(McpAction::Add {
                name: "filesystem".to_owned(),
                command: "npx".to_owned(),
                args: vec![
                    "-y".to_owned(),
                    "@modelcontextprotocol/server-filesystem".to_owned(),
                ],
            })
        );
        Ok(())
    }
}
