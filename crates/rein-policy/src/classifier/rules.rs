// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Command dispatcher, env-var guard, wrapper recursion, and core file rules

use crate::classifier::effect::{EffectLevel, ShellClassification, TargetScope};
use crate::classifier::parser::ParsedCommand;
use crate::classifier::scope::{classify_args_scope, classify_path_token};
use crate::classifier::tools::classify_tool_specific;
use crate::sandbox::WorkspaceSandbox;

/// Safe environment variables permitted on read-only commands (`K21`).
const SAFE_ENV_VARS: &[&str] = &["LC_ALL", "LANG", "TZ", "NO_COLOR", "TERM"];

/// Environment variables that hijack binary loading or spawn arbitrary helper commands (`V01`–`V06`).
const EXEC_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "GIT_SSH_COMMAND",
    "GIT_EXTERNAL_DIFF",
    "PAGER",
    "MANPAGER",
    "RUSTC_WRAPPER",
    "EDITOR",
    "VISUAL",
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
];

/// Binaries that always execute code, scripts, or escalate privileges (`D01`–`D14`, `E33`, `E42`–`E43`, `R09`–`R11`).
const EXEC_BINARIES: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "eval", "source", ".", "python", "python3", "perl", "node",
    "ruby", "lua", "php", "sudo", "su", "doas", "pkexec", "less", "more", "npm", "pnpm", "yarn",
    "bun", "pip", "pip3", "uv", "make", "cmake", "ninja",
];

/// Pure read-only file-reading utilities whose positional path arguments must still be scope-checked (`P01`–`P12`).
const READONLY_PATH_BINARIES: &[&str] = &[
    "cat",
    "tac",
    "rev",
    "fold",
    "expand",
    "unexpand",
    "head",
    "wc",
    "ls",
    "stat",
    "du",
    "diff",
    "grep",
    "nl",
    "cut",
    "tr",
    "sha256sum",
    "b2sum",
    "md5sum",
    "file",
    "realpath",
    "dirname",
    "basename",
    "base64",
    "xxd",
];

/// Classifies a single [`ParsedCommand`] within an optional [`WorkspaceSandbox`].
#[must_use]
pub fn classify_parsed_command(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> ShellClassification {
    let env_effect = classify_env_vars(&cmd.env_vars);
    if cmd.program.is_empty() {
        return env_effect;
    }
    let stage_effect = classify_command_body(cmd, sandbox);
    env_effect.merge(stage_effect)
}

fn classify_env_vars(env_vars: &[(String, String)]) -> ShellClassification {
    let mut acc = ShellClassification::new(EffectLevel::ReadOnly, "no unsafe env vars");
    for (k, _v) in env_vars {
        if k == "PATH" {
            acc = acc.merge(ShellClassification::new(
                EffectLevel::Unknown,
                "PATH modification prevents static binary resolution",
            ));
        } else if EXEC_ENV_VARS.contains(&k.as_str()) {
            acc = acc.merge(ShellClassification::new(
                EffectLevel::Exec,
                format!("environment variable '{k}' triggers command execution"),
            ));
        } else if !SAFE_ENV_VARS.contains(&k.as_str()) {
            // Local variable assignment (`c=touch`, `x=/tmp`) is tracked by the parser.
        }
    }
    acc
}

fn classify_command_body(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> ShellClassification {
    if cmd.program.contains('*') || cmd.program.contains('?') || cmd.program.contains('[') {
        return ShellClassification::new(
            EffectLevel::Unknown,
            "glob in command position cannot be resolved statically",
        );
    }
    if cmd.program.starts_with("./")
        || cmd.program.starts_with("../")
        || cmd.program.ends_with(".sh")
    {
        return ShellClassification::new(
            EffectLevel::Exec,
            format!("executing script '{}'", cmd.program),
        );
    }
    if cmd.binary == "export" {
        return ShellClassification::new(
            EffectLevel::Unknown,
            "export alters persistent shell state",
        );
    }
    if let Some(wrapped) = classify_wrapper(cmd, sandbox) {
        return wrapped;
    }
    if EXEC_BINARIES.contains(&cmd.binary.as_str()) {
        return ShellClassification::new(
            EffectLevel::Exec,
            format!(
                "'{}' is an interpreter or privilege-escalating binary",
                cmd.binary
            ),
        );
    }
    if matches!(
        cmd.binary.as_str(),
        "echo" | "printf" | "pwd" | "true" | "false" | ":" | "whoami" | "id" | "uname" | "which"
    ) {
        return ShellClassification::new(
            EffectLevel::ReadOnly,
            format!("'{}' is a pure builtin/read-only utility", cmd.binary),
        );
    }
    if cmd.binary == "exec" {
        return if cmd.raw_args.is_empty() {
            ShellClassification::new(EffectLevel::ReadOnly, "bare fd-redirection exec")
        } else {
            ShellClassification::new(EffectLevel::Exec, "exec replaces process image")
        };
    }
    if READONLY_PATH_BINARIES.contains(&cmd.binary.as_str()) {
        let scope = classify_args_scope(&cmd.positionals, sandbox);
        return ShellClassification::with_target(
            EffectLevel::ReadOnly,
            scope,
            format!("'{}' reads target path(s)", cmd.binary),
        );
    }
    if let Some(fs_mut) = classify_fs_mutator(cmd, sandbox) {
        return fs_mut;
    }
    classify_tool_specific(cmd, sandbox)
}

fn classify_wrapper(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> Option<ShellClassification> {
    match cmd.binary.as_str() {
        "env" | "command" | "nohup" | "time" => {
            let sub = unwrap_after(cmd, 0)?;
            Some(classify_parsed_command(&sub, sandbox))
        }
        "nice" => {
            let skip = if cmd.raw_args.first().map(String::as_str) == Some("-n") {
                2
            } else {
                0
            };
            let sub = unwrap_after(cmd, skip)?;
            Some(classify_parsed_command(&sub, sandbox))
        }
        "timeout" => {
            let sub = unwrap_after(cmd, 1)?;
            Some(classify_parsed_command(&sub, sandbox))
        }
        "xargs" => Some(classify_xargs(cmd, sandbox)),
        _ => None,
    }
}

fn unwrap_after(cmd: &ParsedCommand, skip_tokens: usize) -> Option<ParsedCommand> {
    let rest = cmd.raw_args.get(skip_tokens..)?;
    let (prog, tail) = rest.split_first()?;
    Some(ParsedCommand::from_words(
        prog.clone(),
        tail.to_vec(),
        Vec::new(),
    ))
}

fn classify_xargs(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd.has_short('t') {
        return ShellClassification::new(EffectLevel::Exec, "xargs -t executes target command");
    }
    if let Some((prog, tail)) = cmd.positionals.split_first() {
        let inner = ParsedCommand::from_words(prog.clone(), tail.to_vec(), Vec::new());
        let inner_class = classify_parsed_command(&inner, sandbox);
        let eff = inner_class.effect.max(EffectLevel::WorkspaceWrite);
        return ShellClassification::with_target(
            eff,
            inner_class.target,
            "xargs wrapper execution",
        );
    }
    ShellClassification::new(EffectLevel::Unknown, "xargs with unresolved target")
}

fn classify_fs_mutator(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> Option<ShellClassification> {
    match cmd.binary.as_str() {
        "ln" => {
            let all_scope = classify_args_scope(&cmd.positionals, sandbox);
            let dest_scope = cmd
                .positionals
                .last()
                .map_or(TargetScope::Workspace, |d| classify_path_token(d, sandbox));
            let eff = if dest_scope == TargetScope::OutsideWorkspace {
                EffectLevel::OutsideWrite
            } else {
                EffectLevel::WorkspaceWrite
            };
            Some(ShellClassification::with_target(
                eff,
                all_scope,
                "ln creates link",
            ))
        }
        "rm" | "touch" | "chmod" | "chown" | "mkdir" | "rmdir" | "tee" => {
            let scope = classify_args_scope(&cmd.positionals, sandbox);
            let eff = if scope == TargetScope::OutsideWorkspace {
                EffectLevel::OutsideWrite
            } else {
                EffectLevel::WorkspaceWrite
            };
            Some(ShellClassification::with_target(
                eff,
                scope,
                format!("'{}' mutates path(s)", cmd.binary),
            ))
        }
        "cp" | "mv" => {
            let all_scope = classify_args_scope(&cmd.positionals, sandbox);
            let dest_scope = cmd
                .positionals
                .last()
                .map_or(TargetScope::Workspace, |d| classify_path_token(d, sandbox));
            let eff = if dest_scope == TargetScope::OutsideWorkspace {
                EffectLevel::OutsideWrite
            } else {
                EffectLevel::WorkspaceWrite
            };
            Some(ShellClassification::with_target(
                eff,
                all_scope,
                format!("'{}' copies/moves path(s)", cmd.binary),
            ))
        }
        "dd" => {
            let out_scope = cmd
                .raw_args
                .iter()
                .find_map(|a| a.strip_prefix("of="))
                .map_or(TargetScope::Workspace, |p| classify_path_token(p, sandbox));
            let eff = if out_scope == TargetScope::OutsideWorkspace {
                EffectLevel::OutsideWrite
            } else {
                EffectLevel::WorkspaceWrite
            };
            Some(ShellClassification::with_target(
                eff,
                out_scope,
                "dd writes output stream",
            ))
        }
        _ => None,
    }
}
