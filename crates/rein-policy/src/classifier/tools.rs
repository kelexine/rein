// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Tool-specific flag, option, and embedded script analyzers (sed, awk, find, git, etc.)

use crate::classifier::effect::{EffectLevel, ShellClassification, TargetScope};
use crate::classifier::parser::ParsedCommand;
use crate::classifier::scope::{classify_args_scope, classify_path_token};
use crate::sandbox::WorkspaceSandbox;

/// Dispatches specialized flag/script analysis for developer, search, archive, and network tools.
#[must_use]
pub fn classify_tool_specific(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> ShellClassification {
    match cmd.binary.as_str() {
        "tail" => classify_tail(cmd, sandbox),
        "find" | "fd" => classify_find(cmd, sandbox),
        "sed" => classify_sed(cmd, sandbox),
        "sort" => classify_sort(cmd, sandbox),
        "uniq" => classify_uniq(cmd, sandbox),
        "awk" | "gawk" => classify_awk(cmd, sandbox),
        "tar" => classify_tar(cmd, sandbox),
        "history" => classify_history(cmd, sandbox),
        "man" => classify_man(cmd),
        "rg" => classify_rg(cmd, sandbox),
        "git" => classify_git(cmd, sandbox),
        "cargo" => classify_cargo(cmd),
        "go" => classify_go(cmd, sandbox),
        "ruff" => classify_ruff(cmd, sandbox),
        "ssh" => classify_ssh(cmd),
        "curl" | "wget" | "dig" | "nc" | "ncat" | "ping" | "scp" | "rsync" => {
            classify_network_tool(cmd, sandbox)
        }
        _ => ShellClassification::new(
            EffectLevel::Unknown,
            format!("unrecognized command '{}'", cmd.binary),
        ),
    }
}

fn classify_tail(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    let scope = classify_args_scope(&cmd.positionals, sandbox);
    if cmd.has_short('f') || cmd.has_short('F') || cmd.has_long_prefix("follow") {
        return ShellClassification::with_target(
            EffectLevel::Exec,
            scope,
            "tail -f streams indefinitely",
        );
    }
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "tail bounded read")
}

fn classify_find(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd
        .raw_args
        .iter()
        .any(|a| matches!(a.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        return ShellClassification::new(EffectLevel::Exec, "find -exec spawns subprocess");
    }
    if let Some(write_target) =
        find_flag_value(&cmd.raw_args, &["-fprintf", "-fprint", "-fprint0", "-fls"])
    {
        let scope = classify_path_token(write_target, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "find file-output flag");
    }
    let search_roots: Vec<String> = cmd
        .raw_args
        .iter()
        .take_while(|a| !a.starts_with('-'))
        .cloned()
        .collect();
    let scope = classify_args_scope(&search_roots, sandbox);
    if cmd.raw_args.iter().any(|a| a == "-delete") {
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "find -delete removes entries");
    }
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "find read-only traversal")
}

fn classify_sed(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    for arg in &cmd.raw_args {
        if arg == "s/test/x/e"
            || arg.ends_with("/e")
            || arg.contains("1e ")
            || arg.starts_with("e ")
        {
            return ShellClassification::new(
                EffectLevel::Exec,
                "sed 'e' directive executes shell command",
            );
        }
        if let Some(w_path) = extract_sed_write_path(arg) {
            let scope = classify_path_token(w_path, sandbox);
            let eff = if scope == TargetScope::OutsideWorkspace {
                EffectLevel::OutsideWrite
            } else {
                EffectLevel::WorkspaceWrite
            };
            return ShellClassification::with_target(eff, scope, "sed 'w' directive writes file");
        }
    }
    let files: Vec<String> = cmd.positionals.iter().skip(1).cloned().collect();
    let scope = classify_args_scope(&files, sandbox);
    if cmd.has_short('i') || cmd.has_long_prefix("in-pl") {
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "sed in-place file modification");
    }
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "sed stream to stdout")
}

fn extract_sed_write_path(expr: &str) -> Option<&str> {
    if let Some(rest) = expr.strip_prefix("w ") {
        return Some(rest.trim());
    }
    if let Some((_, rest)) = expr.split_once("/w ") {
        return Some(rest.trim());
    }
    None
}

fn classify_sort(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd.has_long_prefix("compress-prog") {
        return ShellClassification::new(
            EffectLevel::Exec,
            "sort --compress-program executes binary",
        );
    }
    if let Some(out_path) = extract_output_option(cmd, "-o", "out") {
        let scope = classify_path_token(&out_path, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "sort output file write");
    }
    let scope = classify_args_scope(&cmd.positionals, sandbox);
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "sort to stdout")
}

fn classify_uniq(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if let Some(out_file) = cmd.positionals.get(1) {
        let scope = classify_path_token(out_file, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(
            eff,
            scope,
            "uniq 2nd positional writes output file",
        );
    }
    let scope = classify_args_scope(&cmd.positionals, sandbox);
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "uniq to stdout")
}

fn classify_awk(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    let script = cmd.positionals.first().map_or("", String::as_str);
    if script.contains("system(") || script.contains('|') {
        return ShellClassification::new(
            EffectLevel::Exec,
            "awk executes subprocess via system() or pipe",
        );
    }
    if let Some((_, after_gt)) = script.split_once('>') {
        let path = after_gt
            .trim()
            .trim_matches(|c| c == '"' || c == '\'' || c == '}')
            .trim();
        let scope = classify_path_token(path, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "awk redirects output to file");
    }
    let files: Vec<String> = cmd.positionals.iter().skip(1).cloned().collect();
    let scope = classify_args_scope(&files, sandbox);
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "awk read-only program")
}

fn classify_tar(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd
        .raw_args
        .iter()
        .any(|a| a.contains("exec=") || a.starts_with("--checkpoint-action"))
    {
        return ShellClassification::new(
            EffectLevel::Exec,
            "tar checkpoint-action executes command",
        );
    }
    let scope = classify_args_scope(&cmd.positionals, sandbox);
    let eff = if scope == TargetScope::OutsideWorkspace {
        EffectLevel::OutsideWrite
    } else {
        EffectLevel::WorkspaceWrite
    };
    ShellClassification::with_target(eff, scope, "tar archive operation")
}

fn classify_history(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> ShellClassification {
    if cmd.has_short('w') || cmd.has_short('a') {
        let scope = classify_args_scope(&cmd.positionals, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "history -w writes file");
    }
    ShellClassification::new(EffectLevel::ReadOnly, "history listing")
}

fn classify_man(cmd: &ParsedCommand) -> ShellClassification {
    if cmd.has_short('P') || cmd.has_long_prefix("pager") {
        return ShellClassification::new(EffectLevel::Exec, "man -P invokes custom pager command");
    }
    ShellClassification::new(EffectLevel::ReadOnly, "man page lookup")
}

fn classify_rg(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd.has_long_prefix("pre") || cmd.has_long_prefix("hostname-bin") {
        return ShellClassification::new(
            EffectLevel::Exec,
            "rg preprocessor/hostname-bin executes binary",
        );
    }
    let scope = classify_args_scope(&cmd.positionals, sandbox);
    ShellClassification::with_target(EffectLevel::ReadOnly, scope, "rg search")
}

fn classify_git(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd.has_short('c')
        || cmd.has_long_prefix("open-files-in-pager")
        || cmd.has_long_prefix("receive-pack")
        || cmd.has_long_prefix("upload-pack")
        || cmd.has_long_prefix("exec-path")
    {
        return ShellClassification::new(
            EffectLevel::Exec,
            "git flag or -c config executes helper command",
        );
    }
    let Some(sub) = cmd.positionals.first().map(String::as_str) else {
        return ShellClassification::new(EffectLevel::ReadOnly, "git status/version");
    };
    match sub {
        "config" => ShellClassification::with_target(
            EffectLevel::Exec,
            TargetScope::Protected,
            "git config modifies protected repository/hook settings",
        ),
        "status" | "log" | "diff" | "show" | "rev-parse" | "ls-files" | "blame" | "grep" => {
            let scope = classify_args_scope(&cmd.positionals[1..], sandbox);
            ShellClassification::with_target(
                EffectLevel::ReadOnly,
                scope,
                format!("git {sub} read-only"),
            )
        }
        "push" | "fetch" | "pull" | "clone" | "ls-remote" => {
            ShellClassification::new(EffectLevel::Network, format!("git {sub} network operation"))
        }
        _ => ShellClassification::new(
            EffectLevel::WorkspaceWrite,
            format!("git {sub} workspace mutation"),
        ),
    }
}

fn classify_cargo(cmd: &ParsedCommand) -> ShellClassification {
    if cmd.has_long_prefix("version")
        || cmd.has_short('V')
        || cmd.has_long_prefix("help")
        || cmd.has_short('h')
    {
        return ShellClassification::new(EffectLevel::ReadOnly, "cargo version/help");
    }
    match cmd.positionals.first().map(String::as_str) {
        Some("fmt") => {
            ShellClassification::new(EffectLevel::WorkspaceWrite, "cargo fmt formats workspace")
        }
        Some("fetch" | "publish" | "login") => {
            ShellClassification::new(EffectLevel::Network, "cargo network command")
        }
        Some("install") => ShellClassification::new(
            EffectLevel::OutsideWrite,
            "cargo install writes to cargo home",
        ),
        _ => ShellClassification::new(EffectLevel::Exec, "cargo executes build.rs and proc-macros"),
    }
}

fn classify_go(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if cmd
        .raw_args
        .iter()
        .any(|a| a.starts_with("-vettool") || a == "-exec" || a.starts_with("-exec="))
    {
        return ShellClassification::new(EffectLevel::Exec, "go -vettool/-exec runs binary");
    }
    if let Some(out_path) = extract_output_option(cmd, "-o", "o") {
        let scope = classify_path_token(&out_path, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "go build -o writes binary");
    }
    ShellClassification::new(EffectLevel::Exec, "go toolchain execution")
}

fn classify_ruff(cmd: &ParsedCommand, sandbox: Option<&WorkspaceSandbox>) -> ShellClassification {
    if let Some(out_path) = extract_output_option(cmd, "-o", "output") {
        let scope = classify_path_token(&out_path, sandbox);
        let eff = if scope == TargetScope::OutsideWorkspace {
            EffectLevel::OutsideWrite
        } else {
            EffectLevel::WorkspaceWrite
        };
        return ShellClassification::with_target(eff, scope, "ruff -o writes output file");
    }
    ShellClassification::new(EffectLevel::ReadOnly, "ruff check")
}

fn classify_ssh(cmd: &ParsedCommand) -> ShellClassification {
    if cmd.raw_args.iter().any(|a| a.contains("ProxyCommand=")) {
        return ShellClassification::new(
            EffectLevel::Exec,
            "ssh ProxyCommand executes local command",
        );
    }
    ShellClassification::new(EffectLevel::Network, "ssh remote connection")
}

fn classify_network_tool(
    cmd: &ParsedCommand,
    sandbox: Option<&WorkspaceSandbox>,
) -> ShellClassification {
    let mut scope = classify_args_scope(&cmd.raw_args, sandbox);
    for flag in ["-T", "-f", "-o"] {
        if let Some(val) = find_flag_value(&cmd.raw_args, &[flag]) {
            scope = scope.max(classify_path_token(val, sandbox));
        }
    }
    ShellClassification::with_target(
        EffectLevel::Network,
        scope,
        format!("'{}' network operation", cmd.binary),
    )
}

fn find_flag_value<'a>(args: &'a [String], flags: &[&str]) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if flags.contains(&arg.as_str()) {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn extract_output_option(
    cmd: &ParsedCommand,
    short_flag: &str,
    long_prefix: &str,
) -> Option<String> {
    if let Some(val) = find_flag_value(&cmd.raw_args, &[short_flag]) {
        return Some(val.to_owned());
    }
    for opt in &cmd.long_options {
        if let Some((k, v)) = opt.split_once('=')
            && (k == long_prefix || k.starts_with(long_prefix))
        {
            return Some(v.to_owned());
        }
    }
    None
}
