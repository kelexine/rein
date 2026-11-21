// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-21
// Purpose: web_search tool supporting Parallel AI and Brave Search APIs

use std::path::{Path, PathBuf};
use std::time::Duration;

use rein_policy::Capability;
use serde_json::{Value, json};

use crate::registry::{Tool, ToolError, ToolSchema};
use crate::web::{FETCH_TIMEOUT_SECS, MAX_SEARCH_RESULTS};

const PARALLEL_SEARCH_URL: &str = "https://api.parallel.ai/v1beta/search";
const PARALLEL_BETA_HEADER: &str = "search-extract-2025-10-10";
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const MAX_EXCERPT_CHARS: u32 = 10_000;
const DEFAULT_MAX_RESULTS: u8 = 5;

/// Search backend selection for [`WebSearchTool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackend {
    /// Prefer Parallel AI when configured, falling back to Brave Search.
    Auto,
    /// Force Parallel AI (`https://api.parallel.ai/v1beta/search`).
    Parallel,
    /// Force Brave Search (`https://api.search.brave.com/res/v1/web/search`).
    Brave,
}

impl SearchBackend {
    fn parse(raw: Option<&str>) -> Result<Self, ToolError> {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("auto") => Ok(Self::Auto),
            Some("parallel") => Ok(Self::Parallel),
            Some("brave") => Ok(Self::Brave),
            Some(other) => Err(ToolError::InvalidArguments {
                tool: "web_search".to_owned(),
                reason: format!("unknown provider '{other}' (expected auto, parallel, or brave)"),
            }),
        }
    }
}

/// Read-only web search tool supporting Parallel AI and Brave Search.
#[derive(Debug, Clone, Default)]
pub struct WebSearchTool {
    credentials_dir: Option<PathBuf>,
}

impl WebSearchTool {
    /// Creates a new [`WebSearchTool`] resolving API keys from environment variables.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            credentials_dir: None,
        }
    }

    /// Configures a credentials directory (e.g. `~/.rein/credentials`) checked before env vars.
    #[must_use]
    pub fn with_credentials_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.credentials_dir = Some(dir.into());
        self
    }

    fn resolve_key(&self, cred_name: &str, env_var: &str) -> Option<String> {
        if let Some(dir) = &self.credentials_dir
            && let Some(key) = read_credential_file(&dir.join(cred_name))
        {
            return Some(key);
        }
        std::env::var(env_var)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    }
}

fn read_credential_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn build_http_agent() -> ureq::Agent {
    let cfg = ureq::config::Config::builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(FETCH_TIMEOUT_SECS)))
        .build();
    ureq::Agent::new_with_config(cfg)
}

fn parse_search_args(arguments: &Value) -> Result<(String, u8, SearchBackend), ToolError> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "web_search".to_owned(),
            reason: "missing non-empty string field 'query'".to_owned(),
        })?
        .to_owned();

    let max_results = match arguments.get("max_results").and_then(Value::as_u64) {
        None | Some(0) => DEFAULT_MAX_RESULTS,
        Some(n) if n <= u64::from(MAX_SEARCH_RESULTS) => n as u8,
        Some(n) => {
            return Err(ToolError::InvalidArguments {
                tool: "web_search".to_owned(),
                reason: format!("max_results {n} exceeds limit of {MAX_SEARCH_RESULTS}"),
            });
        }
    };

    let backend = SearchBackend::parse(arguments.get("provider").and_then(Value::as_str))?;
    Ok((query, max_results, backend))
}

impl Tool for WebSearchTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_search".to_owned(),
            description: "Search the web using Parallel AI Search or Brave Search. Returns numbered Markdown results with titles, URLs, and excerpts.".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum number of results to return (1-20, default 5)."
                    },
                    "provider": {
                        "type": "string",
                        "enum": ["auto", "parallel", "brave"],
                        "description": "Search backend to use (default: auto — prefers Parallel AI, falls back to Brave Search)."
                    }
                }
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let (query, _, _) = parse_search_args(arguments)?;
        Ok(Capability::WebSearch { query })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let (query, max_results, backend) = parse_search_args(arguments)?;
        match backend {
            SearchBackend::Parallel => {
                let key = self
                    .resolve_key("parallel", "PARALLEL_API_KEY")
                    .ok_or_else(|| missing_key_err("PARALLEL_API_KEY", "parallel"))?;
                search_parallel(&query, max_results, &key)
            }
            SearchBackend::Brave => {
                let key = self
                    .resolve_key("brave", "BRAVE_SEARCH_API_KEY")
                    .ok_or_else(|| missing_key_err("BRAVE_SEARCH_API_KEY", "brave"))?;
                search_brave(&query, max_results, &key)
            }
            SearchBackend::Auto => {
                if let Some(key) = self.resolve_key("parallel", "PARALLEL_API_KEY") {
                    return search_parallel(&query, max_results, &key);
                }
                if let Some(key) = self.resolve_key("brave", "BRAVE_SEARCH_API_KEY") {
                    return search_brave(&query, max_results, &key);
                }
                Err(ToolError::Execution {
                    tool: "web_search".to_owned(),
                    reason: "neither PARALLEL_API_KEY nor BRAVE_SEARCH_API_KEY is set (or stored in ~/.rein/credentials/{parallel,brave})".to_owned(),
                })
            }
        }
    }
}

fn missing_key_err(env_var: &str, cred_name: &str) -> ToolError {
    ToolError::Execution {
        tool: "web_search".to_owned(),
        reason: format!("{env_var} is not set and ~/.rein/credentials/{cred_name} was not found"),
    }
}

fn search_parallel(query: &str, max_results: u8, api_key: &str) -> Result<String, ToolError> {
    let payload = json!({
        "objective": query,
        "search_queries": [query],
        "max_results": max_results,
        "excerpts": {
            "max_chars_per_result": MAX_EXCERPT_CHARS
        }
    });
    let body_str = serde_json::to_string(&payload)?;
    let resp = build_http_agent()
        .post(PARALLEL_SEARCH_URL)
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("parallel-beta", PARALLEL_BETA_HEADER)
        .send(body_str)
        .map_err(|err| ToolError::Execution {
            tool: "web_search".to_owned(),
            reason: format!("Parallel search request failed: {err}"),
        })?;

    let status = resp.status().as_u16();
    let text = resp
        .into_body()
        .read_to_string()
        .map_err(|err| ToolError::Execution {
            tool: "web_search".to_owned(),
            reason: format!("failed to read Parallel response: {err}"),
        })?;

    if !(200..300).contains(&status) {
        return Err(ToolError::Execution {
            tool: "web_search".to_owned(),
            reason: format!("Parallel API error {status}: {text}"),
        });
    }

    let data: Value = serde_json::from_str(&text)?;
    Ok(format_parallel_results(&data))
}

fn search_brave(query: &str, max_results: u8, api_key: &str) -> Result<String, ToolError> {
    let count_str = max_results.to_string();
    let resp = build_http_agent()
        .get(BRAVE_SEARCH_URL)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .query("q", query)
        .query("count", &count_str)
        .call()
        .map_err(|err| ToolError::Execution {
            tool: "web_search".to_owned(),
            reason: format!("Brave search request failed: {err}"),
        })?;

    let status = resp.status().as_u16();
    let text = resp
        .into_body()
        .read_to_string()
        .map_err(|err| ToolError::Execution {
            tool: "web_search".to_owned(),
            reason: format!("failed to read Brave response: {err}"),
        })?;

    if !(200..300).contains(&status) {
        return Err(ToolError::Execution {
            tool: "web_search".to_owned(),
            reason: format!("Brave API error {status}: {text}"),
        });
    }

    let data: Value = serde_json::from_str(&text)?;
    Ok(format_brave_results(&data))
}

/// Formats a Parallel AI search JSON response into numbered Markdown.
#[must_use]
pub fn format_parallel_results(data: &Value) -> String {
    let Some(results) = data.get("results").and_then(Value::as_array) else {
        return "No search results returned from Parallel API.".to_owned();
    };
    if results.is_empty() {
        return "No search results returned from Parallel API.".to_owned();
    }

    let mut out = String::new();
    for (idx, result) in results.iter().enumerate() {
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Unknown Title");
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("Unknown URL");
        out.push_str(&format!("{}. [{}]({})\n", idx + 1, title, url));

        if let Some(excerpts) = result.get("excerpts").and_then(Value::as_array) {
            for excerpt in excerpts.iter().filter_map(Value::as_str) {
                let indented = excerpt.trim().replace('\n', "\n   > ");
                out.push_str(&format!("   > {indented}\n"));
            }
        }
        out.push('\n');
    }
    out.trim().to_owned()
}

/// Formats a Brave Search JSON response into numbered Markdown.
#[must_use]
pub fn format_brave_results(data: &Value) -> String {
    let Some(results) = data
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(Value::as_array)
    else {
        return "No search results returned from Brave Search API.".to_owned();
    };
    if results.is_empty() {
        return "No search results returned from Brave Search API.".to_owned();
    }

    let mut out = String::new();
    for (idx, result) in results.iter().enumerate() {
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Unknown Title");
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("Unknown URL");
        let desc = result
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");

        let clean = clean_brave_html(desc);
        out.push_str(&format!("{}. [{}]({})\n", idx + 1, title, url));
        if !clean.is_empty() {
            let indented = clean.trim().replace('\n', "\n   > ");
            out.push_str(&format!("   > {indented}\n"));
        }
        out.push('\n');
    }
    out.trim().to_owned()
}

fn clean_brave_html(input: &str) -> String {
    input
        .replace("<strong>", "**")
        .replace("</strong>", "**")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_and_excess_max_results_rejected() {
        let tool = WebSearchTool::new();
        assert!(tool.required_capability(&json!({"query": "   "})).is_err());
        assert!(
            tool.required_capability(&json!({"query": "rust", "max_results": 21}))
                .is_err()
        );
        assert!(
            tool.required_capability(&json!({"query": "rust", "provider": "invalid"}))
                .is_err()
        );
    }

    #[test]
    fn credentials_dir_resolves_keys() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_tools_web_search_creds_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("parallel"), "  par-key-123 \n")?;
        std::fs::write(dir.join("brave"), "brave-key-456")?;

        let tool = WebSearchTool::new().with_credentials_dir(&dir);
        assert_eq!(
            tool.resolve_key("parallel", "NONEXISTENT_ENV_PARALLEL"),
            Some("par-key-123".to_owned())
        );
        assert_eq!(
            tool.resolve_key("brave", "NONEXISTENT_ENV_BRAVE"),
            Some("brave-key-456".to_owned())
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn format_parallel_handles_excerpts_and_empty() {
        assert!(format_parallel_results(&json!({"results": []})).contains("No search results"));
        let out = format_parallel_results(&json!({
            "results": [{
                "title": "Rust",
                "url": "https://rust-lang.org",
                "excerpts": ["line1\nline2"]
            }]
        }));
        assert!(out.contains("1. [Rust](https://rust-lang.org)"));
        assert!(out.contains("   > line1\n   > line2"));
    }

    #[test]
    fn format_brave_cleans_entities_and_tags() {
        let out = format_brave_results(&json!({
            "web": {
                "results": [{
                    "title": "Brave",
                    "url": "https://example.com",
                    "description": "Hello <strong>world</strong> &amp; &#x27;test&#x27;"
                }]
            }
        }));
        assert!(out.contains("1. [Brave](https://example.com)"));
        assert!(out.contains("   > Hello **world** & 'test'"));
    }
}
