# Rein

> A terminal coding and personal assistant.

## What is this?

Rein is an interactive terminal AI assistant for writing code, planning changes, and managing everyday personal tasks from one place. It asks before running sensitive actions, keeps your work organized by session, and remembers context across projects and personal workflows.

## What it does

- **Coding mode**: Read, search, and edit files, run shell commands, and track progress with a live checklist.
- **Plan mode**: Explore a codebase and design changes in read-only mode before touching any files.
- **Personal mode**: Manage notes, calendar events, emails, and long-term personal memory.
- **Sessions & Memory**: Resume earlier conversations anytime, compact long histories, and automatically consolidate memories in the background.
- **Custom Skills & Tools**: Add custom `/commands` via Markdown skills or connect external MCP tools.

## Getting Started

Build and run Rein:

```bash
cargo build --release --bin rein
./target/release/rein
```

Sign in with Google Antigravity (`./target/release/rein login`) or set `ANTHROPIC_API_KEY`.

## Basic Controls

- `Shift+Tab` — Switch between `coding`, `plan`, and `personal` modes
- `/` — Open slash commands and custom skills (`/help`, `/model`, `/resume`, `/compact`, `/clear`, `/exit`)
- `Ctrl+O` — Expand or collapse tool details
- `Esc` — Stop the current response
- `Ctrl+C` (twice) — Exit

## License

MIT OR Apache-2.0
