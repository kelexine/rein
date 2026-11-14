// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Model Context Protocol (MCP) stdio client and policy-gated tool adapter

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rein_policy::Capability;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::registry::{Tool, ToolError, ToolSchema};

/// Configuration for a stdio-attached MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Logical server name.
    pub name: String,
    /// Executable command for spawning the MCP server.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
}

/// Lightweight stdio MCP client executing single-turn JSON-RPC 2.0 exchanges.
#[derive(Debug, Clone)]
pub struct McpClient {
    config: McpServerConfig,
}

impl McpClient {
    /// Creates a new [`McpClient`] from `config`.
    #[must_use]
    pub fn new(config: McpServerConfig) -> Self {
        Self { config }
    }

    /// Returns the server configuration.
    #[must_use]
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Queries `tools/list` on the configured MCP server and adapts each schema.
    pub fn list_tools(&self) -> Result<Vec<ToolSchema>, ToolError> {
        let req = JsonRpcRequest::new(1, "tools/list", json!({}));
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

    /// Sends a single JSON-RPC 2.0 request over stdio to the configured MCP server binary.
    pub fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse, ToolError> {
        let mut child = Command::new(&self.config.command)
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| ToolError::Io {
                path: PathBuf::from(&self.config.command),
                source,
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            let line = serde_json::to_string(request)?;
            writeln!(stdin, "{line}").map_err(|source| ToolError::Io {
                path: PathBuf::from(&self.config.command),
                source,
            })?;
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: self.config.name.clone(),
                reason: "failed to capture MCP server stdout".to_owned(),
            })?;
        let mut response_line = String::new();
        let _ = BufReader::new(stdout)
            .read_line(&mut response_line)
            .map_err(|source| ToolError::Io {
                path: PathBuf::from(&self.config.command),
                source,
            })?;
        let _ = child.wait();
        Ok(serde_json::from_str(&response_line)?)
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
    schema: ToolSchema,
}

impl McpToolAdapter {
    /// Wraps a discovered MCP tool schema under `client`.
    #[must_use]
    pub fn new(client: McpClient, schema: ToolSchema) -> Self {
        Self { client, schema }
    }
}

impl Tool for McpToolAdapter {
    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    fn required_capability(&self, _arguments: &Value) -> Result<Capability, ToolError> {
        Ok(Capability::Mcp {
            server: self.client.config().name.clone(),
            tool: self.schema.name.clone(),
        })
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let req = JsonRpcRequest::new(
            1,
            "tools/call",
            json!({
                "name": self.schema.name,
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
}
