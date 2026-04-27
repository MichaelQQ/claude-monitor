# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & run

```
cargo build --release              # builds cm-core, cm, cm-app, cm-app-tauri
./target/release/cm-app            # headless daemon (serve UI at http://127.0.0.1:<port>/)
./target/release/cm-app-tauri      # same daemon + native window + tray
```

The daemon binds `127.0.0.1:0` and writes the chosen port to `~/.claude/claude-monitor/port`. Set `CM_PORT=NNNN` to pin a port. `cargo test -p cm-core` runs the only test suite; fixtures live at `crates/cm-core/tests/fixtures/assistant_lines.jsonl` and are real transcript lines.

Toolchain is pinned in `mise.toml` — Rust 1.88 (Tauri 2 requires ≥1.88), Node 24 (UI is static, Node is only needed for future bundler work).

## Architecture — the two ingestion paths

This is the thing to internalize before touching anything:

1. **statusline snapshot** — Claude Code invokes `cm statusline` as a `statusLine` hook, which (a) prints a short bar to stdout and (b) POSTs the raw JSON to `http://127.0.0.1:<port>/v1/event`. This payload carries `cost.*`, `context_window.*`, and `rate_limits.*` (the last only exists for Claude.ai subscribers). Stored in the `snapshots` table.
2. **transcript tail** — a `notify`-based watcher on `~/.claude/projects/**/*.jsonl` reads new bytes from each session file, parses `assistant` lines via `cm_core::transcript::parse_assistant_usage`, and inserts per-turn token rows. Offsets persist in the `tail_state` table so restarts resume cleanly. This path works regardless of whether the statusline hook is installed. Stored in the `turns` table.

Both paths converge in SQLite (`~/.claude/claude-monitor/monitor.db`, WAL mode) and fan out to the browser via a broadcast WebSocket at `/v1/live`. `LiveEvent` (`crates/cm-core/src/schema.rs`) tags each message as `snapshot` | `turn` | `subagent_snapshot`.

A third, smaller path: the `subagentStatusLine` hook invokes `cm subagent-statusline`, which forwards tasks to `/v1/subagent-event` and prints one `{"id":…,"content":…}` line per task back to stdout (the format Claude Code expects).

## Workspace layout

- `crates/cm-core/` — shared types, SQLite schema + migrations, transcript JSONL parser, pricing tables, path helpers. Depended on by both binaries.
- `crates/cm-cli/` — the `cm` binary. Subcommands: `statusline`, `subagent-statusline`, `port`, `install`, `uninstall`. `install`/`uninstall` are **macOS-only** — they write `~/Library/LaunchAgents/com.claude-monitor.daemon.plist` and merge/remove a `statusLine` block in `~/.claude/settings.json`.
- `crates/cm-app/` — daemon library (`lib.rs`, `server.rs`, `tailer.rs`, `state.rs`) plus the `cm-app` headless binary.
- `crates/cm-app/src-tauri/` — `cm-app-tauri` native-window binary. Tauri embeds the daemon via `cm_app::start` and points a webview at `http://127.0.0.1:<port>/`. Window close hides to tray; **Quit** from the tray is the only way to fully exit.
- `crates/cm-app/ui/` — `index.html` + `styles.css` + `app.js`. Plain ES modules, no build step. uPlot is loaded from a CDN. Don't add a bundler unless the user asks.

## Non-obvious invariants

- **Statusline must never blank.** `cm statusline` always prints a bar, even when JSON parsing fails — otherwise Claude Code's statusline goes empty. See `cm-cli/src/main.rs::statusline`.
- **Offline queue.** If the daemon isn't listening when `cm statusline` POSTs, the payload is appended to `~/.claude/claude-monitor/queue.jsonl`. The daemon drains and deletes this file on startup (`cm_app::drain_queue`). Don't break this round-trip.
- **Tailer stops at partial lines.** `ingest_new_bytes` only advances the offset past lines that end in `\n`. This is how we resume cleanly while Claude Code is still writing the file. Don't "fix" it to consume trailing bytes.
- **Truncation detection.** If `file_len < stored_offset`, the tailer restarts from 0 (file was rotated/truncated).
- **Quota-normalized tokens.** The `sessions` rollup and `/v1/quota-caps` compute an input-normalized token count: `input×1 + output×5 + cache_read×0.1 + cache_write_5m×1.25 + cache_write_1h×2`, with any unattributed cache-creation bytes counted at the 5m rate. This weighting is how Anthropic publicly describes rate-limit accounting; keep the formula in sync across `server.rs::list_sessions`, `server.rs::quota_caps`, and `pricing.rs::estimate_cost_usd`.
- **Pricing fallback.** `pricing::price_for` matches by substring (`opus`/`sonnet`/`haiku`) so unknown model date-suffixes still price. Unknown families return `None` and the turn records no estimate — that's intentional, don't add a generic fallback.
- **Schema migrations are versioned and append-only.** `db::migrate` walks `MIGRATIONS: &[(name, fn(&Connection))]` and applies any entry whose 1-based index exceeds `PRAGMA user_version`, stamping the version after each. Never reorder or delete an entry — that changes version numbers on already-migrated DBs and silently skips steps. The `SCHEMA` DDL still runs on every open and must stay idempotent (`CREATE TABLE IF NOT EXISTS`).
- **Project-dir decoding.** `tailer.rs::decode_project_dir` reverses Claude Code's directory-encoding scheme: `--` → literal `-`, single `-` → `/`. The two-step `\0` dance handles both without collisions.

## Runtime state on disk

All under `~/.claude/claude-monitor/`:

- `monitor.db` — SQLite (WAL). Tables: `sessions`, `turns`, `snapshots`, `tail_state`, `subagent_tasks`.
- `port` — written on daemon start, removed on clean shutdown. `cm` reads this to locate the daemon.
- `queue.jsonl` — CLI offline buffer, drained by the daemon on next start.
- `cm-app.{log,err}` — LaunchAgent stdout/stderr when installed via `cm install`.

## HTTP surface

- `GET  /v1/health` — liveness
- `GET  /v1/sessions` — rollups with totals, quota tokens, snapshot cost, estimated cost
- `GET  /v1/sessions/:id/turns` — per-turn rows (ts ASC) with priced `estimated_cost_usd`
- `GET  /v1/sessions/:id/snapshots` — statusline snapshots
- `GET  /v1/sessions/:id/subagents` — subagent tasks seen for this session
- `GET  /v1/trends?window=hour|day|week` — server-bucketed totals (SQL integer-truncation trick on `ts`)
- `GET  /v1/quota-caps` — best-effort estimate of absolute 5h/weekly caps, derived by inverting `used_percentage` from the latest snapshot over the matching turn-token window
- `POST /v1/event` — statusline snapshot ingest
- `POST /v1/subagent-event?session_id=…` — subagent tasks ingest (query fallback when stdin payload lacks `session_id`)
- `GET  /v1/live` — WebSocket, broadcasts `{kind:"snapshot"|"turn"|"subagent_snapshot", …}`
