// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Built-in slash command registry, prefix matcher, and SKILL.md activation prompt builder

use std::path::PathBuf;

/// Origin of a slash entry shown as `(user)` or `(repo)` in the autocomplete menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// Built directly into Rein.
    Builtin,
    /// Loaded from `~/.rein/skills/`.
    User,
    /// Loaded from `<workspace>/.rein/skills/`.
    Repo,
}

impl CommandSource {
    /// Returns the short menu badge (`user` or `repo`) for skills.
    #[must_use]
    pub fn badge(self) -> Option<&'static str> {
        match self {
            Self::Builtin => None,
            Self::User => Some("user"),
            Self::Repo => Some("repo"),
        }
    }
}

/// Built-in interactive actions executed directly in-process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAction {
    /// Lists commands and keyboard shortcuts.
    Help,
    /// Clears display transcript and active in-memory history.
    Clear,
    /// Summarizes active conversation via the provider while preserving the session's `<uid>.full.jsonl` archive.
    Compact,
    /// Queries project or personal memory records (`[project|personal] [query]`).
    Memory,
    /// Lists pre-approved shell commands from global and repo `commands.json`.
    Commands,
    /// Lists configured MCP servers and active tool adapters.
    Mcp,
    /// Authenticates directly with Antigravity via browser OAuth 2.0.
    Login,
    /// Signs out and removes saved Antigravity OAuth credentials.
    Logout,
    /// Displays interactive model picker or switches model (`[provider/model|model]`).
    Model,
    /// Displays or switches the active model provider (`[anthropic|antigravity] [model]`).
    Provider,
    /// Opens an interactive picker of saved sessions in the current directory.
    Resume,
    /// Rewinds conversation history and optionally workspace code to a previous turn.
    Rewind,
    /// Exits Rein cleanly.
    Quit,
}

/// Execution behavior backing a registered [`SlashCommand`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashKind {
    /// Native in-process Rein command.
    Builtin(BuiltinAction),
    /// User/repo skill activated by instructing the model to read `SKILL.md` first.
    Skill {
        /// Path to the skill's `SKILL.md` file.
        skill_md_path: PathBuf,
    },
}

/// A single registered built-in `/command` or `/skill` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    /// Command or skill name without leading `/`.
    pub name: String,
    /// Concise 1-line description rendered in the autocomplete menu.
    pub description: String,
    /// Origin scope (`Builtin`, `User`, or `Repo`).
    pub source: CommandSource,
    /// Execution behavior (`Builtin` or `Skill`).
    pub kind: SlashKind,
}

impl SlashCommand {
    /// Expands an activation prompt for a skill command instructing the model to read `SKILL.md`.
    #[must_use]
    pub fn expand_skill_prompt(&self, args: &str) -> Option<String> {
        let trimmed = args.trim();
        match &self.kind {
            SlashKind::Builtin(_) => None,
            SlashKind::Skill { skill_md_path } => {
                let base = format!(
                    "Activate the `{}` skill by first reading `{}` using the `read` tool, then follow its instructions.",
                    self.name,
                    skill_md_path.display()
                );
                if trimmed.is_empty() {
                    Some(base)
                } else {
                    Some(format!("{base}\n\nTask: {trimmed}"))
                }
            }
        }
    }
}

/// Result of resolving an entered `/command ...` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashResolution<'a> {
    /// Matched command or skill with raw trailing argument string.
    Matched {
        /// Resolved entry.
        command: &'a SlashCommand,
        /// Trailing argument text after the `/command` token.
        args: String,
    },
    /// Unknown `/...` command name.
    Unknown(String),
}

/// Registry holding built-in commands plus `~/.rein/skills/` and `.rein/skills/` entries.
#[derive(Debug, Clone)]
pub struct SlashRegistry {
    commands: Vec<SlashCommand>,
}

impl Default for SlashRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl SlashRegistry {
    /// Initializes a registry populated with all built-in Rein slash commands.
    #[must_use]
    pub fn with_builtins() -> Self {
        let commands = vec![
            builtin(
                "help",
                "Show built-in slash commands and keyboard shortcuts",
                BuiltinAction::Help,
            ),
            builtin(
                "clear",
                "Clear transcript and active in-memory history",
                BuiltinAction::Clear,
            ),
            builtin(
                "compact",
                "Summarize active history while keeping session full transcript",
                BuiltinAction::Compact,
            ),
            builtin(
                "memory",
                "Inspect scoped memory ([project|personal] [query])",
                BuiltinAction::Memory,
            ),
            builtin(
                "commands",
                "List allowlisted shell commands from commands.json",
                BuiltinAction::Commands,
            ),
            builtin(
                "mcp",
                "Show configured MCP servers and active tools",
                BuiltinAction::Mcp,
            ),
            builtin(
                "login",
                "Authenticate directly with Google Antigravity OAuth",
                BuiltinAction::Login,
            ),
            builtin(
                "logout",
                "Sign out and remove saved Google Antigravity OAuth token",
                BuiltinAction::Logout,
            ),
            builtin(
                "model",
                "Select or switch active model ([model|provider/model])",
                BuiltinAction::Model,
            ),
            builtin(
                "provider",
                "Show or switch provider/model ([anthropic|antigravity] [model])",
                BuiltinAction::Provider,
            ),
            builtin(
                "resume",
                "Interactively pick and resume a saved session in this directory",
                BuiltinAction::Resume,
            ),
            builtin(
                "rewind",
                "Rewind conversation to a previous message",
                BuiltinAction::Rewind,
            ),
            builtin(
                "exit",
                "Exit Rein (or press Ctrl+C twice within 2s)",
                BuiltinAction::Quit,
            ),
        ];
        Self { commands }
    }

    /// Inserts or overrides a skill entry by `name` (repo skills override user skills).
    pub fn register(&mut self, cmd: SlashCommand) {
        if let Some(existing) = self.commands.iter_mut().find(|c| c.name == cmd.name) {
            if !matches!(existing.kind, SlashKind::Builtin(_)) {
                *existing = cmd;
            }
        } else {
            self.commands.push(cmd);
        }
    }

    /// Returns all registered slash entries.
    #[must_use]
    pub fn all(&self) -> &[SlashCommand] {
        &self.commands
    }

    /// Returns entries matching the `/` token being typed (hidden once args start).
    #[must_use]
    pub fn matching(&self, input: &str) -> Vec<&SlashCommand> {
        let Some(rest) = input.strip_prefix('/') else {
            return Vec::new();
        };
        if rest.contains(char::is_whitespace) {
            return Vec::new();
        }
        let needle = rest.to_ascii_lowercase();
        let mut prefix_hits = Vec::new();
        let mut contains_hits = Vec::new();
        for cmd in &self.commands {
            let lower = cmd.name.to_ascii_lowercase();
            if lower.starts_with(&needle) {
                prefix_hits.push(cmd);
            } else if !needle.is_empty() && lower.contains(&needle) {
                contains_hits.push(cmd);
            }
        }
        prefix_hits.extend(contains_hits);
        prefix_hits
    }

    /// Resolves a submitted `/...` input string against built-ins and skills (plus `/quit` alias).
    #[must_use]
    pub fn resolve<'a>(&'a self, input: &str) -> SlashResolution<'a> {
        let rest = input.trim().trim_start_matches('/');
        let mut parts = rest.splitn(2, char::is_whitespace);
        let raw_name = parts.next().unwrap_or_default().trim();
        let args = parts.next().unwrap_or_default().trim().to_owned();
        let lookup = if raw_name == "quit" { "exit" } else { raw_name };

        match self.commands.iter().find(|c| c.name == lookup) {
            Some(command) => SlashResolution::Matched { command, args },
            None => SlashResolution::Unknown(raw_name.to_owned()),
        }
    }
}

fn builtin(name: &str, description: &str, action: BuiltinAction) -> SlashCommand {
    SlashCommand {
        name: name.to_owned(),
        description: description.to_owned(),
        source: CommandSource::Builtin,
        kind: SlashKind::Builtin(action),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_prefix_and_builds_skill_activation_prompt() {
        let mut reg = SlashRegistry::with_builtins();
        reg.register(SlashCommand {
            name: "linux-perf".to_owned(),
            description: "Profile with perf".to_owned(),
            source: CommandSource::Repo,
            kind: SlashKind::Skill {
                skill_md_path: PathBuf::from("/tmp/skills/linux-perf/SKILL.md"),
            },
        });

        let m_hits: Vec<_> = reg
            .matching("/m")
            .into_iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            m_hits,
            vec!["memory", "mcp", "model", "compact", "commands", "resume"]
        );
        assert!(reg.matching("/memory project").is_empty());

        let SlashResolution::Matched { command, args } = reg.resolve("/linux-perf hotspot") else {
            panic!("expected linux-perf match");
        };
        let prompt = command.expand_skill_prompt(&args).unwrap_or_default();
        assert!(prompt.contains("/tmp/skills/linux-perf/SKILL.md"));
        assert!(prompt.contains("read"));
        assert!(prompt.contains("Task: hotspot"));
    }

    #[test]
    fn resolves_builtin_commands_quit_alias_and_unknown_commands() {
        let reg = SlashRegistry::with_builtins();

        assert_eq!(CommandSource::Builtin.badge(), None);
        assert_eq!(CommandSource::User.badge(), Some("user"));
        assert_eq!(CommandSource::Repo.badge(), Some("repo"));
        assert!(reg.matching("plain text").is_empty());
        assert!(!reg.matching("/").is_empty());

        assert!(matches!(
            reg.resolve("/quit"),
            SlashResolution::Matched {
                command: SlashCommand {
                    kind: SlashKind::Builtin(BuiltinAction::Quit),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            reg.resolve("/exit"),
            SlashResolution::Matched {
                command: SlashCommand {
                    kind: SlashKind::Builtin(BuiltinAction::Quit),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            reg.resolve("/provider antigravity"),
            SlashResolution::Matched {
                command: SlashCommand {
                    kind: SlashKind::Builtin(BuiltinAction::Provider),
                    ..
                },
                ref args,
            } if args == "antigravity"
        ));
        assert!(matches!(
            reg.resolve("/compact"),
            SlashResolution::Matched {
                command: SlashCommand {
                    kind: SlashKind::Builtin(BuiltinAction::Compact),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            reg.resolve("/resume"),
            SlashResolution::Matched {
                command: SlashCommand {
                    kind: SlashKind::Builtin(BuiltinAction::Resume),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            reg.resolve("/nonexistent arg"),
            SlashResolution::Unknown(ref name) if name == "nonexistent"
        ));
    }
}
