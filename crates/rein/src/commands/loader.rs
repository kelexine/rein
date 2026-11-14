// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Loads Rein-scoped SKILL.md entries from ~/.rein/skills/ and <workspace>/.rein/skills/

use std::path::Path;

use anyhow::{Context, Result};

use crate::commands::registry::{CommandSource, SlashCommand, SlashKind, SlashRegistry};
use crate::config::ReinPaths;

/// Builds a [`SlashRegistry`] with built-in commands plus skills from Rein skill directories.
pub fn load_slash_registry(paths: &ReinPaths) -> Result<SlashRegistry> {
    let mut registry = SlashRegistry::with_builtins();
    load_skills_dir(&paths.global_skills_dir, CommandSource::User, &mut registry)?;
    load_skills_dir(&paths.repo_skills_dir, CommandSource::Repo, &mut registry)?;
    Ok(registry)
}

fn load_skills_dir(dir: &Path, source: CommandSource, registry: &mut SlashRegistry) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut subdirs = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to scan skills dir '{}'", dir.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            subdirs.push(path);
        }
    }
    subdirs.sort();

    for skill_dir in subdirs {
        let skill_md = skill_dir.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let Some(folder_name) = skill_dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let raw = std::fs::read_to_string(&skill_md)
            .with_context(|| format!("failed to read skill '{}'", skill_md.display()))?;
        let parsed = parse_skill_meta(folder_name, &raw);
        registry.register(SlashCommand {
            name: parsed.name,
            description: parsed.description,
            source,
            kind: SlashKind::Skill {
                skill_md_path: skill_md,
            },
        });
    }
    Ok(())
}

struct ParsedMeta {
    name: String,
    description: String,
}

fn parse_skill_meta(fallback_name: &str, raw: &str) -> ParsedMeta {
    let mut name = fallback_name.to_owned();
    let mut description = String::new();
    let body = if let Some((frontmatter, rest)) = split_frontmatter(raw) {
        for line in frontmatter.lines() {
            if let Some(val) = line.trim().strip_prefix("name:") {
                let cleaned = unquote_yaml(val);
                if !cleaned.is_empty() {
                    name = cleaned;
                }
            } else if let Some(val) = line.trim().strip_prefix("description:") {
                let cleaned = unquote_yaml(val);
                if !cleaned.is_empty() && cleaned != ">" && cleaned != "|" {
                    description = cleaned;
                }
            }
        }
        rest.trim().to_owned()
    } else {
        raw.trim().to_owned()
    };

    if description.is_empty() {
        description = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.trim_start_matches('#').trim())
            .unwrap_or("Rein skill")
            .to_owned();
    }

    ParsedMeta { name, description }
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let stripped = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    let end_idx = stripped.find("\n---")?;
    let fm = &stripped[..end_idx];
    let after_marker = &stripped[end_idx + 4..];
    let rest = after_marker
        .strip_prefix("\r\n")
        .or_else(|| after_marker.strip_prefix('\n'))
        .unwrap_or(after_marker);
    Some((fm, rest))
}

fn unquote_yaml(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_user_and_repo_skills_with_builtin_commands_protected() -> Result<()> {
        let root = std::env::temp_dir().join("rein_slash_loader_test");
        let _ = std::fs::remove_dir_all(&root);
        let paths = ReinPaths::from_dirs(root.join("home/.rein"), root.join("repo/.rein"));
        std::fs::create_dir_all(paths.global_skills_dir.join("linux-perf"))?;
        std::fs::create_dir_all(paths.repo_skills_dir.join("local-audit"))?;
        std::fs::create_dir_all(paths.repo_skills_dir.join("help"))?;

        std::fs::write(
            paths.global_skills_dir.join("linux-perf/SKILL.md"),
            "---\nname: linux-perf\ndescription: Profile Linux CPU hotspots\n---\n# Perf",
        )?;
        std::fs::write(
            paths.repo_skills_dir.join("local-audit/SKILL.md"),
            "# Local Audit Skill\nAudit crates.",
        )?;
        std::fs::write(
            paths.repo_skills_dir.join("help/SKILL.md"),
            "---\ndescription: Should not overwrite builtin\n---\nignored",
        )?;

        let reg = load_slash_registry(&paths)?;
        let perf = reg
            .all()
            .iter()
            .find(|c| c.name == "linux-perf")
            .context("missing linux-perf")?;
        assert_eq!(perf.source, CommandSource::User);
        assert_eq!(perf.description, "Profile Linux CPU hotspots");
        assert!(matches!(perf.kind, SlashKind::Skill { .. }));

        let audit = reg
            .all()
            .iter()
            .find(|c| c.name == "local-audit")
            .context("missing local-audit")?;
        assert_eq!(audit.source, CommandSource::Repo);
        assert_eq!(audit.description, "Local Audit Skill");

        let help = reg
            .all()
            .iter()
            .find(|c| c.name == "help")
            .context("missing help")?;
        assert_eq!(help.source, CommandSource::Builtin);

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
