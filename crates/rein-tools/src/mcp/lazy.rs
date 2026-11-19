// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-19
// Purpose: Lazy MCP tool schema disk catalog and generic `call_mcp_tool` dispatcher

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rein_policy::Capability;
use serde_json::{Value, json};

use crate::mcp::client::McpClient;
use crate::mcp::protocol::JsonRpcRequest;
use crate::registry::{Tool, ToolError, ToolSchema};

/// Lightweight summary of one discovered MCP server and its lazy-loaded tool names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSummary {
    /// Logical MCP server name.
    pub server_name: String,
    /// Exposed remote tool names.
    pub tools: Vec<String>,
    /// Directory where `<tool>.json` schema files were written.
    pub schema_dir: PathBuf,
}

#[derive(Default)]
struct CatalogInner {
    clients: BTreeMap<String, McpClient>,
    summaries: Vec<McpServerSummary>,
}

/// Shared catalog holding connected MCP server clients and cached schema files on disk.
#[derive(Clone)]
pub struct McpCatalog {
    schemas_root: PathBuf,
    inner: Arc<Mutex<CatalogInner>>,
}

impl std::fmt::Debug for McpCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCatalog")
            .field("schemas_root", &self.schemas_root)
            .finish_non_exhaustive()
    }
}

impl McpCatalog {
    /// Creates an [`McpCatalog`] storing lazy tool schema JSON files under `schemas_root`.
    #[must_use]
    pub fn new(schemas_root: impl Into<PathBuf>) -> Self {
        Self {
            schemas_root: schemas_root.into(),
            inner: Arc::new(Mutex::new(CatalogInner::default())),
        }
    }

    /// Registers an MCP server's discovered schemas on disk (`<schemas_root>/<server>/<tool>.json`).
    pub fn register_server(
        &self,
        client: McpClient,
        schemas: &[ToolSchema],
    ) -> Result<(), ToolError> {
        let server_name = client.config().name.clone();
        let safe_server = sanitize_segment(&server_name);
        let server_dir = self.schemas_root.join(&safe_server);
        std::fs::create_dir_all(&server_dir).map_err(|source| ToolError::Io {
            path: server_dir.clone(),
            source,
        })?;

        let mut tool_names = Vec::with_capacity(schemas.len());
        for schema in schemas {
            let safe_tool = sanitize_segment(&schema.name);
            let schema_path = server_dir.join(format!("{safe_tool}.json"));
            let encoded = serde_json::to_string_pretty(&json!({
                "server": server_name,
                "name": schema.name,
                "description": schema.description,
                "inputSchema": schema.input_schema,
            }))?;
            std::fs::write(&schema_path, format!("{encoded}\n")).map_err(|source| {
                ToolError::Io {
                    path: schema_path,
                    source,
                }
            })?;
            tool_names.push(schema.name.clone());
        }

        if let Ok(mut guard) = self.inner.lock() {
            guard.clients.insert(server_name.clone(), client);
            guard.summaries.retain(|s| s.server_name != server_name);
            guard.summaries.push(McpServerSummary {
                server_name,
                tools: tool_names,
                schema_dir: server_dir,
            });
        }
        Ok(())
    }

    /// Returns lightweight server + tool summaries without full JSON schemas.
    #[must_use]
    pub fn summaries(&self) -> Vec<McpServerSummary> {
        self.inner
            .lock()
            .map(|g| g.summaries.clone())
            .unwrap_or_default()
    }

    /// Returns `true` when at least one MCP server is registered.
    #[must_use]
    pub fn has_servers(&self) -> bool {
        !self.summaries().is_empty()
    }

    /// Returns the root directory holding lazy MCP `<tool>.json` schema files.
    #[must_use]
    pub fn schemas_root(&self) -> &Path {
        &self.schemas_root
    }

    /// Executes a remote `tools/call` on `server_name` for `tool_name`.
    pub fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<String, ToolError> {
        let client = self
            .inner
            .lock()
            .ok()
            .and_then(|g| g.clients.get(server_name).cloned())
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "call_mcp_tool".to_owned(),
                reason: format!("unknown MCP server '{server_name}'"),
            })?;
        let req = JsonRpcRequest::new(
            client.next_request_id(),
            "tools/call",
            json!({
                "name": tool_name,
                "arguments": arguments,
            }),
        );
        let resp = client.send_request(&req)?;
        Ok(serde_json::to_string(&resp)?)
    }
}

/// Generic tool calling any lazy-loaded MCP server tool after policy evaluation (`Capability::Mcp`).
#[derive(Debug, Clone)]
pub struct CallMcpTool {
    catalog: McpCatalog,
}

impl CallMcpTool {
    /// Creates a [`CallMcpTool`] backed by `catalog`.
    #[must_use]
    pub fn new(catalog: McpCatalog) -> Self {
        Self { catalog }
    }
}

impl Tool for CallMcpTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "call_mcp_tool".to_owned(),
            description: "Call a lazy-loaded MCP tool. Read its schema JSON file in the MCP schemas directory first if needed, then pass server_name, tool_name, and arguments.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server_name": { "type": "string" },
                    "tool_name": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["server_name", "tool_name", "arguments"]
            }),
        }
    }

    fn required_capability(&self, arguments: &Value) -> Result<Capability, ToolError> {
        let (server, tool, _) = parse_call_args(arguments)?;
        Ok(Capability::Mcp { server, tool })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let (server, tool, args) = parse_call_args(arguments)?;
        self.catalog.call_tool(&server, &tool, &args)
    }
}

fn parse_call_args(arguments: &Value) -> Result<(String, String, Value), ToolError> {
    let server = arguments
        .get("server_name")
        .or_else(|| arguments.get("ServerName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "call_mcp_tool".to_owned(),
            reason: "expected non-empty string 'server_name'".to_owned(),
        })?
        .to_owned();
    let tool = arguments
        .get("tool_name")
        .or_else(|| arguments.get("ToolName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "call_mcp_tool".to_owned(),
            reason: "expected non-empty string 'tool_name'".to_owned(),
        })?
        .to_owned();
    let args = arguments
        .get("arguments")
        .or_else(|| arguments.get("Arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok((server, tool, args))
}

fn sanitize_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "item".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::McpServerConfig;

    #[test]
    fn caches_schemas_on_disk_and_dispatches_call_mcp_tool()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("rein_mcp_lazy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let client = McpClient::new(McpServerConfig {
            name: "demo".to_owned(),
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                r#"read -r _; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}'"#
                    .to_owned(),
            ],
        });
        let catalog = McpCatalog::new(&dir);
        catalog.register_server(
            client,
            &[ToolSchema {
                name: "ping".to_owned(),
                description: "ping remote".to_owned(),
                input_schema: json!({"type": "object"}),
            }],
        )?;
        assert!(dir.join("demo/ping.json").exists());

        let tool = CallMcpTool::new(catalog);
        let cap = tool.required_capability(&json!({
            "server_name": "demo",
            "tool_name": "ping",
            "arguments": {}
        }))?;
        assert_eq!(
            cap,
            Capability::Mcp {
                server: "demo".to_owned(),
                tool: "ping".to_owned()
            }
        );
        let res = tool.execute(&json!({
            "server_name": "demo",
            "tool_name": "ping",
            "arguments": {"v": 1}
        }))?;
        assert!(res.contains("\"ok\":true"));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
