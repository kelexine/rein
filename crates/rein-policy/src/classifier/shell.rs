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
