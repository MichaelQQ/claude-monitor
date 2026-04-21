# claude-monitor

Real-time Claude Code token tracing: a CLI that plugs into Claude Code's
`statusLine` hook, plus a local daemon with a browser dashboard that also
tails every `~/.claude/projects/*.jsonl` transcript so you can see per-turn
token usage, cost, and rate-limit state across all sessions.

## Architecture

```
Claude Code session
  └── statusLine ──► cm statusline  (cm-cli)
                       ├─► prints short bar to stdout
                       └─► POST http://127.0.0.1:<port>/v1/event
                                                   │
cm-app (daemon)                                    ▼
  ├── axum HTTP/WS server (127.0.0.1, random port written to port file)
  ├── transcript tailer — notify-based watcher on ~/.claude/projects
  ├── SQLite (~/.claude/claude-monitor/monitor.db) — sessions, turns, snapshots
  └── static web UI served at /
```

Two ingestion paths by design:
- **statusline snapshot** — carries `cost.*`, `context_window.*`, `rate_limits.*` (present only for Claude.ai subscribers)
- **transcript tail** — carries per-turn `input/output/cache_creation/cache_read/ephemeral_{1h,5m}` tokens, works for every session regardless of statusline install

## Layout

```
crates/
  cm-core/   # shared types (schema), SQLite setup, transcript JSONL parser
  cm-cli/    # the `cm` binary — statusline subcommand
  cm-app/    # the daemon — axum server, transcript tailer, static UI
    ui/      # index.html / styles.css / app.js (uPlot, ES modules, no build)
```

## Toolchain

Pinned via mise (`mise.toml` at repo root):
- Rust 1.86
- Node 24 (only needed if you swap the UI for a bundled framework later)

```
mise install
```

## Build & run

```
cargo build --release                     # builds cm-core, cm, cm-app
./target/release/cm-app                   # starts the daemon
# Browser → http://127.0.0.1:$(cat ~/.claude/claude-monitor/port)/
```

Wire the CLI into Claude Code (`~/.claude/settings.json`):

```json
{
  "statusLine": {
    "type": "command",
    "command": "/absolute/path/to/claude-monitor/target/release/cm statusline"
  }
}
```

Env override: set `CM_PORT` to force a specific port (the app binds 0 by default and writes the bound port to `~/.claude/claude-monitor/port`).

## Testing

```
cargo test -p cm-core
```

Fixtures at `crates/cm-core/tests/fixtures/assistant_lines.jsonl` are real
transcript lines copied from an existing project — parser and DB behavior
are validated against actual Claude Code output.

## HTTP endpoints

- `GET  /v1/health` — liveness
- `GET  /v1/sessions` — session rollups with total tokens and latest cost
- `GET  /v1/sessions/:id/turns` — per-turn rows (ordered by ts)
- `GET  /v1/sessions/:id/snapshots` — statusline snapshots for this session
- `POST /v1/event` — statusline snapshot ingest (used by `cm statusline`)
- `GET  /v1/live` (WebSocket) — broadcasts `{kind:"snapshot"|"turn", …}` events

## Data on disk

- `~/.claude/claude-monitor/monitor.db` — SQLite (WAL)
- `~/.claude/claude-monitor/port` — current bound port (written on start, removed on clean exit)
- `~/.claude/claude-monitor/queue.jsonl` — CLI offline buffer; drained when the daemon next starts
