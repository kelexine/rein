// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Model Context Protocol (MCP) stdio client and policy-gated tool adapter

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rein_policy::Capability;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::registry::{Tool, ToolError, ToolSchema};

/// Environment variables scrubbed by default when spawning external MCP server subprocesses.
const SCRUBBED_SECRET_ENV_VARS: &[&str] = &[
    "GEMINI_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENROUTER_API_KEY",
    "ZAI_API_KEY",
];

/// Configuration for a stdio-attached MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Logical server name.
    pub name: String,
    /// Executable command for spawning the MCP server.
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
}

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpSession {
    fn spawn(
        config: &McpServerConfig,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<Self, ToolError> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for secret_var in SCRUBBED_SECRET_ENV_VARS {
            cmd.env_remove(secret_var);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|source| ToolError::Io {
            path: PathBuf::from(&config.command),
            source,
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: config.name.clone(),
                reason: "failed to capture MCP server stdin".to_owned(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: config.name.clone(),
                reason: "failed to capture MCP server stdout".to_owned(),
            })?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn exchange(
        &mut self,
        config: &McpServerConfig,
        line: &str,
    ) -> Result<Option<String>, ToolError> {
        if writeln!(self.stdin, "{line}").is_err() || self.stdin.flush().is_err() {
            return Ok(None);
        }
        loop {
            let mut response_line = String::new();
            let bytes = self
                .stdout
                .read_line(&mut response_line)
                .map_err(|source| ToolError::Io {
                    path: PathBuf::from(&config.command),
                    source,
                })?;
            if bytes == 0 {
                return Ok(None);
            }
            let trimmed = response_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Skip server-initiated JSON-RPC notifications (messages with "method" and no "id").
            if let Ok(val) = serde_json::from_str::<Value>(trimmed)
                && val.get("id").is_none()
                && val.get("method").is_some()
            {
                continue;
            }
            return Ok(Some(response_line));
        }
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Stdio MCP client maintaining a persistent JSON-RPC 2.0 session with automatic reconnect fallback.
#[derive(Clone)]
pub struct McpClient {
    config: McpServerConfig,
    extra_env: BTreeMap<String, String>,
    session: Arc<Mutex<Option<McpSession>>>,
    next_id: Arc<AtomicU64>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Creates a new [`McpClient`] from `config`.
    #[must_use]
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            extra_env: BTreeMap::new(),
            session: Arc::new(Mutex::new(None)),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Passes an explicit environment variable to the spawned MCP server subprocess.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.insert(key.into(), value.into());
        self
    }

    /// Returns the server configuration.
    #[must_use]
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Allocates the next monotonic JSON-RPC request ID for this client.
    #[must_use]
    pub fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Sends the MCP `initialize` handshake and `notifications/initialized` notification on the persistent pipe.
    pub fn initialize(&self) -> Result<JsonRpcResponse, ToolError> {
        let req = JsonRpcRequest::new(
            self.next_request_id(),
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "rein",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        );
        let resp = self.send_request(&req)?;
        if let Ok(mut guard) = self.session.lock()
            && let Some(active) = guard.as_mut()
        {
            let notif = json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            });
            let _ = writeln!(active.stdin, "{notif}");
            let _ = active.stdin.flush();
        }
        Ok(resp)
    }

    /// Queries `tools/list` on the configured MCP server and adapts each schema.
    pub fn list_tools(&self) -> Result<Vec<ToolSchema>, ToolError> {
        let req = JsonRpcRequest::new(self.next_request_id(), "tools/list", json!({}));
        let resp = self.send_request(&req)?;
        if let Some(err) = resp.error {
            return Err(ToolError::InvalidArguments {
                tool: self.config.name.clone(),
                reason: format!("MCP tools/list failed: {err}"),
            });
        }
        let raw_tools = resp
            .result
            .as_ref()
            .and_then(|res| res.get("tools"))
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: self.config.name.clone(),
                reason: "MCP tools/list response missing 'result.tools' array".to_owned(),
            })?;
        let mut schemas = Vec::with_capacity(raw_tools.len());
        for entry in raw_tools {
            let wire: McpWireTool = serde_json::from_value(entry.clone())?;
            schemas.push(ToolSchema {
                name: wire.name,
                description: wire.description.unwrap_or_default(),
                input_schema: wire
                    .input_schema
                    .unwrap_or_else(|| json!({"type": "object"})),
            });
        }
        Ok(schemas)
    }

    /// Sends a JSON-RPC 2.0 request over the persistent stdio pipe, spawning or reconnecting if needed.
    pub fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse, ToolError> {
        let line = serde_json::to_string(request)?;
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(active) = guard.as_mut() {
            if let Some(resp_line) = active.exchange(&self.config, &line)? {
                return Ok(serde_json::from_str(&resp_line)?);
            }
            *guard = None;
        }

        let mut fresh = McpSession::spawn(&self.config, &self.extra_env)?;
        let resp_line = fresh.exchange(&self.config, &line)?.unwrap_or_default();
        *guard = Some(fresh);
        Ok(serde_json::from_str(&resp_line)?)
    }
}

#[derive(Debug, Deserialize)]
struct McpWireTool {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "inputSchema", alias = "input_schema", default)]
    input_schema: Option<Value>,
}

/// Wraps an MCP server tool so it conforms to [`Tool`] and passes through [`Capability::Mcp`].
#[derive(Debug, Clone)]
pub struct McpToolAdapter {
    client: McpClient,
    remote_name: String,
    schema: ToolSchema,
}

impl McpToolAdapter {
    /// Wraps a discovered MCP tool schema under `client`.
    #[must_use]
    pub fn new(client: McpClient, schema: ToolSchema) -> Self {
        let remote_name = schema.name.clone();
        Self {
            client,
            remote_name,
            schema,
        }
    }

    /// Wraps a discovered MCP tool schema using `registered_name` in Rein's [`crate::ToolRegistry`]
    /// while preserving the remote tool's original name for `tools/call` wire requests.
    #[must_use]
    pub fn with_registered_name(
        client: McpClient,
        mut schema: ToolSchema,
        registered_name: impl Into<String>,
    ) -> Self {
        let remote_name = schema.name.clone();
        schema.name = registered_name.into();
        Self {
            client,
            remote_name,
            schema,
        }
    }

    /// Formats a collision-safe namespaced tool identifier (`mcp__<server>__<tool>`).
    #[must_use]
    pub fn namespaced_name(server: &str, tool: &str) -> String {
        let sanitize = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        format!("mcp__{}__{}", sanitize(server), sanitize(tool))
    }

    /// Returns the original tool name exposed by the remote MCP server.
    #[must_use]
    pub fn remote_name(&self) -> &str {
        &self.remote_name
    }
}

impl Tool for McpToolAdapter {
    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    fn required_capability(&self, _arguments: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Mcp {
            server: self.client.config().name.clone(),
            tool: self.remote_name.clone(),
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let req = JsonRpcRequest::new(
            self.client.next_request_id(),
            "tools/call",
            json!({
                "name": self.remote_name,
                "arguments": arguments,
            }),
        );
        let resp = self.client.send_request(&req)?;
        Ok(serde_json::to_string(&resp)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_roundtrip_against_fake_server() -> Result<(), Box<dyn std::error::Error>> {
        // Fake MCP server: consumes one request line, echoes a fixed JSON-RPC result.
        let client = McpClient::new(McpServerConfig {
            name: "fake".to_owned(),
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                r#"read -r _; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"content":"pong"}}'"#.to_owned(),
            ],
        });
        let adapter = McpToolAdapter::new(
            client,
            ToolSchema {
                name: "ping".to_owned(),
                description: "ping".to_owned(),
                input_schema: json!({"type": "object"}),
            },
        );
        assert!(matches!(
            adapter.required_capability(&json!({}))?,
            Capability::Mcp { ref server, ref tool } if server == "fake" && tool == "ping"
        ));
        let out = adapter.execute(&json!({"x": 1}))?;
        let resp: JsonRpcResponse = serde_json::from_str(&out)?;
        assert_eq!(resp.id, 1);
        assert_eq!(resp.result, Some(json!({"content": "pong"})));
        Ok(())
    }

    #[test]
    fn lists_tools_from_fake_server() -> Result<(), Box<dyn std::error::Error>> {
        let client = McpClient::new(McpServerConfig {
            name: "fake-list".to_owned(),
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                r#"read -r _; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"echo","description":"echoes text","inputSchema":{"type":"object"}}]}}'"#.to_owned(),
            ],
        });
        let schemas = client.list_tools()?;
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
        assert_eq!(schemas[0].description, "echoes text");
        Ok(())
    }

    #[test]
    fn reports_errors_for_malformed_json_or_missing_binary() {
        let bad_json_client = McpClient::new(McpServerConfig {
            name: "bad-json".to_owned(),
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), "printf 'not-json\\n'".to_owned()],
        });
        assert!(bad_json_client.list_tools().is_err());

        let missing_bin_client = McpClient::new(McpServerConfig {
            name: "missing".to_owned(),
            command: "rein_nonexistent_mcp_binary_xyz".to_owned(),
            args: Vec::new(),
        });
        assert!(missing_bin_client.list_tools().is_err());
    }

    #[test]
    fn persistent_session_reuses_child_skips_notifications_and_scrubs_secrets()
    -> Result<(), Box<dyn std::error::Error>> {
        // Stateful server loop that increments a counter in the same process and checks secret scrubbing.
        let script = r#"
count=0
while IFS= read -r line; do
  case "$line" in
    *'"method":"notifications/initialized"'*)
      continue
      ;;
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}'
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}'
      ;;
    *'"method":"tools/call"'*)
      count=$((count + 1))
      leaked="${OPENAI_API_KEY:-}${ANTHROPIC_API_KEY:-}"
      printf '{"jsonrpc":"2.0","id":%d,"result":{"call_count":%d,"leaked":"%s","custom":"%s"}}\n' "$count" "$count" "$leaked" "${MCP_CUSTOM_VAR:-}"
      ;;
  esac
done
"#;
        let client = McpClient::new(McpServerConfig {
            name: "stateful".to_owned(),
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), script.to_owned()],
        })
        .with_env("OPENAI_API_KEY", "should_be_overridden_only_if_explicit")
        .with_env("MCP_CUSTOM_VAR", "allowed_val");

        // Also test default secret scrubbing without explicit override.
        let scrubbed_client = McpClient::new(McpServerConfig {
            name: "stateful_scrubbed".to_owned(),
            command: "sh".to_owned(),
            args: vec!["-c".to_owned(), script.to_owned()],
        })
        .with_env("MCP_CUSTOM_VAR", "ok");

        let init_resp = scrubbed_client.initialize()?;
        assert!(init_resp.result.is_some());

        let namespaced = McpToolAdapter::namespaced_name("stateful_scrubbed", "read");
        assert_eq!(namespaced, "mcp__stateful_scrubbed__read");

        let adapter = McpToolAdapter::with_registered_name(
            scrubbed_client,
            ToolSchema {
                name: "read".to_owned(),
                description: "remote read".to_owned(),
                input_schema: json!({"type": "object"}),
            },
            &namespaced,
        );
        assert_eq!(adapter.schema().name, "mcp__stateful_scrubbed__read");
        assert_eq!(adapter.remote_name(), "read");

        let first: JsonRpcResponse = serde_json::from_str(&adapter.execute(&json!({}))?)?;
        let second: JsonRpcResponse = serde_json::from_str(&adapter.execute(&json!({}))?)?;
        // Both calls were served by the same persistent child process (call_count increments from 1 to 2).
        assert_eq!(
            first.result.as_ref().and_then(|v| v.get("call_count")),
            Some(&json!(1))
        );
        assert_eq!(
            second.result.as_ref().and_then(|v| v.get("call_count")),
            Some(&json!(2))
        );
        assert_eq!(
            second.result.as_ref().and_then(|v| v.get("custom")),
            Some(&json!("ok"))
        );
        drop(client);
        Ok(())
    }
}
