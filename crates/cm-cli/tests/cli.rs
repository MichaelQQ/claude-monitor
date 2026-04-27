use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// Run the `cm` binary under a sandboxed HOME so the queue fallback writes land
/// in a tempdir instead of the real user directory.
fn cmd() -> (Command, TempDir) {
    let home = tempfile::tempdir().unwrap();
    let mut c = Command::cargo_bin("cm").unwrap();
    c.env("HOME", home.path())
        .env_remove("CM_PORT")
        .env_remove("XDG_CONFIG_HOME");
    (c, home)
}

const FULL_PAYLOAD: &str = r#"{
    "session_id": "S1",
    "model": { "id": "claude-opus-4-7", "display_name": "Opus" },
    "cost": { "total_cost_usd": 1.23 },
    "context_window": { "used_percentage": 47.0 },
    "rate_limits": {
        "five_hour":  { "used_percentage": 33.0, "resets_at": 99999999999 },
        "seven_day": { "used_percentage": 12.0, "resets_at": 99999999999 }
    }
}"#;

#[test]
fn statusline_prints_bar_for_valid_input() {
    let (mut cmd, _home) = cmd();
    cmd.arg("statusline")
        .write_stdin(FULL_PAYLOAD)
        .assert()
        .success()
        .stdout(predicate::str::contains("[Opus]"))
        .stdout(predicate::str::contains("47%"))
        .stdout(predicate::str::contains("$1.23"))
        .stdout(predicate::str::contains("5h:33%"))
        .stdout(predicate::str::contains("7d:12%"));
}

#[test]
fn statusline_never_blanks_on_garbage() {
    let (mut cmd, _home) = cmd();
    cmd.arg("statusline")
        .write_stdin("not json at all")
        .assert()
        .success()
        // Must print SOMETHING — Claude Code's statusline must not go empty.
        .stdout(predicate::str::is_empty().not())
        .stdout(predicate::str::contains("[claude-monitor]"));
}

#[test]
fn statusline_never_blanks_on_empty_stdin() {
    let (mut cmd, _home) = cmd();
    cmd.arg("statusline")
        .write_stdin("")
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

#[test]
fn subagent_statusline_emits_one_row_per_task() {
    let (mut cmd, _home) = cmd();
    let payload = r#"{
        "session_id": "SUB",
        "tasks": [
            {"id": "a1", "name": "plan", "status": "running", "tokenCount": 1200},
            {"id": "a2", "label": "build", "status": "done"}
        ]
    }"#;
    let out = cmd
        .arg("subagent-statusline")
        .write_stdin(payload)
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "got {stdout:?}");
    // Each line must be a JSON object with id + content.
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect(line);
        assert!(v.get("id").is_some());
        assert!(v.get("content").is_some());
    }
    let v0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v0["id"], "a1");
    let c0 = v0["content"].as_str().unwrap();
    assert!(c0.contains("plan"));
    assert!(c0.contains("running"));
    assert!(c0.contains("1.2k tok"));

    let v1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(v1["id"], "a2");
    // label takes priority over id for the leading slot.
    assert!(v1["content"].as_str().unwrap().contains("build"));
}

#[test]
fn subagent_statusline_tolerates_garbage() {
    let (mut cmd, _home) = cmd();
    cmd.arg("subagent-statusline")
        .write_stdin("}}}not json{{{")
        .assert()
        .success();
}

#[test]
fn port_reports_none_when_daemon_not_running() {
    let (mut cmd, _home) = cmd();
    cmd.arg("port")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("none"));
}

#[test]
fn port_respects_env_override() {
    let (mut cmd, _home) = cmd();
    cmd.env("CM_PORT", "37000")
        .arg("port")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("37000"));
}

#[test]
fn statusline_queues_payload_when_no_daemon() {
    let (mut cmd, home) = cmd();
    cmd.arg("statusline")
        .write_stdin(FULL_PAYLOAD)
        .assert()
        .success();
    let queue = home.path().join(".claude/claude-monitor/queue.jsonl");
    let body = std::fs::read_to_string(&queue)
        .unwrap_or_else(|e| panic!("expected queue at {}: {e}", queue.display()));
    let v: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(v["session_id"], "S1");
}
