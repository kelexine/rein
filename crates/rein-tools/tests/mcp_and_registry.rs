// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-18
// Purpose: Integration tests for ToolRegistry policy gating and McpClient/McpToolAdapter stdio round-trips

//! Integration tests for `rein-tools` policy-gated registry and MCP stdio round-trips.

use std::path::PathBuf;

use rein_core::{EventLog, ToolCall, Turn};
use rein_policy::{AuditLog, PolicyDecision, PolicyEngine, PolicyRuleSet, WorkspaceSandbox};
use rein_tools::{
    McpClient, McpServerConfig, McpToolAdapter, ReadTool, Tool, ToolRegistry, WriteTool,
};
use serde_json::json;

fn fresh_temp_dir(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("rein_tools_it_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn registry_enforces_profile_and_sandbox_policy() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("registry_policy")?;
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace)?;
    std::fs::write(workspace.join("notes.txt"), "hello rein")?;

    let sandbox = WorkspaceSandbox::new(&workspace);
    let policy = PolicyEngine::new(
        PolicyRuleSet::coding_default(),
        sandbox.clone(),
        AuditLog::new(dir.join("audit.jsonl")),
    );

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(sandbox.clone())));
    registry.register(Box::new(WriteTool::new(sandbox)));

    let log = EventLog::new(dir.join("session.jsonl"));

    // 1. Read inside workspace is allowed and executes cleanly.
    let awaiting = Turn::new(&log)
        .begin_user_turn("read notes")?
        .request_tool(ToolCall {
            id: "c1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({ "path": "notes.txt" }),
        })?;
    let decision = registry.evaluate_turn_for_profile(&awaiting, &policy, &["read".to_owned()])?;
    assert!(matches!(decision, PolicyDecision::Allow { .. }));
    let executing = awaiting.approve("policy allow")?;
    let (_idle, output) = registry.execute_approved(executing)?;
    assert!(!output.is_error);
    assert!(output.content.contains("hello rein"));

    // 2. Write outside workspace is denied by sandbox policy.
    let outside = dir.join("outside.txt");
    let awaiting_write = Turn::new(&log)
        .begin_user_turn("write outside")?
        .request_tool(ToolCall {
            id: "c2".to_owned(),
            name: "write".to_owned(),
            arguments: json!({ "path": outside, "content": "nope" }),
        })?;
    let write_decision = registry.evaluate_turn_for_profile(
        &awaiting_write,
        &policy,
        &["read".to_owned(), "write".to_owned()],
    )?;
    assert!(matches!(write_decision, PolicyDecision::Deny { .. }));

    // 3. Tool omitted from active profile's allowed_tools is denied.
    let profile_decision =
        registry.evaluate_turn_for_profile(&awaiting_write, &policy, &["read".to_owned()])?;
    assert!(matches!(profile_decision, PolicyDecision::Deny { .. }));

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn mcp_client_and_adapter_round_trip_via_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_temp_dir("mcp_stdio")?;
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace)?;

    // Mock stdio MCP server responding to tools/list and tools/call.
    let server_script = r#"
read -r req
case "$req" in
  *'"method":"tools/list"'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"echo_upper","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}'
    ;;
  *'"method":"tools/call"'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"MCP_OK"}]}}'
    ;;
  *)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown method"}}'
    ;;
esac
"#;

    let client = McpClient::new(McpServerConfig {
        name: "mock_mcp".to_owned(),
        command: "sh".to_owned(),
        args: vec!["-c".to_owned(), server_script.to_owned()],
    });

    let schemas = client.list_tools()?;
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "echo_upper");

    let mut registry = ToolRegistry::new();
    let adapter = McpToolAdapter::new(client.clone(), schemas[0].clone());
    let registered_name = adapter.schema().name;
    assert_eq!(registered_name, "echo_upper");
    registry.register(Box::new(adapter));

    let policy = PolicyEngine::new(
        PolicyRuleSet::coding_default(),
        WorkspaceSandbox::new(&workspace),
        AuditLog::new(dir.join("audit.jsonl")),
    );

    let log = EventLog::new(dir.join("mcp_session.jsonl"));
    let awaiting = Turn::new(&log)
        .begin_user_turn("call mcp")?
        .request_tool(ToolCall {
            id: "c_mcp".to_owned(),
            name: registered_name.clone(),
            arguments: json!({ "text": "hi" }),
        })?;
    let decision = registry.evaluate_turn_for_profile(&awaiting, &policy, &[registered_name])?;
    assert!(matches!(decision, PolicyDecision::Ask { .. }));

    let executing = awaiting.approve("mcp allowed")?;
    let (_idle, output) = registry.execute_approved(executing)?;
    assert!(!output.is_error);
    assert!(output.content.contains("MCP_OK"));

    // Verify collision protection: built-in `read` cannot be overwritten via try_register,
    // and a colliding MCP tool can be registered under `mcp__<server>__<tool>` while still
    // invoking the remote tool name on the MCP server.
    registry.register(Box::new(ReadTool::new(WorkspaceSandbox::new(&workspace))));
    assert!(registry.contains("read"));
    let colliding_schema = rein_tools::ToolSchema {
        name: "read".to_owned(),
        description: "MCP read tool colliding with built-in read".to_owned(),
        input_schema: json!({"type": "object"}),
    };
    assert!(!registry.try_register(Box::new(McpToolAdapter::new(
        client.clone(),
        colliding_schema.clone(),
    ))));
    let namespaced = McpToolAdapter::namespaced_name("mock_mcp", &colliding_schema.name);
    assert_eq!(namespaced, "mcp__mock_mcp__read");
    let ns_adapter =
        McpToolAdapter::with_registered_name(client, colliding_schema, namespaced.clone());
    assert!(registry.try_register(Box::new(ns_adapter)));

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
