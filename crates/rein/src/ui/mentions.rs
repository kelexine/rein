// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-20
// Purpose: Pure-Rust @ mention autocomplete (CWD files/dirs, built-in/plugin agents, plugins, MCP) and prompt context resolver

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Category of an `@` mention autocomplete candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionKind {
    /// Directory inside CWD.
    Directory,
    /// File inside CWD.
    File,
    /// Built-in, custom, or plugin agent.
    Agent,
    /// Installed/enabled plugin.
    Plugin,
    /// Configured MCP server.
    Mcp,
}

/// Single selectable entry in the `@` autocomplete menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionCandidate {
    /// Entry category.
    pub kind: MentionKind,
    /// Display label shown in the popup.
    pub label: String,
    /// Text inserted after `@` when selected.
    pub insert_text: String,
    /// Optional right-aligned description (primarily for agents/plugins).
    pub description: String,
}

/// Extracts `(at_byte_index, query)` when the cursor is inside an active `@query` token.
#[must_use]
pub fn active_mention_query(input: &str, cursor_bytes: usize) -> Option<(usize, String)> {
    let clamped = cursor_bytes.min(input.len());
    let before = &input[..clamped];
    // Find the start of the current whitespace-delimited token.
    let token_start = before
        .char_indices()
        .rev()
        .find(|&(_, c)| c.is_whitespace())
        .map_or(0, |(idx, c)| idx + c.len_utf8());
    let token = &before[token_start..];
    let query = token.strip_prefix('@')?;
    // Ignore already-quoted selections like `@"explore (agent)"`.
    if query.starts_with('"') {
        return None;
    }
    Some((token_start, query.to_owned()))
}

/// Collects `@` candidates from CWD, built-in/custom/plugin agents, plugins, and MCP servers.
#[must_use]
pub fn collect_mention_candidates(
    cwd: &Path,
    respect_gitignore: bool,
    extra_agents: &[(String, String)],
    plugins: &[(String, String)],
    mcp_servers: &[String],
) -> Vec<MentionCandidate> {
    let mut out = Vec::new();

    // 1. Built-in agents
    for (name, desc) in [
        ("explore", "Fast read-only codebase exploration"),
        ("plan", "Architecture & step-by-step planning"),
        ("verify", "Run tests, lints, and verify changes"),
    ] {
        out.push(MentionCandidate {
            kind: MentionKind::Agent,
            label: format!("{name} (agent)"),
            insert_text: format!("\"{name} (agent)\""),
            description: desc.to_owned(),
        });
    }

    // 2. Custom & plugin agents
    for (name, desc) in extra_agents {
        out.push(MentionCandidate {
            kind: MentionKind::Agent,
            label: format!("{name} (agent)"),
            insert_text: format!("\"{name} (agent)\""),
            description: desc.clone(),
        });
    }

    // 3. Enabled plugins
    for (name, desc) in plugins {
        out.push(MentionCandidate {
            kind: MentionKind::Plugin,
            label: format!("{name} (plugin)"),
            insert_text: format!("plugin:{name}"),
            description: desc.clone(),
        });
    }

    // 4. Configured MCP servers
    for srv in mcp_servers {
        out.push(MentionCandidate {
            kind: MentionKind::Mcp,
            label: format!("{srv} (mcp)"),
            insert_text: format!("mcp:{srv}"),
            description: "Configured MCP server".to_owned(),
        });
    }

    // 5. Files & directories in CWD (pure Rust)
    let ignore_rules = if respect_gitignore {
        load_gitignore_rules(cwd)
    } else {
        Vec::new()
    };
    walk_cwd(cwd, cwd, 0, &ignore_rules, &mut out);
    out
}

/// Filters and ranks candidates matching `query` (case-insensitive prefix first, then substring).
#[must_use]
pub fn filter_mention_candidates<'a>(
    all: &'a [MentionCandidate],
    query: &str,
) -> Vec<&'a MentionCandidate> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return all.iter().collect();
    }
    let mut prefix = Vec::new();
    let mut substr = Vec::new();
    for item in all {
        let low = item.label.to_ascii_lowercase();
        let file_name = item
            .label
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&low);
        if low.starts_with(&q) || file_name.starts_with(&q) {
            prefix.push(item);
        } else if low.contains(&q) || item.description.to_ascii_lowercase().contains(&q) {
            substr.push(item);
        }
    }
    prefix.extend(substr);
    prefix
}

/// Resolves `@path` and `@"name (agent)"` references in `prompt` into appended `<tagged_context>`.
#[must_use]
pub fn resolve_mentions_for_prompt(prompt: &str, cwd: &Path) -> String {
    let Ok(canon_cwd) = std::fs::canonicalize(cwd) else {
        return prompt.to_owned();
    };
    let mut contexts = Vec::new();
    let mut seen = BTreeSet::new();

    // Quoted agent tags: @"name (agent)"
    let mut rest = prompt;
    while let Some(idx) = rest.find("@\"") {
        let after = &rest[idx + 2..];
        if let Some(end_q) = after.find('"') {
            let inside = &after[..end_q];
            if let Some(agent_name) = inside.strip_suffix(" (agent)")
                && seen.insert(format!("agent:{agent_name}"))
            {
                contexts.push(format!(
                    "- Tagged agent `{agent_name}`: delegate to `{agent_name}` via `agent` or `invoke_subagent` when appropriate."
                ));
            }
            rest = &after[end_q + 1..];
        } else {
            break;
        }
    }

    // Unquoted @tokens (files/dirs/plugins/mcp)
    for word in prompt.split_whitespace() {
        let Some(raw_tag) = word.strip_prefix('@') else {
            continue;
        };
        let tag = raw_tag.trim_end_matches([',', '.', ';', ':', ')', ']']);
        if tag.is_empty() || tag.starts_with('"') || !seen.insert(tag.to_owned()) {
            continue;
        }
        if let Some(plugin_name) = tag.strip_prefix("plugin:") {
            contexts.push(format!("- Tagged plugin `{plugin_name}`."));
            continue;
        }
        if let Some(mcp_name) = tag.strip_prefix("mcp:") {
            contexts.push(format!(
                "- Tagged MCP server `{mcp_name}`: use `call_mcp_tool` with server `{mcp_name}`."
            ));
            continue;
        }
        let candidate = cwd.join(tag.trim_end_matches('/'));
        if let Ok(canon) = std::fs::canonicalize(&candidate)
            && canon.starts_with(&canon_cwd)
        {
            if canon.is_file() {
                if let Ok(bytes) = std::fs::read(&canon) {
                    let text = String::from_utf8_lossy(&bytes);
                    let snippet: String = text.chars().take(6000).collect();
                    let truncated = if text.len() > snippet.len() {
                        "\n... (truncated)"
                    } else {
                        ""
                    };
                    contexts.push(format!(
                        "<tagged_file path=\"{tag}\">\n{snippet}{truncated}\n</tagged_file>"
                    ));
                }
            } else if canon.is_dir()
                && let Ok(rd) = std::fs::read_dir(&canon)
            {
                let mut names: Vec<String> = rd
                    .flatten()
                    .take(40)
                    .map(|e| {
                        let mut n = e.file_name().to_string_lossy().into_owned();
                        if e.file_type().is_ok_and(|ft| ft.is_dir()) {
                            n.push('/');
                        }
                        n
                    })
                    .collect();
                names.sort();
                contexts.push(format!(
                    "<tagged_dir path=\"{tag}\">\n{}\n</tagged_dir>",
                    names.join("\n")
                ));
            }
        }
    }

    if contexts.is_empty() {
        prompt.to_owned()
    } else {
        format!(
            "{prompt}\n\n<tagged_context>\n{}\n</tagged_context>",
            contexts.join("\n")
        )
    }
}

fn load_gitignore_rules(cwd: &Path) -> Vec<String> {
    let path = cwd.join(".gitignore");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(|l| l.trim_start_matches('/').trim_end_matches('/').to_owned())
        .collect()
}

fn is_ignored(rel: &str, name: &str, rules: &[String]) -> bool {
    if matches!(name, ".git" | "target" | "node_modules" | ".venv") {
        return true;
    }
    for rule in rules {
        if let Some(ext) = rule.strip_prefix("*.") {
            if name.ends_with(&format!(".{ext}")) {
                return true;
            }
        } else if name == rule || rel == rule || rel.starts_with(&format!("{rule}/")) {
            return true;
        }
    }
    false
}

fn walk_cwd(
    root: &Path,
    dir: &Path,
    depth: usize,
    rules: &[String],
    out: &mut Vec<MentionCandidate>,
) {
    const MAX_DEPTH: usize = 5;
    const MAX_ENTRIES: usize = 1200;
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();

    for path in entries {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(rel_path) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel_path.to_string_lossy().replace('\\', "/");
        if is_ignored(&rel, name, rules) {
            continue;
        }
        if path.is_dir() {
            let label = format!("{rel}/");
            out.push(MentionCandidate {
                kind: MentionKind::Directory,
                label: label.clone(),
                insert_text: label,
                description: "Directory".to_owned(),
            });
            walk_cwd(root, &path, depth + 1, rules, out);
        } else if path.is_file() {
            out.push(MentionCandidate {
                kind: MentionKind::File,
                label: rel.clone(),
                insert_text: rel,
                description: "File".to_owned(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_active_mention_query_and_resolves_tagged_file()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            active_mention_query("@crat", 5),
            Some((0, "crat".to_owned()))
        );
        assert_eq!(
            active_mention_query("inspect @src/main", 17),
            Some((8, "src/main".to_owned()))
        );
        assert_eq!(active_mention_query("email@domain", 12), None);
        assert_eq!(active_mention_query("@\"explore (agent)\" ", 19), None);

        let root = std::env::temp_dir().join(format!("rein_mentions_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/rein"))?;
        std::fs::write(root.join("crates/rein/lib.rs"), "pub fn hi() {}\n")?;
        std::fs::write(root.join(".gitignore"), "ignored.log\n")?;
        std::fs::write(root.join("ignored.log"), "secret")?;

        let all = collect_mention_candidates(
            &root,
            true,
            &[],
            &[("code-review".to_owned(), "PR review".to_owned())],
            &["sqlite".to_owned()],
        );
        assert!(all.iter().any(|c| c.label == "crates/"));
        assert!(all.iter().any(|c| c.label == "explore (agent)"));
        assert!(!all.iter().any(|c| c.label == "ignored.log"));

        let filtered = filter_mention_candidates(&all, "crat");
        assert!(filtered.iter().any(|c| c.label.starts_with("crates/")));

        let resolved = resolve_mentions_for_prompt(
            "check @crates/rein/lib.rs and @\"explore (agent)\"",
            &root,
        );
        assert!(resolved.contains("<tagged_file path=\"crates/rein/lib.rs\">"));
        assert!(resolved.contains("pub fn hi()"));
        assert!(resolved.contains("Tagged agent `explore`"));

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
