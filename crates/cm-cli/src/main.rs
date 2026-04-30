use anyhow::Result;
use clap::{Parser, Subcommand};
use cm_core::paths;
use cm_core::schema::{StatuslineInput, SubagentStatuslineInput};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

mod install;

#[derive(Parser)]
#[command(name = "cm", about = "claude-monitor CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run as a Claude Code statusline: read JSON on stdin, forward to the app, print a short bar.
    Statusline,
    /// Run as a Claude Code subagentStatusLine: read JSON on stdin, forward to the app, print one
    /// `{"id":…,"content":…}` JSON line per task.
    SubagentStatusline {
        /// Parent session id. Claude Code may or may not pass it on stdin; this lets the user pin
        /// it via the hook `command` string.
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Print the resolved app port (or "none" if not running).
    Port,
    /// Install the LaunchAgent (auto-start on login) and wire the statusline hook into ~/.claude/settings.json. macOS only.
    Install {
        /// Path to the cm-app daemon binary. Defaults to a sibling `cm-app` next to the current executable.
        #[arg(long)]
        binary: Option<PathBuf>,
    },
    /// Reverse `cm install`: unload + remove the LaunchAgent plist and drop our statusLine entry. macOS only.
    Uninstall,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Statusline => statusline(),
        Cmd::SubagentStatusline { session_id } => subagent_statusline(session_id.as_deref()),
        Cmd::Port => {
            match read_port() {
                Some(p) => println!("{p}"),
                None => println!("none"),
            }
            Ok(())
        }
        Cmd::Install { binary } => install::install(binary),
        Cmd::Uninstall => install::uninstall(),
    }
}

fn statusline() -> Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    // Always print the bar — never let a parse failure blank the statusline.
    let parsed: Option<StatuslineInput> = serde_json::from_str(&raw).ok();
    print_bar(parsed.as_ref());
    // Fire-and-forget forward; failures fall back to disk queue.
    if let Err(_) = forward(&raw) {
        let _ = enqueue(&raw);
    }
    Ok(())
}

fn subagent_statusline(cli_session_id: Option<&str>) -> Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let parsed: Option<SubagentStatuslineInput> = serde_json::from_str(&raw).ok();
    print_subagent_rows(parsed.as_ref());
    let session_id = cli_session_id
        .map(str::to_string)
        .or_else(|| parsed.as_ref().and_then(|s| s.session_id.clone()));
    if forward_subagent(&raw, session_id.as_deref()).is_err() {
        let _ = enqueue(&raw);
    }
    Ok(())
}

fn print_subagent_rows(s: Option<&SubagentStatuslineInput>) {
    let Some(s) = s else { return };
    // Claude Code reads one `{"id":…,"content":…}` JSON object per line from stdout.
    for t in &s.tasks {
        let label = t
            .label
            .clone()
            .or_else(|| t.name.clone())
            .unwrap_or_else(|| t.id.clone());
        let mut parts = vec![label];
        if let Some(status) = &t.status {
            parts.push(status.clone());
        }
        if let Some(tok) = t.token_count {
            parts.push(format_tokens(tok));
        }
        let row = serde_json::json!({ "id": t.id, "content": parts.join(" · ") });
        println!("{row}");
    }
}

fn format_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tok", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.1}k tok", n as f64 / 1000.0)
    } else {
        format!("{n} tok")
    }
}

fn print_bar(s: Option<&StatuslineInput>) {
    let Some(s) = s else {
        println!("[claude-monitor]");
        return;
    };
    let model = &s.model.display_name;
    let cw = s.context_window.as_ref();
    let pct = cw.and_then(|c| c.used_percentage).unwrap_or(0.0) as i64;
    let session_tokens = cw
        .map(|c| c.total_input_tokens.unwrap_or(0) + c.total_output_tokens.unwrap_or(0))
        .unwrap_or(0);
    let cache_hit = cw.and_then(|c| c.current_usage.as_ref()).and_then(|u| {
        let read = u.cache_read_input_tokens.unwrap_or(0);
        let create = u.cache_creation_input_tokens.unwrap_or(0);
        let input = u.input_tokens.unwrap_or(0);
        let total = read + create + input;
        (total > 0).then(|| (read as f64 / total as f64 * 100.0) as i64)
    });
    let cost = s.cost.as_ref().and_then(|c| c.total_cost_usd).unwrap_or(0.0);
    let mut head = format!("[{model}] {pct}%");
    if session_tokens > 0 {
        head.push_str(&format!(" ({})", format_tokens(session_tokens)));
    }
    head.push_str(&format!(" · ${cost:.2}"));
    if let Some(hit) = cache_hit {
        head.push_str(&format!(" · cache:{hit}%"));
    }
    let mut parts = vec![head];
    if let Some(rl) = &s.rate_limits {
        let mut sub = Vec::new();
        if let Some(f) = &rl.five_hour {
            sub.push(format!(
                "5h:{:.0}%→{}",
                f.used_percentage,
                fmt_resets(f.resets_at)
            ));
        }
        if let Some(w) = &rl.seven_day {
            sub.push(format!(
                "7d:{:.0}%→{}",
                w.used_percentage,
                fmt_resets(w.resets_at)
            ));
        }
        if !sub.is_empty() {
            parts.push(sub.join(" "));
        }
    }
    println!("{}", parts.join(" · "));
}

fn fmt_resets(epoch: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let dt = epoch - now;
    if dt <= 0 {
        return "now".into();
    }
    let mins = dt / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hrs = mins / 60;
    if hrs < 48 {
        return format!("{}h{}m", hrs, mins % 60);
    }
    format!("{}d", hrs / 24)
}

fn read_port() -> Option<u16> {
    if let Ok(v) = std::env::var("CM_PORT") {
        if let Ok(p) = v.parse() {
            return Some(p);
        }
    }
    let mut s = String::new();
    File::open(paths::port_file()).ok()?.read_to_string(&mut s).ok()?;
    s.trim().parse().ok()
}

fn forward(raw: &str) -> Result<()> {
    let port = read_port().ok_or_else(|| anyhow::anyhow!("no port"))?;
    let url = format!("http://127.0.0.1:{port}/v1/event");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(200))
        .build();
    agent
        .post(&url)
        .set("content-type", "application/json")
        .send_string(raw)
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

fn forward_subagent(raw: &str, session_id: Option<&str>) -> Result<()> {
    let port = read_port().ok_or_else(|| anyhow::anyhow!("no port"))?;
    let mut url = format!("http://127.0.0.1:{port}/v1/subagent-event");
    if let Some(sid) = session_id {
        url.push_str("?session_id=");
        url.push_str(&urlencode(sid));
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(200))
        .build();
    agent
        .post(&url)
        .set("content-type", "application/json")
        .send_string(raw)
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn enqueue(raw: &str) -> Result<()> {
    let path = paths::queue_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    // One JSON object per line; strip embedded newlines to keep the file a valid JSONL.
    let compacted: serde_json::Value = serde_json::from_str(raw)?;
    writeln!(f, "{}", serde_json::to_string(&compacted)?)?;
    Ok(())
}
