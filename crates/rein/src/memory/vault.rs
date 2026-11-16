// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Sandboxed vault, transcript grep, doc reconciliation, and JSONL tools for AutoDream

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rein_core::{MemoryScope, ToolCall, ToolSpec};
use rein_tools::{JsonlMemoryStore, MemoryKind, MemoryRecord};
use serde_json::{Value, json};

const MAX_INDEX_LINES: usize = 200;
const MAX_INDEX_BYTES: usize = 25 * 1024;
const MAX_GREP_MATCHES: usize = 50;
const ALLOWED_DOCS: &[&str] = &["GEMINI.md", "CLAUDE.md", "AGENTS.md", "README.md"];

/// Scoped target paths available to AutoDream consolidation tools.
#[derive(Debug, Clone)]
pub struct DreamVault {
    project_memory_dir: PathBuf,
    personal_memory_dir: PathBuf,
    sessions_dir: PathBuf,
    workspace_dir: PathBuf,
    store: JsonlMemoryStore,
}

impl DreamVault {
    /// Creates a [`DreamVault`] scoped to `project_memory_dir`, `personal_memory_dir`, `sessions_dir`, `workspace_dir`, and `store`.
    #[must_use]
    pub fn new(
        project_memory_dir: PathBuf,
        personal_memory_dir: PathBuf,
        sessions_dir: PathBuf,
        workspace_dir: PathBuf,
        store: JsonlMemoryStore,
    ) -> Self {
        Self {
            project_memory_dir,
            personal_memory_dir,
            sessions_dir,
            workspace_dir,
            store,
        }
    }

    /// Returns provider tool specs permitted exclusively during an AutoDream pass.
    #[must_use]
    pub fn tool_specs() -> Vec<ToolSpec> {
        vec![
            spec(
                "list_vault",
                "List Markdown topic files and MEMORY.md presence inside project and personal memory vaults.",
                json!({ "type": "object", "properties": { "scope": { "type": "string", "enum": ["project", "personal"] } } }),
            ),
            spec(
                "read_vault",
                "Read MEMORY.md (omit topic) or a named Markdown topic file from project or personal vault.",
                json!({
                    "type": "object",
                    "required": ["scope"],
                    "properties": {
                        "scope": { "type": "string", "enum": ["project", "personal"] },
                        "topic": { "type": "string", "description": "Optional topic filename (without path separators)" }
                    }
                }),
            ),
            spec(
                "write_vault",
                "Write or update a Markdown topic file inside project or personal memory vault (not MEMORY.md).",
                json!({
                    "type": "object",
                    "required": ["scope", "topic", "content"],
                    "properties": {
                        "scope": { "type": "string", "enum": ["project", "personal"] },
                        "topic": { "type": "string" },
                        "content": { "type": "string" }
                    }
                }),
            ),
            spec(
                "delete_vault",
                "Delete an obsolete Markdown topic file inside project or personal memory vault.",
                json!({
                    "type": "object",
                    "required": ["scope", "topic"],
                    "properties": {
                        "scope": { "type": "string", "enum": ["project", "personal"] },
                        "topic": { "type": "string" }
                    }
                }),
            ),
            spec(
                "update_index",
                "Atomically update MEMORY.md index for a scope (<=200 lines, <=25KB, 1-line entries).",
                json!({
                    "type": "object",
                    "required": ["scope", "content"],
                    "properties": {
                        "scope": { "type": "string", "enum": ["project", "personal"] },
                        "content": { "type": "string" }
                    }
                }),
            ),
            spec(
                "grep_transcripts",
                "Grep session JSONL transcripts narrowly for a substring (capped at 50 matches).",
                json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": { "query": { "type": "string" } }
                }),
            ),
            spec(
                "read_project_doc",
                "Read-only access to workspace instruction docs (GEMINI.md, CLAUDE.md, AGENTS.md, README.md) for memory reconciliation.",
                json!({
                    "type": "object",
                    "required": ["filename"],
                    "properties": { "filename": { "type": "string", "enum": ["GEMINI.md", "CLAUDE.md", "AGENTS.md", "README.md"] } }
                }),
            ),
            spec(
                "memory_list",
                "List structured JSONL memory records in project or personal scope.",
                json!({
                    "type": "object",
                    "required": ["scope"],
                    "properties": { "scope": { "type": "string", "enum": ["project", "personal"] } }
                }),
            ),
            spec(
                "memory_upsert",
                "Upsert and mark a consolidated structured JSONL memory record.",
                json!({
                    "type": "object",
                    "required": ["scope", "key", "value"],
                    "properties": {
                        "scope": { "type": "string", "enum": ["project", "personal"] },
                        "key": { "type": "string" },
                        "value": { "type": "string" },
                        "kind": { "type": "string", "enum": ["general", "user", "feedback", "project", "reference"] }
                    }
                }),
            ),
            spec(
                "memory_delete",
                "Delete a stale or contradicted structured JSONL memory record by key.",
                json!({
                    "type": "object",
                    "required": ["scope", "key"],
                    "properties": {
                        "scope": { "type": "string", "enum": ["project", "personal"] },
                        "key": { "type": "string" }
                    }
                }),
            ),
        ]
    }

    /// Executes a single AutoDream tool call strictly within vault and read-only bounds.
    #[must_use]
    pub fn execute(&self, call: &ToolCall) -> (String, bool) {
        match self.dispatch(call) {
            Ok(out) => (out, false),
            Err(err) => (format!("error: {err}"), true),
        }
    }

    fn dispatch(&self, call: &ToolCall) -> Result<String, String> {
        let args = &call.arguments;
        match call.name.as_str() {
            "list_vault" => self.cmd_list_vault(args),
            "read_vault" => self.cmd_read_vault(args),
            "write_vault" => self.cmd_write_vault(args),
            "delete_vault" => self.cmd_delete_vault(args),
            "update_index" => self.cmd_update_index(args),
            "grep_transcripts" => self.cmd_grep_transcripts(args),
            "read_project_doc" => self.cmd_read_project_doc(args),
            "memory_list" => self.cmd_memory_list(args),
            "memory_upsert" => self.cmd_memory_upsert(args),
            "memory_delete" => self.cmd_memory_delete(args),
            other => Err(format!("unknown dream tool '{other}'")),
        }
    }

    fn dir_for(&self, scope: MemoryScope) -> &Path {
        match scope {
            MemoryScope::Project => &self.project_memory_dir,
            MemoryScope::Personal => &self.personal_memory_dir,
        }
    }

    fn cmd_list_vault(&self, args: &Value) -> Result<String, String> {
        let scopes = match args.get("scope").and_then(Value::as_str) {
            Some(raw) => vec![parse_scope(raw)?],
            None => vec![MemoryScope::Project, MemoryScope::Personal],
        };
        let mut entries = Vec::new();
        for scope in scopes {
            let dir = self.dir_for(scope);
            let files = list_md_files(dir)?;
            entries.push(json!({ "scope": scope_name(scope), "files": files }));
        }
        serde_json::to_string(&entries).map_err(|e| e.to_string())
    }

    fn cmd_read_vault(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let dir = self.dir_for(scope);
        let filename = match args.get("topic").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => sanitize_topic_filename(raw, true)?,
            _ => "MEMORY.md".to_owned(),
        };
        let path = dir.join(filename);
        if !path.exists() {
            return Ok(String::new());
        }
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    fn cmd_write_vault(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let topic = require_str(args, "topic")?;
        let content = require_str(args, "content")?;
        let filename = sanitize_topic_filename(topic, false)?;
        let dir = self.dir_for(scope);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        atomic_write_text(&dir.join(&filename), content)?;
        Ok(format!("wrote {}/{filename}", scope_name(scope)))
    }

    fn cmd_delete_vault(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let topic = require_str(args, "topic")?;
        let filename = sanitize_topic_filename(topic, false)?;
        let path = self.dir_for(scope).join(&filename);
        if !path.exists() {
            return Ok(format!("not found: {}/{filename}", scope_name(scope)));
        }
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        Ok(format!("deleted {}/{filename}", scope_name(scope)))
    }

    fn cmd_update_index(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let content = require_str(args, "content")?;
        if content.lines().count() > MAX_INDEX_LINES {
            return Err(format!("MEMORY.md exceeds {MAX_INDEX_LINES} lines"));
        }
        if content.len() > MAX_INDEX_BYTES {
            return Err(format!("MEMORY.md exceeds {MAX_INDEX_BYTES} bytes"));
        }
        let dir = self.dir_for(scope);
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        atomic_write_text(&dir.join("MEMORY.md"), content)?;
        Ok(format!("updated {}/MEMORY.md", scope_name(scope)))
    }

    fn cmd_grep_transcripts(&self, args: &Value) -> Result<String, String> {
        let query = require_str(args, "query")?.to_ascii_lowercase();
        if !self.sessions_dir.exists() {
            return Ok("[]".to_owned());
        }
        let mut hits = Vec::new();
        let entries = std::fs::read_dir(&self.sessions_dir).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            collect_file_hits(&path, &query, &mut hits);
            if hits.len() >= MAX_GREP_MATCHES {
                break;
            }
        }
        serde_json::to_string(&hits).map_err(|e| e.to_string())
    }

    fn cmd_read_project_doc(&self, args: &Value) -> Result<String, String> {
        let name = require_str(args, "filename")?;
        if !ALLOWED_DOCS.contains(&name) {
            return Err(format!("unsupported doc '{name}'"));
        }
        let path = self.workspace_dir.join(name);
        if !path.exists() {
            return Ok(String::new());
        }
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    }

    fn cmd_memory_list(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let records = self.store.query(scope, "").map_err(|e| e.to_string())?;
        serde_json::to_string(&records).map_err(|e| e.to_string())
    }

    fn cmd_memory_upsert(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let key = require_str(args, "key")?;
        let value = require_str(args, "value")?;
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .map_or(MemoryKind::General, MemoryKind::parse_lossy);
        let record = MemoryRecord::new(scope, key, value)
            .with_kind(kind)
            .with_consolidated(true);
        self.store.upsert(&record).map_err(|e| e.to_string())?;
        Ok(format!("upserted {}:{}", scope_name(scope), key))
    }

    fn cmd_memory_delete(&self, args: &Value) -> Result<String, String> {
        let scope = require_scope(args)?;
        let key = require_str(args, "key")?;
        let deleted = self.store.delete(scope, key).map_err(|e| e.to_string())?;
        Ok(if deleted {
            format!("deleted {}:{}", scope_name(scope), key)
        } else {
            format!("not found {}:{}", scope_name(scope), key)
        })
    }
}

fn spec(name: &str, description: &str, input_schema: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
    }
}

fn require_scope(args: &Value) -> Result<MemoryScope, String> {
    parse_scope(require_str(args, "scope")?)
}

fn parse_scope(raw: &str) -> Result<MemoryScope, String> {
    match raw {
        "project" => Ok(MemoryScope::Project),
        "personal" => Ok(MemoryScope::Personal),
        other => Err(format!("invalid scope '{other}'")),
    }
}

fn scope_name(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Project => "project",
        MemoryScope::Personal => "personal",
    }
}

fn require_str<'a>(args: &'a Value, field: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing string field '{field}'"))
}

fn sanitize_topic_filename(raw: &str, allow_index: bool) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
    {
        return Err(format!("invalid topic filename '{trimmed}'"));
    }
    let normalized = if trimmed.ends_with(".md") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.md")
    };
    if !allow_index && normalized.eq_ignore_ascii_case("MEMORY.md") {
        return Err("use update_index to modify MEMORY.md".to_owned());
    }
    if !normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("unsafe topic characters in '{normalized}'"));
    }
    Ok(normalized)
}

fn list_md_files(dir: &Path) -> Result<Vec<String>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md")
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn atomic_write_text(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn collect_file_hits(path: &Path, query_lower: &str, hits: &mut Vec<Value>) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session.jsonl");
    for (idx, line) in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .enumerate()
    {
        if line.to_ascii_lowercase().contains(query_lower) {
            let excerpt: String = line.chars().take(200).collect();
            hits.push(json!({ "file": name, "line": idx + 1, "excerpt": excerpt }));
            if hits.len() >= MAX_GREP_MATCHES {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_blocks_traversal_and_enforces_index_bounds() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = std::env::temp_dir().join("rein_dream_vault_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions"))?;
        let store = JsonlMemoryStore::dual(dir.join("proj.jsonl"), dir.join("pers.jsonl"));
        let vault = DreamVault::new(
            dir.join("proj_mem"),
            dir.join("pers_mem"),
            dir.join("sessions"),
            dir.clone(),
            store,
        );

        let traversal = vault.execute(&ToolCall {
            id: "1".to_owned(),
            name: "write_vault".to_owned(),
            arguments: json!({"scope": "project", "topic": "../escape", "content": "bad"}),
        });
        assert!(traversal.1);

        let idx_write = vault.execute(&ToolCall {
            id: "2".to_owned(),
            name: "write_vault".to_owned(),
            arguments: json!({"scope": "project", "topic": "MEMORY.md", "content": "bad"}),
        });
        assert!(idx_write.1);

        let ok_write = vault.execute(&ToolCall {
            id: "3".to_owned(),
            name: "write_vault".to_owned(),
            arguments: json!({"scope": "project", "topic": "build-tips", "content": "# Build\ncargo test"}),
        });
        assert!(!ok_write.1);

        let too_many_lines = vec!["- item"; 205].join("\n");
        let bad_idx = vault.execute(&ToolCall {
            id: "4".to_owned(),
            name: "update_index".to_owned(),
            arguments: json!({"scope": "project", "content": too_many_lines}),
        });
        assert!(bad_idx.1);

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn vault_supports_read_delete_grep_docs_and_memory_crud()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_dream_vault_crud_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions"))?;
        std::fs::write(dir.join("README.md"), "# Rein Workspace\n")?;
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n")?;
        std::fs::write(
            dir.join("sessions/s1.jsonl"),
            "{\"msg\":\"hit alpha\"}\n{\"msg\":\"miss\"}\n{\"msg\":\"HIT BETA\"}\n",
        )?;

        let store = JsonlMemoryStore::dual(dir.join("proj.jsonl"), dir.join("pers.jsonl"));
        let vault = DreamVault::new(
            dir.join("proj_mem"),
            dir.join("pers_mem"),
            dir.join("sessions"),
            dir.clone(),
            store,
        );

        let write_ok = vault.execute(&ToolCall {
            id: "w1".to_owned(),
            name: "write_vault".to_owned(),
            arguments: json!({"scope": "project", "topic": "arch", "content": "typestate"}),
        });
        assert!(!write_ok.1);

        let read_ok = vault.execute(&ToolCall {
            id: "r1".to_owned(),
            name: "read_vault".to_owned(),
            arguments: json!({"scope": "project", "topic": "arch"}),
        });
        assert!(!read_ok.1);
        assert_eq!(read_ok.0, "typestate");

        let list_ok = vault.execute(&ToolCall {
            id: "l1".to_owned(),
            name: "list_vault".to_owned(),
            arguments: json!({"scope": "project"}),
        });
        assert!(!list_ok.1);
        assert!(list_ok.0.contains("arch.md"));

        let del_ok = vault.execute(&ToolCall {
            id: "d1".to_owned(),
            name: "delete_vault".to_owned(),
            arguments: json!({"scope": "project", "topic": "arch"}),
        });
        assert!(!del_ok.1);
        assert!(del_ok.0.contains("deleted project/arch.md"));

        let readme = vault.execute(&ToolCall {
            id: "doc1".to_owned(),
            name: "read_project_doc".to_owned(),
            arguments: json!({"filename": "README.md"}),
        });
        assert!(!readme.1);
        assert!(readme.0.contains("Rein Workspace"));

        let blocked_doc = vault.execute(&ToolCall {
            id: "doc2".to_owned(),
            name: "read_project_doc".to_owned(),
            arguments: json!({"filename": "Cargo.toml"}),
        });
        assert!(blocked_doc.1);

        let grep_hits = vault.execute(&ToolCall {
            id: "g1".to_owned(),
            name: "grep_transcripts".to_owned(),
            arguments: json!({"query": "hit"}),
        });
        assert!(!grep_hits.1);
        let parsed_hits: Vec<Value> = serde_json::from_str(&grep_hits.0)?;
        assert_eq!(parsed_hits.len(), 2);

        let oversized_idx = "x".repeat(MAX_INDEX_BYTES + 1);
        let bad_bytes = vault.execute(&ToolCall {
            id: "i1".to_owned(),
            name: "update_index".to_owned(),
            arguments: json!({"scope": "project", "content": oversized_idx}),
        });
        assert!(bad_bytes.1);

        let ok_idx = vault.execute(&ToolCall {
            id: "i2".to_owned(),
            name: "update_index".to_owned(),
            arguments: json!({"scope": "project", "content": "- [arch](arch.md)"}),
        });
        assert!(!ok_idx.1);

        let upserted = vault.execute(&ToolCall {
            id: "m1".to_owned(),
            name: "memory_upsert".to_owned(),
            arguments: json!({"scope": "personal", "key": "editor", "value": "helix", "kind": "user"}),
        });
        assert!(!upserted.1);

        let listed = vault.execute(&ToolCall {
            id: "m2".to_owned(),
            name: "memory_list".to_owned(),
            arguments: json!({"scope": "personal"}),
        });
        assert!(!listed.1);
        assert!(listed.0.contains("helix"));

        let deleted = vault.execute(&ToolCall {
            id: "m3".to_owned(),
            name: "memory_delete".to_owned(),
            arguments: json!({"scope": "personal", "key": "editor"}),
        });
        assert!(!deleted.1);
        assert!(deleted.0.contains("deleted personal:editor"));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
