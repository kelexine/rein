// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: High-level ShellClassifier orchestrating script parsing, substitutions, and stage analysis

use crate::classifier::effect::{EffectLevel, ShellClassification, TargetScope};
use crate::classifier::parser::{ParsedCommand, RedirectKind, Redirection, ShellPipeline};
use crate::classifier::rules::classify_parsed_command;
use crate::classifier::scope::classify_path_token;
use crate::sandbox::WorkspaceSandbox;

/// Stateless analyzer that classifies both structured `(program, args)` and raw shell strings.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShellClassifier;

impl ShellClassifier {
    /// Classifies a structured `(program, args)` invocation without a workspace sandbox context.
    #[must_use]
    pub fn classify(program: &str, args: &[String]) -> ShellClassification {
        Self::classify_with_sandbox(program, args, None)
    }

    /// Classifies a structured `(program, args)` invocation with a [`WorkspaceSandbox`] for symlink/path resolution.
    #[must_use]
    pub fn classify_with_sandbox(
        program: &str,
        args: &[String],
        sandbox: Option<&WorkspaceSandbox>,
    ) -> ShellClassification {
        let base = program.rsplit('/').next().unwrap_or(program);
        if matches!(base, "sh" | "bash" | "zsh")
            && let Some(script) = args
                .windows(2)
                .find(|w| w[0] == "-c")
                .map(|w| w[1].as_str())
        {
            return Self::classify_script_with_sandbox(script, sandbox);
        }
        let cmd = ParsedCommand::from_words(program.to_owned(), args.to_vec(), Vec::new());
        classify_parsed_command(&cmd, sandbox)
    }

    /// Classifies a raw shell command string without a workspace sandbox context.
    #[must_use]
    pub fn classify_script(script: &str) -> ShellClassification {
        Self::classify_script_with_sandbox(script, None)
    }

    /// Classifies a raw shell command string with an optional [`WorkspaceSandbox`] for symlink/path checks.
    #[must_use]
    pub fn classify_script_with_sandbox(
        script: &str,
        sandbox: Option<&WorkspaceSandbox>,
    ) -> ShellClassification {
        let Ok(pipeline) = ShellPipeline::parse(script) else {
            return ShellClassification::new(
                EffectLevel::Unknown,
                "unparseable or unterminated shell syntax (failing closed)",
            );
        };
        if pipeline.dynamic_command {
            return ShellClassification::new(
                EffectLevel::Unknown,
                "dynamic command substitution in command or subscript position",
            );
        }
        if pipeline.stages.is_empty()
            && pipeline.redirections.is_empty()
            && pipeline.substitutions.is_empty()
        {
            return ShellClassification::new(EffectLevel::Unknown, "empty shell command");
        }

        let mut acc = ShellClassification::new(EffectLevel::ReadOnly, "read-only pipeline");
        for sub in &pipeline.substitutions {
            acc = acc.merge(Self::classify_script_with_sandbox(sub, sandbox));
        }
        for redir in &pipeline.redirections {
            acc = acc.merge(classify_redirection(redir, sandbox));
        }
        for stage in &pipeline.stages {
            acc = acc.merge(classify_parsed_command(stage, sandbox));
        }
        acc
    }

    /// Returns `true` when the entire command consists of safe `git` repository operations (without `git config`, dangerous `-c` keys, or output file redirections).
    #[must_use]
    pub fn allows_git_writes(
        program: &str,
        args: &[String],
        sandbox: Option<&WorkspaceSandbox>,
    ) -> bool {
        let base = program.rsplit('/').next().unwrap_or(program);
        if matches!(base, "sh" | "bash" | "zsh")
            && let Some(script) = args
                .windows(2)
                .find(|w| w[0] == "-c")
                .map(|w| w[1].as_str())
        {
            let Ok(pipeline) = ShellPipeline::parse(script) else {
                return false;
            };
            if pipeline.dynamic_command
                || !pipeline.substitutions.is_empty()
                || pipeline.stages.is_empty()
                || pipeline
                    .redirections
                    .iter()
                    .any(|r| r.kind == RedirectKind::Output)
            {
                return false;
            }
            let mut saw_git_mutation = false;
            for stage in &pipeline.stages {
                if matches!(stage.binary.as_str(), "echo" | "printf" | "true" | "pwd") {
                    continue;
                }
                if !is_safe_git_stage(stage, sandbox) {
                    return false;
                }
                if is_git_mutation_subcommand(stage) {
                    saw_git_mutation = true;
                }
            }
            return saw_git_mutation;
        }
        let cmd = ParsedCommand::from_words(program.to_owned(), args.to_vec(), Vec::new());
        is_safe_git_stage(&cmd, sandbox) && is_git_mutation_subcommand(&cmd)
    }
}

fn is_safe_git_stage(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> bool {
    if cmd.binary != "git" || cmd.has_var_expansion {
        return false;
    }
    if cmd.has_long_prefix("exec-path")
        || cmd.has_long_prefix("upload-pack")
        || cmd.has_long_prefix("receive-pack")
        || cmd.has_long_prefix("open-files-in-pager")
    {
        return false;
    }
    let Some(sub) = cmd.positionals.first().map(String::as_str) else {
        return false;
    };
    if sub == "config" {
        return false;
    }
    let mut iter = cmd.raw_args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-c" {
            let Some(kv) = iter.next() else {
                return false;
            };
            if is_dangerous_git_config_kv(kv) {
                return false;
            }
        } else if let Some(kv) = arg.strip_prefix("-c")
            && !kv.is_empty()
            && is_dangerous_git_config_kv(kv)
        {
            return false;
        } else if matches!(arg.as_str(), "-C" | "--git-dir" | "--work-tree")
            && let Some(val) = iter.next()
            && classify_path_token(val, sandbox) != TargetScope::Workspace
        {
            return false;
        }
    }
    true
}

fn is_dangerous_git_config_kv(kv: &str) -> bool {
    let key = kv
        .split('=')
        .next()
        .unwrap_or(kv)
        .trim()
        .to_ascii_lowercase();
    key.starts_with("core.fsmonitor")
        || key.starts_with("core.hookspath")
        || key.starts_with("core.pager")
        || key.starts_with("core.sshcommand")
        || key.starts_with("core.askpass")
        || key.starts_with("credential.")
        || key.starts_with("diff.external")
        || key.starts_with("merge.")
        || key.starts_with("filter.")
        || key.starts_with("alias.")
        || key.starts_with("uploadpack.")
        || key.starts_with("receivepack.")
}

fn is_git_mutation_subcommand(cmd: &ParsedCommand) -> bool {
    let Some(sub) = cmd.positionals.first().map(String::as_str) else {
        return false;
    };
    matches!(
        sub,
        "add"
            | "commit"
            | "checkout"
            | "switch"
            | "branch"
            | "tag"
            | "reset"
            | "restore"
            | "stash"
            | "merge"
            | "rebase"
            | "cherry-pick"
            | "rm"
            | "mv"
            | "am"
            | "apply"
            | "fetch"
            | "pull"
            | "push"
    )
}

fn classify_redirection(
    redir: &Redirection,
    sandbox: Option<&WorkspaceSandbox>,
) -> ShellClassification {
    match redir.kind {
        RedirectKind::HereString => {
            ShellClassification::new(EffectLevel::ReadOnly, "here-string input")
        }
        RedirectKind::Input => {
            let scope = classify_path_token(&redir.target, sandbox);
            ShellClassification::with_target(EffectLevel::ReadOnly, scope, "input file redirection")
        }
        RedirectKind::Output => {
            if redir.target == "/dev/null" {
                return ShellClassification::new(EffectLevel::ReadOnly, "redirect to /dev/null");
            }
            if redir.target.starts_with("/dev/tcp/") || redir.target.starts_with("/dev/udp/") {
                return ShellClassification::new(
                    EffectLevel::Network,
                    "bash /dev/tcp or /dev/udp network redirection",
                );
            }
            let scope = classify_path_token(&redir.target, sandbox);
            let eff = if scope == TargetScope::OutsideWorkspace {
                EffectLevel::OutsideWrite
            } else {
                EffectLevel::WorkspaceWrite
            };
            ShellClassification::with_target(eff, scope, "output file redirection")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_redirections_bash_c_empty_and_unparseable_scripts() {
        let dev_null = ShellClassifier::classify_script("cargo check 2>/dev/null");
        assert!(dev_null.is_read_only() || dev_null.effect == EffectLevel::Exec);

        let ls_null = ShellClassifier::classify_script("ls -la > /dev/null");
        assert_eq!(ls_null.effect, EffectLevel::ReadOnly);

        let dev_tcp = ShellClassifier::classify_script("cat < /dev/null > /dev/tcp/127.0.0.1/80");
        assert_eq!(dev_tcp.effect, EffectLevel::Network);

        let via_bash = ShellClassifier::classify(
            "/bin/bash",
            &["-c".to_owned(), "git status && rg TODO src".to_owned()],
        );
        assert_eq!(via_bash.effect, EffectLevel::ReadOnly);
        assert_eq!(via_bash.target, TargetScope::Workspace);

        let empty = ShellClassifier::classify_script("   ");
        assert_eq!(empty.effect, EffectLevel::Unknown);

        let unclosed = ShellClassifier::classify_script("echo 'unterminated");
        assert_eq!(unclosed.effect, EffectLevel::Unknown);

        let dyn_sub = ShellClassifier::classify_script("$(echo ls)");
        assert_eq!(dyn_sub.effect, EffectLevel::Unknown);

        assert!(ShellClassifier::allows_git_writes(
            "bash",
            &[
                "-c".to_owned(),
                "git -c core.excludesFile=/dev/null add .github/workflows/ci.yml && git commit -s -m \"ci: test\"".to_owned(),
            ],
            None
        ));
        assert!(!ShellClassifier::allows_git_writes(
            "bash",
            &["-c".to_owned(), "cat >> .git/config".to_owned()],
            None
        ));
        assert!(!ShellClassifier::allows_git_writes(
            "bash",
            &[
                "-c".to_owned(),
                "git -c core.fsmonitor=pwn add .".to_owned(),
            ],
            None
        ));
        assert!(!ShellClassifier::allows_git_writes(
            "bash",
            &["-c".to_owned(), "git config filter.x.clean pwn".to_owned(),],
            None
        ));
    }
}
