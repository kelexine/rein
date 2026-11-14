# Rein

> A policy-enforced, dual-mode coding and personal assistant CLI written in pure Rust.

## What is this?

Rein separates tool execution from model reasoning with an explicit typestate turn machine (`Idle -> Streaming -> AwaitingPolicy -> Executing`), Linux Landlock child sandboxing, and dual-scope memory/configuration layouts (`~/.rein/` for personal/global settings and `<workspace>/.rein/` for repository context). One single binary handles interactive TUI sessions, headless session replay, and re-exec sandboxed shell children without IPC daemons.

## Features

- **Compile-time Typestate Turn Loop** (`rein-core`): Forces every tool call through explicit policy evaluation (`PolicyEvaluated`) before execution, with event-sourced `session.jsonl` logging, Anthropic SSE streaming with full HTTP error surfacing, and batched multi-tool rounds.
- **Kernel-enforced Sandbox & Shell Classifier** (`rein-policy`): Classifies shell pipelines and scripts (`read_only` vs `mutating`, `workspace` vs `external`), enforces Linux Landlock LSM + `PR_SET_NO_NEW_PRIVS` via self `--sandbox-child` re-exec, and fails closed when kernel isolation is unavailable.
- **Coding, Personal & MCP Tools** (`rein-tools`):
  - Coding: `read`, `write`, `edit`, `glob`, `grep`, `shell` (supports structured `program`/`args` or `command` pipelines).
  - Personal: `memory` (dual Project + Personal JSONL stores), `notes`, `calendar` (RFC 5545 `.ics` export + directory sync), `email` (JSONL spool + Maildir `new/` & `cur/`).
  - External: JSON-RPC 2.0 stdio `mcp` client with automatic `tools/list` discovery.
- **Minimalist Borderless TUI** (`rein`): Full-bleed scrolling transcript, non-blocking scoped-thread background streaming, inline `write`/`edit` `[y/n]` confirmation previews, `Tab` mode switching (`coding` / `personal`), and `--resume`.

## Installation

```bash
cargo build --release --bin rein
```

Set your Anthropic API key in the environment before starting an interactive session:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

Optional environment overrides: `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`, `REIN_HOME`.

## Usage

```bash
# Start interactive borderless TUI (auto-creates ~/.rein/ and ./.rein/)
./target/release/rein

# Resume the previous workspace session from .rein/session.jsonl
./target/release/rein --resume

# Deterministically replay and verify a recorded session log
./target/release/rein replay .rein/session.jsonl
```

### Dual-Scope Configuration Layout

| Scope | Path | Auto-created Files |
|---|---|---|
| Global / Personal | `~/.rein/` | `config.json`, `commands.json`, `memory.jsonl`, `notes.jsonl`, `calendar.jsonl`, `email.jsonl` |
| Repository / Coding | `<workspace>/.rein/` | `config.json`, `commands.json`, `memory.jsonl`, `session.jsonl`, `audit.jsonl` |

Add pre-approved sandboxed shell commands as JSON arrays in `~/.rein/commands.json` or `<workspace>/.rein/commands.json`:

```json
["cargo", "git", "rg"]
```

Optional keys in `config.json`:

```json
{
  "model": "claude-sonnet-4-20250514",
  "endpoint": "https://api.anthropic.com/v1/messages",
  "max_tokens": 8192,
  "api_key_env": "ANTHROPIC_API_KEY",
  "ics_dir": "/path/to/calendars",
  "maildir": "/path/to/Maildir",
  "mcp_servers": [
    { "name": "example", "command": "npx", "args": ["-y", "@example/mcp"] }
  ]
}
```

## Architecture

```
crates/
├── rein-core/     # EventLog, Typestate Turn<S>, Provider trait, Anthropic SSE + ReplayProvider
├── rein-policy/   # Shell AST/pipeline classifier, PolicyEngine, Landlock child sandbox
├── rein-tools/    # Coding tools, Dual-scope JSONL memory, Personal (.ics/Maildir), MCP stdio
└── rein/          # Dual-scope config loader, AgentLoop, ModeManager, borderless Ratatui TUI
```

## Contributing

- Branch naming: `<type>/<short-slug>` off `dev` (hotfixes off `main`).
- Follow Conventional Commits (`feat(scope): summary`) with `git commit -s`.
- Before pushing: run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --workspace`.

## License

MIT OR Apache-2.0
