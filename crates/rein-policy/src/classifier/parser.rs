// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Compound shell parser handling redirections, variables, control flow, and flags

use std::collections::BTreeMap;

use crate::classifier::lexer::{LexedScript, normalize_and_expand_word, scan_script, unquote_word};

/// Redirection operator kind attached to a shell stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectKind {
    /// Input file redirection (`<`).
    Input,
    /// Here-string redirection (`<<<`).
    HereString,
    /// Output or read-write file redirection (`>`, `>>`, `>|`, `&>`, `2>`, `3>`, `1<>`).
    Output,
}

/// A single redirection in a shell stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirection {
    /// Kind of redirection.
    pub kind: RedirectKind,
    /// Unquoted target path or string.
    pub target: String,
}

/// Parsed representation of a single command stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    /// Full normalized program token (preserves leading `./` or `/???/`).
    pub program: String,
    /// Basename of `program`.
    pub binary: String,
    /// Leading environment variable assignments (`KEY=VALUE`).
    pub env_vars: Vec<(String, String)>,
    /// Unpacked short flags (`-ni` -> `['n', 'i']`).
    pub short_flags: Vec<char>,
    /// Long options (`--in-place=.bak`, `--pre`).
    pub long_options: Vec<String>,
    /// All normalized arguments following `program`.
    pub raw_args: Vec<String>,
    /// Positional (non-flag) arguments.
    pub positionals: Vec<String>,
}

/// Complete parsed shell pipeline/script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPipeline {
    /// Command stages in execution order.
    pub stages: Vec<ParsedCommand>,
    /// Redirections collected across all stages.
    pub redirections: Vec<Redirection>,
    /// Extracted `$(...)`, `` `...` ``, and `<(...)` sub-scripts.
    pub substitutions: Vec<String>,
    /// Whether a substitution or glob sits in command position.
    pub dynamic_command: bool,
}

impl ParsedCommand {
    /// Constructs a [`ParsedCommand`] from normalized tokens and leading env assignments.
    #[must_use]
    pub fn from_words(program: String, args: Vec<String>, env_vars: Vec<(String, String)>) -> Self {
        let binary = program.rsplit('/').next().unwrap_or(&program).to_owned();
        let mut short_flags = Vec::new();
        let mut long_options = Vec::new();
        let mut positionals = Vec::new();
        let mut after_dd = false;

        for arg in &args {
            if after_dd {
                positionals.push(arg.clone());
            } else if arg == "--" {
                after_dd = true;
            } else if let Some(long) = arg.strip_prefix("--") {
                long_options.push(long.to_owned());
            } else if let Some(cluster) = arg.strip_prefix('-')
                && !cluster.is_empty()
                && !cluster.chars().all(|c| c.is_ascii_digit())
            {
                for ch in cluster.chars().take_while(char::is_ascii_alphabetic) {
                    short_flags.push(ch);
                }
            } else {
                positionals.push(arg.clone());
            }
        }
        Self {
            program,
            binary,
            env_vars,
            short_flags,
            long_options,
            raw_args: args,
            positionals,
        }
    }

    /// Returns `true` if `flag` is present in unpacked short flags.
    #[must_use]
    pub fn has_short(&self, flag: char) -> bool {
        self.short_flags.contains(&flag)
    }

    /// Returns `true` if any long option equals or starts with `prefix` (GNU `getopt_long` prefix rule).
    #[must_use]
    pub fn has_long_prefix(&self, prefix: &str) -> bool {
        self.long_options.iter().any(|opt| {
            let name = opt.split('=').next().unwrap_or(opt);
            name == prefix || name.starts_with(prefix)
        })
    }
}

impl ShellPipeline {
    /// Parses a raw shell script or returns an error on unterminated quotes/substitutions.
    pub fn parse(script: &str) -> Result<Self, &'static str> {
        let LexedScript {
            substitutions,
            mut dynamic_command,
        } = scan_script(script)?;
        let raw_tokens = tokenize_raw(script);
        let mut stages = Vec::new();
        let mut redirections = Vec::new();
        let mut vars: BTreeMap<String, String> = BTreeMap::new();
        let mut current = Vec::new();
        let mut iter = raw_tokens.into_iter().peekable();

        while let Some(tok) = iter.next() {
            if matches!(tok.as_str(), "|" | "&&" | "||" | ";" | "&" | "\n") {
                flush_tokens(&mut current, &mut vars, &mut stages, &mut dynamic_command);
            } else if tok == "2>&1" || tok == "1>&2" {
                continue;
            } else if let Some(kind) = classify_redirect_op(&tok) {
                if let Some(target_raw) = iter.next() {
                    let target = expand_vars(&unquote_word(&target_raw), &vars);
                    redirections.push(Redirection { kind, target });
                }
            } else {
                current.push(tok);
            }
        }
        flush_tokens(&mut current, &mut vars, &mut stages, &mut dynamic_command);
        Ok(Self {
            stages,
            redirections,
            substitutions,
            dynamic_command,
        })
    }
}

fn classify_redirect_op(tok: &str) -> Option<RedirectKind> {
    match tok {
        "<" => Some(RedirectKind::Input),
        "<<<" => Some(RedirectKind::HereString),
        ">" | ">>" | ">|" | "&>" | "2>" | "3>" | "1<>" | "<>" => Some(RedirectKind::Output),
        _ => None,
    }
}

fn flush_tokens(
    raw_stage: &mut Vec<String>,
    vars: &mut BTreeMap<String, String>,
    stages: &mut Vec<ParsedCommand>,
    dynamic_command: &mut bool,
) {
    if raw_stage.is_empty() {
        return;
    }
    let drained: Vec<String> = std::mem::take(raw_stage);
    let mut words = Vec::new();
    for raw in drained {
        if matches!(raw.as_str(), "(" | ")" | "{" | "}") {
            continue;
        }
        if let Some(fn_name) = raw.strip_suffix("(){").or_else(|| raw.strip_suffix("()")) {
            vars.insert(format!("__fn_{fn_name}"), "1".to_owned());
            continue;
        }
        let trimmed = raw
            .strip_prefix('(')
            .unwrap_or(&raw)
            .strip_suffix(')')
            .unwrap_or(&raw);
        if trimmed.is_empty() || vars.contains_key(&format!("__fn_{trimmed}")) {
            continue;
        }
        if (raw.starts_with("$(") || raw.starts_with('`')) && words.is_empty() {
            *dynamic_command = true;
        }
        for w in normalize_and_expand_word(trimmed) {
            words.push(expand_vars(&w, vars));
        }
    }
    build_stage_commands(words, vars, stages);
}

fn build_stage_commands(
    words: Vec<String>,
    vars: &mut BTreeMap<String, String>,
    stages: &mut Vec<ParsedCommand>,
) {
    let mut idx = 0;
    while idx < words.len()
        && matches!(
            words[idx].as_str(),
            "if" | "then" | "elif" | "else" | "fi" | "do" | "done"
        )
    {
        idx += 1;
    }
    if idx + 3 <= words.len()
        && words[idx] == "for"
        && words.get(idx + 2).map(String::as_str) == Some("in")
    {
        if let Some(val) = words.get(idx + 3) {
            vars.insert(words[idx + 1].clone(), val.clone());
        }
        return;
    }
    let mut env_vars = Vec::new();
    while idx < words.len() {
        if let Some((k, v)) = split_env_assignment(&words[idx]) {
            vars.insert(k.clone(), v.clone());
            env_vars.push((k, v));
            idx += 1;
        } else {
            break;
        }
    }
    if idx < words.len() {
        let program = words[idx].clone();
        let args = words[idx + 1..].to_vec();
        stages.push(ParsedCommand::from_words(program, args, env_vars));
    } else if !env_vars.is_empty() {
        stages.push(ParsedCommand::from_words(
            String::new(),
            Vec::new(),
            env_vars,
        ));
    }
}

fn split_env_assignment(word: &str) -> Option<(String, String)> {
    let (k, v) = word.split_once('=')?;
    if !k.is_empty()
        && !k.starts_with('-')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        Some((k.to_owned(), v.to_owned()))
    } else {
        None
    }
}

fn expand_vars(word: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = word.to_owned();
    for (k, v) in vars {
        out = out
            .replace(&format!("${{{k}}}"), v)
            .replace(&format!("${k}"), v);
    }
    out
}

fn tokenize_raw(script: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut quote: Option<char> = None;
    let mut paren_depth = 0usize;
    let mut chars = script.chars().peekable();

    while let Some(ch) = chars.next() {
        if paren_depth > 0 {
            if ch == '(' {
                paren_depth += 1;
            } else if ch == ')' {
                paren_depth -= 1;
            }
            buf.push(ch);
            continue;
        }
        if let Some(q) = quote {
            buf.push(ch);
            if ch == '\\' && q == '"' {
                if let Some(escaped) = chars.next() {
                    buf.push(escaped);
                }
            } else if q == '"' && ch == '$' && chars.peek() == Some(&'(') {
                if let Some(open) = chars.next() {
                    buf.push(open);
                    paren_depth += 1;
                }
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\\' {
            buf.push(ch);
            if let Some(next) = chars.next() {
                buf.push(next);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            buf.push(ch);
        } else if ch == '(' && (buf.ends_with('$') || buf.ends_with('<')) {
            paren_depth += 1;
            buf.push(ch);
        } else if paren_depth > 0 {
            if ch == '(' {
                paren_depth += 1;
            } else if ch == ')' {
                paren_depth -= 1;
            }
            buf.push(ch);
        } else if ch == '\n' {
            push_buf(&mut buf, &mut tokens);
            tokens.push("\n".to_owned());
        } else if ch.is_whitespace() {
            push_buf(&mut buf, &mut tokens);
        } else if matches!(ch, '|' | '&' | ';' | '>' | '<') {
            lex_operator(ch, &mut chars, &mut buf, &mut tokens);
        } else {
            buf.push(ch);
        }
    }
    push_buf(&mut buf, &mut tokens);
    tokens
}

fn lex_operator(
    ch: char,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    buf: &mut String,
    tokens: &mut Vec<String>,
) {
    if (ch == '>' || ch == '<') && buf.chars().all(|c| c.is_ascii_digit()) && !buf.is_empty() {
        let mut fd_op = std::mem::take(buf);
        fd_op.push(ch);
        while let Some(&next) = chars.peek() {
            if matches!(next, '>' | '<' | '&' | '1' | '2') {
                fd_op.push(next);
                let _ = chars.next();
            } else {
                break;
            }
        }
        tokens.push(fd_op);
        return;
    }
    push_buf(buf, tokens);
    let mut op = String::from(ch);
    while let Some(&next) = chars.peek() {
        if (ch == '|' && next == '|')
            || (ch == '&' && (next == '&' || next == '>'))
            || (ch == '>' && (next == '>' || next == '|'))
            || (ch == '<' && (next == '<' || next == '>'))
        {
            op.push(next);
            let _ = chars.next();
        } else {
            break;
        }
    }
    tokens.push(op);
}

fn push_buf(buf: &mut String, tokens: &mut Vec<String>) {
    if !buf.is_empty() {
        tokens.push(std::mem::take(buf));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pipelines_env_vars_redirections_and_flags() {
        let pipe = ShellPipeline::parse(
            "RUST_LOG=debug cargo test --release -- --nocapture 2>&1 > out.log",
        )
        .expect("valid pipeline");
        assert!(!pipe.dynamic_command);
        assert_eq!(pipe.stages.len(), 1);
        let stage = &pipe.stages[0];
        assert_eq!(stage.binary, "cargo");
        assert_eq!(
            stage.env_vars,
            vec![("RUST_LOG".to_owned(), "debug".to_owned())]
        );
        assert!(stage.has_long_prefix("rel"));
        assert_eq!(stage.positionals, vec!["test", "--nocapture"]);
        assert_eq!(
            pipe.redirections,
            vec![Redirection {
                kind: RedirectKind::Output,
                target: "out.log".to_owned(),
            }]
        );

        let multi =
            ShellPipeline::parse("x=src/lib.rs && cat $x | wc -l <<< 'hi'").expect("multi stage");
        assert_eq!(multi.stages.len(), 3);
        assert_eq!(multi.stages[1].positionals, vec!["src/lib.rs"]);
        assert_eq!(multi.stages[2].binary, "wc");
        assert!(multi.stages[2].has_short('l'));
        assert_eq!(multi.redirections[0].kind, RedirectKind::HereString);
    }

    #[test]
    fn handles_short_flag_clusters_and_negative_numbers() {
        let cmd = ParsedCommand::from_words(
            "/usr/bin/head".to_owned(),
            vec![
                "-n".to_owned(),
                "10".to_owned(),
                "-5".to_owned(),
                "-qv".to_owned(),
                "--in-place=.bak".to_owned(),
            ],
            Vec::new(),
        );
        assert_eq!(cmd.binary, "head");
        assert!(cmd.has_short('n'));
        assert!(cmd.has_short('q'));
        assert!(cmd.has_short('v'));
        assert!(cmd.has_long_prefix("in-place"));
        assert!(cmd.has_long_prefix("in-pl"));
        assert_eq!(cmd.positionals, vec!["10", "-5"]);
    }
}
