// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Command allow-pattern suggestion and pipeline stage matching helpers

use crate::classifier::{
    EffectLevel, ParsedCommand, ShellClassification, ShellPipeline, TargetScope,
    classify_parsed_command, classify_path_token, has_dangerous_env_var,
};
use crate::sandbox::WorkspaceSandbox;

use super::evaluator::MULTI_SUBCOMMAND_TOOLS;

/// Suggests a concise excerpt and a least-privilege allowlist pattern for a shell invocation.
#[must_use]
pub fn suggest_command_allow_pattern(
    program: &str,
    args: &[String],
    sandbox: &WorkspaceSandbox,
) -> (String, String) {
    let Some(stages) = extract_stages(program, args, sandbox) else {
        let fallback = format_command_tokens(program, args);
        return (truncate_excerpt(&fallback), fallback);
    };
    if stages.is_empty() {
        let fallback = format_command_tokens(program, args);
        return (truncate_excerpt(&fallback), fallback);
    }
    let joined = stages
        .iter()
        .map(stage_text)
        .collect::<Vec<_>>()
        .join(" && ");
    let excerpt = truncate_excerpt(&joined);

    let actionable: Vec<&ParsedCommand> = stages
        .iter()
        .filter(|stage| {
            let class = classify_parsed_command(stage, Some(sandbox));
            !stage_is_auto_allowed(stage, &class)
        })
        .collect();
    if actionable.len() == 1
        && let Some(&stage) = actionable.first()
    {
        let class = classify_parsed_command(stage, Some(sandbox));
        let base = basename(&stage.program);
        if is_workspace_safe_stage(stage, &class) && MULTI_SUBCOMMAND_TOOLS.contains(&base) {
            return (excerpt, format!("{base} *"));
        }
        return (excerpt, stage_text(stage));
    }
    let actionable_joined = if actionable.is_empty() {
        joined
    } else {
        actionable
            .iter()
            .map(|s| stage_text(s))
            .collect::<Vec<_>>()
            .join(" && ")
    };
    (excerpt, actionable_joined)
}

pub(super) fn stage_is_auto_allowed(stage: &ParsedCommand, class: &ShellClassification) -> bool {
    class.effect == EffectLevel::ReadOnly
        && class.target == TargetScope::Workspace
        && !has_dangerous_env_var(&stage.env_vars)
}

fn is_workspace_safe_stage(stage: &ParsedCommand, class: &ShellClassification) -> bool {
    if class.target != TargetScope::Workspace {
        return false;
    }
    if matches!(
        class.effect,
        EffectLevel::OutsideWrite | EffectLevel::Network | EffectLevel::Unknown
    ) {
        return false;
    }
    let base = basename(&stage.program);
    if base == "git" && class.effect == EffectLevel::Exec {
        return false;
    }
    if base == "go"
        && stage
            .raw_args
            .iter()
            .any(|a| a.starts_with("-vettool") || a == "-exec" || a.starts_with("-exec="))
    {
        return false;
    }
    true
}

fn format_command_tokens(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_owned()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

fn truncate_excerpt(raw: &str) -> String {
    let single_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_LEN: usize = 72;
    if single_line.chars().count() <= MAX_LEN {
        single_line
    } else {
        let head: String = single_line.chars().take(MAX_LEN - 3).collect();
        format!("{head}...")
    }
}

pub(super) fn extract_stages(
    program: &str,
    args: &[String],
    sandbox: &WorkspaceSandbox,
) -> Option<Vec<ParsedCommand>> {
    let base = basename(program);
    if matches!(base, "sh" | "bash" | "zsh") {
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == "-c" {
                let script = iter.next()?;
                let pipeline = ShellPipeline::parse(script).ok()?;
                if pipeline.dynamic_command || !pipeline.substitutions.is_empty() {
                    return None;
                }
                let bad_redirect = pipeline.redirections.iter().any(|r| {
                    classify_path_token(&r.target, Some(sandbox)) != TargetScope::Workspace
                });
                if bad_redirect {
                    return None;
                }
                return Some(pipeline.stages);
            }
        }
    }
    Some(vec![ParsedCommand::from_words(
        program.to_owned(),
        args.to_vec(),
        Vec::new(),
    )])
}

fn stage_text(stage: &ParsedCommand) -> String {
    std::iter::once(stage.program.as_str())
        .chain(stage.raw_args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn basename(program: &str) -> &str {
    std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
}

pub(super) fn stage_matches_pattern(
    stage: &ParsedCommand,
    class: &ShellClassification,
    pattern: &str,
) -> bool {
    if class.effect >= EffectLevel::Unknown
        || class.target == TargetScope::Protected
        || has_dangerous_env_var(&stage.env_vars)
    {
        return false;
    }
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    let stage_tokens: Vec<&str> = std::iter::once(stage.program.as_str())
        .chain(stage.raw_args.iter().map(String::as_str))
        .collect();
    let workspace_safe = is_workspace_safe_stage(stage, class);

    if let Some(prefix) = pattern.strip_suffix(" *") {
        if !workspace_safe {
            return false;
        }
        let prefix_tokens: Vec<&str> = prefix.split_whitespace().collect();
        return tokens_start_with(&stage_tokens, &prefix_tokens);
    }

    let pat_tokens: Vec<&str> = pattern.split_whitespace().collect();
    if pat_tokens.len() == 1 {
        if !workspace_safe {
            return stage_tokens.len() == 1 && basename(stage_tokens[0]) == pat_tokens[0];
        }
        return !stage_tokens.is_empty() && basename(stage_tokens[0]) == pat_tokens[0];
    }

    if stage_tokens.len() == pat_tokens.len() {
        return tokens_start_with(&stage_tokens, &pat_tokens);
    }

    workspace_safe && tokens_start_with(&stage_tokens, &pat_tokens)
}

fn tokens_start_with(stage_tokens: &[&str], pattern_tokens: &[&str]) -> bool {
    if pattern_tokens.is_empty() || stage_tokens.len() < pattern_tokens.len() {
        return false;
    }
    stage_tokens[0] == pattern_tokens[0]
        && stage_tokens[1..pattern_tokens.len()] == pattern_tokens[1..]
}
