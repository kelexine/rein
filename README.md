# Rein

> A policy-enforced, dual-mode coding and personal assistant CLI written in pure Rust.

## What is this?

Rein separates tool execution from model reasoning with an explicit typestate turn machine (`Idle -> Streaming -> AwaitingPolicy -> Executing`), Linux Landlock child sandboxing, and dual-scope memory/configuration layouts (`~/.rein/` for personal/global settings and `<workspace>/.rein/` for repository context). One single binary handles interactive TUI sessions, headless session replay, and re-exec sandboxed shell children without IPC daemons.

## Features

- **Compile-time Typestate Turn Loop & Providers** (`rein-core`): Forces every tool call through explicit policy evaluation (`PolicyEvaluated`) before execution, with event-sourced `session.jsonl` logging, Anthropic Messages SSE streaming, Google Antigravity (`daily-cloudcode-pa.googleapis.com`) SSE streaming with tool calling and OAuth 2.0 loopback browser login, and batched multi-tool rounds.
- **Kernel-enforced Sandbox & Bash Classifier** (`rein-policy`): Classifies `bash -c` pipelines and subcommands (`read_only` vs `mutating` vs `network`/`outside_write`, `workspace` vs `outside_workspace` vs `protected`) with scoped allowlisting (allowing `cargo *` permits workspace builds/tests without permitting `cargo install` or `cargo publish`), enforces Linux Landlock LSM + `PR_SET_NO_NEW_PRIVS` via self `--sandbox-child` re-exec with no command timeouts, caps inline `stdout`/`stderr` at 32 KiB per stream while streaming full output logs to `<workspace>/.rein/bash-outputs/`, and fails closed when kernel isolation is unavailable.
- **Coding, Personal & MCP Tools** (`rein-tools`):
  - Coding: `read`, `write`, `edit`, `glob`, `grep`, `bash` (runs `command` via `bash -c` or structured `program`/`args`).
  - Personal: `memory` (dual Project + Personal JSONL stores), `notes`, `calendar` (RFC 5545 `.ics` export + directory sync), `email` (JSONL spool + Maildir `new/` & `cur/`).
  - External: JSON-RPC 2.0 stdio `mcp` client with automatic `tools/list` discovery.
- **Minimalist Borderless TUI** (`rein`): Citadel Owl header, full-bleed scrolling transcript, inline `/slash` autocomplete menu (built-ins + `~/.rein/skills/` & `.rein/skills/` `SKILL.md` activation), non-blocking scoped-thread background streaming, 4-option permission prompt (`1. Allow`, `2. Allow in CWD`, `3. Allow Global`, `4. Deny`) across `bash` and sandboxed tools with `Tab` inline feedback attached to any highlighted option and sent back with the tool result, `Tab` mode switching (`coding` / `personal`), `--resume`, and `/compact` summarization preserving `.rein/full-transcript.jsonl`.

## Installation

```bash
cargo build --release --bin rein
```

Either authenticate directly with **Google Antigravity** (saves `~/.rein/antigravity-oauth.json` and sets `"provider": "antigravity"` in `~/.rein/config.json`) or set your Anthropic API key:

```bash
# Direct Google Antigravity OAuth browser login (also available via /login in TUI)
./target/release/rein login

# Or use Anthropic Messages API
export ANTHROPIC_API_KEY="sk-ant-..."
```

Optional environment overrides: `REIN_PROVIDER` (`anthropic` | `antigravity`), `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`, `ANTIGRAVITY_BASE_URL`, `ANTIGRAVITY_MODEL`, `ANTIGRAVITY_TOKEN`, `ANTIGRAVITY_TOKEN_PATH`, `ANTIGRAVITY_PROJECT_ID`, `REIN_HOME`.

## Usage

```bash
# Start interactive borderless TUI (auto-creates ~/.rein/ and ./.rein/)
./target/release/rein

# Override provider / model for a session
./target/release/rein --provider antigravity --model gemini-3.8-flash-medium

# Authenticate directly with Antigravity via browser OAuth 2.0
./target/release/rein login

# Resume a saved session in the current workspace directory
./target/release/rein --resume <uid>

# Deterministically replay and verify a recorded session log
./target/release/rein replay ~/.rein/sessions/<workspace-slug>/<uid>.jsonl
```

### Dual-Scope Configuration Layout

| Scope | Path | Auto-created Files & Directories |
|---|---|---|
| Global / Personal | `~/.rein/` | `config.json`, `commands.json`, `skills/`, `memory.jsonl`, `notes.jsonl`, `calendar.jsonl`, `email.jsonl`, `sessions/<workspace-slug>/<uid>.jsonl` (event log & post-compaction state), `sessions/<workspace-slug>/<uid>.full.jsonl` (uncompacted full session transcript) |
| Repository / Coding | `<workspace>/.rein/` | `config.json`, `commands.json`, `skills/`, `bash-outputs/`, `memory.jsonl`, `audit.jsonl` |

### Built-in Slash Commands & Skills

Typing `/` opens an inline autocomplete popup above the prompt (`Up`/`Down` navigate, `Tab` completes, `Enter` runs):

- Built-ins: `/help`, `/mode [coding|personal]`, `/login` (Google Antigravity OAuth browser login), `/model [provider|model]` (dynamically fetches live models and quotas from the server), `/provider [anthropic|antigravity] [model]`, `/clear`, `/compact` (summarizes active context while preserving the session's `<uid>.full.jsonl` verbatim archive), `/memory`, `/commands`, `/mcp`, `/resume`, `/quit`.
- Shortcuts: `Ctrl+O` toggles collapsed/expanded tool output previews, `Tab` toggles `coding`/`personal` mode (or edits inline approval feedback), `Up`/`Down` navigate prompt history, `PageUp`/`PageDown` scroll the transcript, `Esc` quits.
- Skills: any `<name>/SKILL.md` inside `~/.rein/skills/` (`(user)`) or `<workspace>/.rein/skills/` (`(repo)`) registers `/<name>` and instructs the model to read `SKILL.md` first before proceeding.

Add pre-approved sandboxed commands or sandboxed tool patterns as JSON arrays in `~/.rein/commands.json` or `<workspace>/.rein/commands.json` (or persist interactively via Option 2 / Option 3):

```json
["cargo *", "git status", "tool:write"]
```

Optional keys in `config.json`:

```json
{
  "provider": "antigravity",
  "model": "claude-sonnet-5-5-medium",
  "endpoint": "https://api.anthropic.com/v1/messages",
  "max_tokens": 8192,
  "api_key_env": "ANTHROPIC_API_KEY",
  "antigravity_base_url": "https://daily-cloudcode-pa.googleapis.com",
  "antigravity_token_path": "/home/user/.rein/antigravity-oauth.json",
  "antigravity_project_id": "aicode-consumers",
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
├── rein-core/     # EventLog, Typestate Turn<S>, Provider trait, Anthropic + Antigravity SSE & OAuth, ReplayProvider
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
