// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-15
// Purpose: Global session-scoped Todo & Task management tools with named storage and auto-clear

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use rein_policy::{AccessMode, Capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};

const MAX_SLUG_LEN: usize = 40;

/// Execution status of a tracked task item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task has not started yet.
    #[default]
    Pending,
    /// Task is actively being worked on.
    InProgress,
    /// Task has finished.
    Completed,
}

/// Single task entry managed by [`TodoWriteTool`] and [`TodoReadTool`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// Stable short identifier (auto-numbered if omitted by callers).
    pub id: String,
    /// Clear, actionable task description.
    pub content: String,
    /// Current execution state (`pending`, `in_progress`, or `completed`).
    #[serde(default)]
    pub status: TodoStatus,
}

/// Loaded named todo checklist state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoListState {
    /// Descriptive checklist title (used for file naming and TUI display).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Ordered task checklist items.
    #[serde(default)]
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Clone)]
enum StoreBackend {
    DirectFile(PathBuf),
    SessionDir {
        dir: PathBuf,
        session_uid: Arc<RwLock<String>>,
    },
}

/// Persistent JSON todo list helper supporting direct paths or global session-scoped named files (`~/.rein/todos/<uid>-<slug>.json`).
#[derive(Debug, Clone)]
pub struct TodoStore {
    backend: StoreBackend,
}

impl TodoStore {
    /// Creates a [`TodoStore`] reading and writing a specific `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            backend: StoreBackend::DirectFile(path.into()),
        }
    }

    /// Creates a session-scoped [`TodoStore`] inside global `todos_dir` (`<uid>-<slug>.json`).
    #[must_use]
    pub fn for_session(todos_dir: impl Into<PathBuf>, session_uid: impl Into<String>) -> Self {
        Self {
            backend: StoreBackend::SessionDir {
                dir: todos_dir.into(),
                session_uid: Arc::new(RwLock::new(session_uid.into())),
            },
        }
    }

    /// Updates the active session UID on shared session-scoped stores (used on `/clear` and `/resume`).
    pub fn set_session_uid(&self, new_uid: impl Into<String>) {
        if let StoreBackend::SessionDir { session_uid, .. } = &self.backend
            && let Ok(mut guard) = session_uid.write()
        {
            *guard = new_uid.into();
        }
    }

    fn current_session_uid(session_uid: &Arc<RwLock<String>>) -> String {
        session_uid
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Resolves the currently active file path on disk (or default session target path if none exists yet).
    #[must_use]
    pub fn path(&self) -> PathBuf {
        match &self.backend {
            StoreBackend::DirectFile(path) => path.clone(),
            StoreBackend::SessionDir { dir, session_uid } => {
                let uid = Self::current_session_uid(session_uid);
                find_session_todo_file(dir, &uid).unwrap_or_else(|| dir.join(format!("{uid}.json")))
            }
        }
    }

    /// Loads stored [`TodoItem`] entries (empty when absent).
    pub fn load(&self) -> Result<Vec<TodoItem>, ToolError> {
        Ok(self.load_state()?.todos)
    }

    /// Loads both optional checklist title and [`TodoItem`] entries.
    pub fn load_state(&self) -> Result<TodoListState, ToolError> {
        let target = self.path();
        if !target.exists() {
            return Ok(TodoListState::default());
        }
        let raw = fs::read_to_string(&target).map_err(|source| ToolError::Io {
            path: target.clone(),
            source,
        })?;
        if raw.trim().is_empty() {
            return Ok(TodoListState::default());
        }
        if let Ok(state) = serde_json::from_str::<TodoListState>(&raw) {
            return Ok(state);
        }
        let todos: Vec<TodoItem> = serde_json::from_str(&raw)?;
        Ok(TodoListState { title: None, todos })
    }

    /// Replaces stored todos without changing the existing title.
    pub fn save(&self, items: &[TodoItem]) -> Result<(), ToolError> {
        let _ = self.save_named(None, items)?;
        Ok(())
    }

    /// Saves a named checklist, or removes/clears stored todos when `items` is empty or all completed.
    pub fn save_named(
        &self,
        title: Option<&str>,
        items: &[TodoItem],
    ) -> Result<Option<String>, ToolError> {
        let all_done = !items.is_empty() && items.iter().all(|t| t.status == TodoStatus::Completed);
        if items.is_empty() || all_done {
            self.clear()?;
            return Ok(None);
        }
        let resolved_title = resolve_title(title, &self.load_state().ok(), items);
        let state = TodoListState {
            title: resolved_title.clone(),
            todos: items.to_vec(),
        };
        let target = self.target_path_for_title(resolved_title.as_deref());
        write_state_atomic(&target, &state)?;
        if let StoreBackend::SessionDir { dir, session_uid } = &self.backend {
            let uid = Self::current_session_uid(session_uid);
            remove_other_session_files(dir, &uid, &target);
        }
        Ok(resolved_title)
    }

    /// Clears stored todos (removes session-scoped file or truncates direct file to `[]`).
    pub fn clear(&self) -> Result<(), ToolError> {
        match &self.backend {
            StoreBackend::DirectFile(path) => {
                if path.exists() {
                    write_json_string(path, "[]\n")?;
                }
            }
            StoreBackend::SessionDir { dir, session_uid } => {
                let uid = Self::current_session_uid(session_uid);
                for existing in list_session_todo_files(dir, &uid) {
                    let _ = fs::remove_file(existing);
                }
            }
        }
        Ok(())
    }

    fn target_path_for_title(&self, title: Option<&str>) -> PathBuf {
        match &self.backend {
            StoreBackend::DirectFile(path) => path.clone(),
            StoreBackend::SessionDir { dir, session_uid } => {
                let uid = Self::current_session_uid(session_uid);
                let slug = title.map(slugify_title).filter(|s| !s.is_empty());
                match slug {
                    Some(s) => dir.join(format!("{uid}-{s}.json")),
                    None => dir.join(format!("{uid}.json")),
                }
            }
        }
    }
}

/// Tool replacing/updating the structured session todo checklist (`todo_write`).
#[derive(Debug, Clone)]
pub struct TodoWriteTool {
    store: TodoStore,
}

impl TodoWriteTool {
    /// Creates a [`TodoWriteTool`] backed by a direct file `store_path`.
    #[must_use]
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store: TodoStore::new(store_path),
        }
    }

    /// Creates a [`TodoWriteTool`] using an arbitrary [`TodoStore`] (such as session-scoped global storage).
    #[must_use]
    pub fn with_store(store: TodoStore) -> Self {
        Self { store }
    }
}

impl Tool for TodoWriteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "todo_write".to_owned(),
            description: "Create or update the persistent session task checklist (stored globally by session name and cleared automatically once all tasks are completed).".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Concise task-list name describing the work in progress"
                    },
                    "todos": {
                        "type": "array",
                        "description": "Ordered list of todos reflecting the full current plan",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "content": { "type": "string" },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                }
                            },
                            "required": ["content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    fn required_capability(&self, _args: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Todo {
            mode: AccessMode::Write,
        })
    }

    fn execute(&self, args: &Value) -> Result<String, ToolError> {
        let title = args
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let parsed = parse_todo_items(args)?;
        let total = parsed.len();
        let all_done = total > 0 && parsed.iter().all(|t| t.status == TodoStatus::Completed);
        let cleared = parsed.is_empty() || all_done;
        let saved_title = self.store.save_named(title, &parsed)?;

        let payload = if cleared {
            json!({
                "title": Value::Null,
                "todos": [],
                "cleared": true,
                "summary": format!("all {total} tasks completed and cleared"),
            })
        } else {
            let done = parsed
                .iter()
                .filter(|t| t.status == TodoStatus::Completed)
                .count();
            let active = parsed
                .iter()
                .filter(|t| t.status == TodoStatus::InProgress)
                .count();
            let pending = parsed
                .iter()
                .filter(|t| t.status == TodoStatus::Pending)
                .count();
            json!({
                "title": saved_title,
                "todos": parsed,
                "cleared": false,
                "summary": format!("{total} tasks ({done} completed, {active} in progress, {pending} pending)"),
            })
        };
        Ok(serde_json::to_string_pretty(&payload)?)
    }
}

/// Tool reading the current structured session todo checklist (`todo_read`).
#[derive(Debug, Clone)]
pub struct TodoReadTool {
    store: TodoStore,
}

impl TodoReadTool {
    /// Creates a [`TodoReadTool`] backed by a direct file `store_path`.
    #[must_use]
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store: TodoStore::new(store_path),
        }
    }

    /// Creates a [`TodoReadTool`] backed by `store`.
    #[must_use]
    pub fn with_store(store: TodoStore) -> Self {
        Self { store }
    }
}

impl Tool for TodoReadTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "todo_read".to_owned(),
            description: "Read the current session task checklist".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    fn required_capability(&self, _args: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Todo {
            mode: AccessMode::Read,
        })
    }

    fn execute(&self, _args: &Value) -> Result<String, ToolError> {
        let state = self.store.load_state()?;
        let payload = json!({
            "title": state.title,
            "todos": state.todos,
            "cleared": state.todos.is_empty(),
        });
        Ok(serde_json::to_string_pretty(&payload)?)
    }
}

fn parse_todo_items(args: &Value) -> Result<Vec<TodoItem>, ToolError> {
    let raw_items =
        args.get("todos")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "todo_write".to_owned(),
                reason: "missing array 'todos'".to_owned(),
            })?;
    let mut parsed = Vec::with_capacity(raw_items.len());
    for (idx, item) in raw_items.iter().enumerate() {
        let content = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "todo_write".to_owned(),
                reason: format!("todos[{idx}] missing non-empty 'content'"),
            })?;
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| (idx + 1).to_string());
        let status = match item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
        {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" | "done" => TodoStatus::Completed,
            other => {
                return Err(ToolError::InvalidArguments {
                    tool: "todo_write".to_owned(),
                    reason: format!("todos[{idx}] has invalid status '{other}'"),
                });
            }
        };
        parsed.push(TodoItem {
            id,
            content: content.to_owned(),
            status,
        });
    }
    Ok(parsed)
}

fn resolve_title(
    explicit: Option<&str>,
    existing: &Option<TodoListState>,
    items: &[TodoItem],
) -> Option<String> {
    if let Some(title) = explicit {
        return Some(title.to_owned());
    }
    if let Some(prev) = existing.as_ref().and_then(|s| s.title.as_deref())
        && !prev.trim().is_empty()
    {
        return Some(prev.to_owned());
    }
    items
        .iter()
        .find(|t| t.status == TodoStatus::InProgress)
        .or_else(|| items.first())
        .map(|t| t.content.clone())
}

fn slugify_title(raw: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = true;
    for ch in raw.chars() {
        if slug.len() >= MAX_SLUG_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_matches('-').to_owned()
}

fn list_session_todo_files(dir: &Path, session_uid: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let exact = format!("{session_uid}.json");
    let prefix = format!("{session_uid}-");
    let mut matches: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|path| {
            path.extension().and_then(|e| e.to_str()) == Some("json")
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name == exact || name.starts_with(&prefix))
        })
        .collect();
    matches.sort();
    matches
}

fn find_session_todo_file(dir: &Path, session_uid: &str) -> Option<PathBuf> {
    list_session_todo_files(dir, session_uid).into_iter().next()
}

fn remove_other_session_files(dir: &Path, session_uid: &str, keep: &Path) {
    for path in list_session_todo_files(dir, session_uid) {
        if path != keep {
            let _ = fs::remove_file(path);
        }
    }
}

fn write_state_atomic(path: &Path, state: &TodoListState) -> Result<(), ToolError> {
    let encoded = serde_json::to_string_pretty(state)?;
    write_json_string(path, &format!("{encoded}\n"))
}

static TODO_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn write_json_string(path: &Path, content: &str) -> Result<(), ToolError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| ToolError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let seq = TODO_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("todo.json");
    let tmp = path.with_file_name(format!(".{file_name}.{pid}.{seq}.tmp"));
    fs::write(&tmp, content).map_err(|source| ToolError::Io {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, path).map_err(|source| ToolError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_write_names_session_file_and_clears_when_all_completed()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("rein_tools_todo_session_test");
        let _ = fs::remove_dir_all(&temp_dir);
        let store = TodoStore::for_session(&temp_dir, "a1b2c3d4");
        let writer = TodoWriteTool::with_store(store.clone());
        let reader = TodoReadTool::with_store(store.clone());

        let initial: Value = serde_json::from_str(&reader.execute(&json!({}))?)?;
        assert_eq!(initial["todos"], json!([]));

        let out_str = writer.execute(&json!({
            "title": "Refactor Memory System",
            "todos": [
                { "content": "Inspect codebase", "status": "completed" },
                { "content": "Render bottom bar", "status": "in_progress" },
                { "id": "custom-3", "content": "Run tests", "status": "pending" }
            ]
        }))?;
        let out: Value = serde_json::from_str(&out_str)?;
        assert_eq!(out["cleared"], json!(false));
        assert_eq!(out["title"], json!("Refactor Memory System"));
        assert_eq!(out["todos"][0]["id"], json!("1"));
        assert_eq!(out["todos"][2]["id"], json!("custom-3"));

        let expected_file = temp_dir.join("a1b2c3d4-refactor-memory-system.json");
        assert!(expected_file.exists());
        assert_eq!(store.path(), expected_file);

        let loaded: Value = serde_json::from_str(&reader.execute(&json!({}))?)?;
        assert_eq!(loaded["todos"].as_array().map(Vec::len), Some(3));
        assert_eq!(loaded["title"], json!("Refactor Memory System"));

        // Marking all tasks completed automatically clears and removes the session file.
        let done_str = writer.execute(&json!({
            "todos": [
                { "content": "Inspect codebase", "status": "completed" },
                { "content": "Render bottom bar", "status": "completed" },
                { "content": "Run tests", "status": "completed" }
            ]
        }))?;
        let done: Value = serde_json::from_str(&done_str)?;
        assert_eq!(done["cleared"], json!(true));
        assert_eq!(done["todos"], json!([]));
        assert!(!expected_file.exists());

        let after: Value = serde_json::from_str(&reader.execute(&json!({}))?)?;
        assert_eq!(after["todos"], json!([]));
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn todo_write_rejects_invalid_statuses_and_missing_content() {
        let temp_dir = std::env::temp_dir().join("rein_tools_todo_validation_test");
        let _ = fs::remove_dir_all(&temp_dir);
        let store = TodoStore::for_session(&temp_dir, "validation");
        let writer = TodoWriteTool::with_store(store);

        assert!(writer.execute(&json!({})).is_err());
        assert!(
            writer
                .execute(&json!({
                    "todos": [{ "content": "Bad status", "status": "blocked" }]
                }))
                .is_err()
        );
        assert!(
            writer
                .execute(&json!({
                    "todos": [{ "status": "pending" }]
                }))
                .is_err()
        );
        assert!(
            writer
                .execute(&json!({
                    "todos": [{ "content": "   ", "status": "pending" }]
                }))
                .is_err()
        );
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
