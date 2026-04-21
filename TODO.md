# claude-monitor — next session pickup list

Ordered by value. Each item is self-contained enough to tackle in isolation.

## High value

- [ ] **Tauri window wrapper** — currently the UI is a browser tab. Wrap `cm-app` in Tauri so `cm-app` launches a native window pointed at its own localhost server. Add `crates/cm-app/src-tauri/` alongside the current pure-axum binary; the server and tailer code should move unchanged. Needs `tauri.conf.json`, icons (use a placeholder), and a tray-icon-only mode so the daemon can run in the background while the window is closed.
- [ ] **Auto-start on login** — LaunchAgent plist on macOS pointing at `cm-app`. Add a `cm install` subcommand that writes the plist, runs `launchctl load`, and flips `~/.claude/settings.json` to include the statusline hook. `cm uninstall` reverses it.
- [ ] **Cost per-turn estimate** — the transcript `usage` object doesn't include a dollar figure. Pull the current public per-token prices per model and compute `estimated_cost_usd` on each row in the `turns` table. Snapshot `total_cost_usd` stays the source of truth; per-turn estimate fills the gap between snapshots.
- [ ] **Session detail view** — currently Live shows only the active session. Add `/#/session/:id` route in `app.js` that loads the full per-turn stacked area chart + snapshot timeline overlay. Session table rows should link to it.

## Medium value

- [ ] **Trend aggregates server-side** — `loadTrends()` currently sums in the browser off session totals; good enough for now but not accurate across projects. Add `GET /v1/trends?window=day` that groups `turns.ts` in SQL.
- [ ] **Rate-limit alerting** — push-notify when `five_hour_pct` crosses 80/95. Out of scope for the daemon; emit a desktop notification via `notify-rust` when wrapped in Tauri.
- [ ] **Subagent row breakdown** — the `subagentStatusLine` feature (see statusline docs) emits a separate event shape with one row per subagent. Add `cm subagent-statusline` subcommand and a `subagent_turns` table if you want per-agent attribution.
- [ ] **Compaction events** — transcript JSONL has `type:"summary"` and compact boundary markers. Mark these on the per-turn chart so the context-window drops make sense.
- [ ] **Export / CSV** — `GET /v1/sessions/:id/turns.csv` for spreadsheet analysis.

## Low value / cleanup

- [ ] **Tests for cm-app** — the server and tailer have zero unit tests. Factor out enough of `ingest_new_bytes` and the `post_event` handler to test with `tempfile` + a `tower::ServiceExt`-driven router.
- [ ] **Tests for cm-cli** — currently smoke-tested only. `assert_cmd` + `predicates` for the stdout bar contract.
- [ ] **Tighten CORS** — `tower_http::cors::CorsLayer::permissive()` is fine for localhost-only, but tighten to `allow_origin(Any)` stripped of credentials when the Tauri wrapper lands.
- [ ] **Config file** — `~/.claude/claude-monitor/config.toml` for port pinning, retention policy, and tailer include/exclude globs. Right now everything is env/path conventions.
- [ ] **Retention** — nothing ever deletes rows. Add `retention_days` config and a daily vacuum.
- [ ] **Windows support** — `paths.rs` hard-codes POSIX conventions; `~/.claude/projects/<encoded-path>` decoding uses `/` which is wrong on Windows. Not urgent unless you actually use Windows.
- [ ] **Upgrade-safe schema migrations** — current DDL uses `CREATE TABLE IF NOT EXISTS` only. Bring in `refinery` or hand-roll a `schema_version` table before the first breaking change.

## Known quirks

- The `decode_project_dir` function in `crates/cm-app/src/tailer.rs` is a heuristic: Claude Code encodes `/` → `-` and `-` → `--`. Edge cases (e.g. paths with embedded `--`) may decode wrong, but it's only cosmetic (used for display, not joining).
- The app watches `~/.claude/projects` recursively. If you have a very large backfill (thousands of sessions), the initial ingest on first launch can take a few seconds. Subsequent launches resume from `tail_state.offset` per file, so it's fast.
- `Cargo.lock` is gitignored (see `.gitignore`); this is a workspace of binaries, but you may want to commit it for reproducible builds — toggle as you prefer.

## Open decisions

- **Single daemon vs per-user**: today it's per-user, listening on localhost. If you ever want a shared dashboard across a team, swap SQLite for Postgres and bind to a non-loopback interface behind auth.
- **Historical import depth**: the tailer ingests every pre-existing JSONL on first run. That's convenient but could be surprising for very old transcripts. Consider a `--since` flag.
