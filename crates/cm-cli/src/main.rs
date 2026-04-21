use anyhow::Result;
use clap::{Parser, Subcommand};
use cm_core::paths;
use cm_core::schema::StatuslineInput;
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

fn print_bar(s: Option<&StatuslineInput>) {
    let Some(s) = s else {
        println!("[claude-monitor]");
        return;
    };
    let model = &s.model.display_name;
    let pct = s
        .context_window
        .as_ref()
        .and_then(|c| c.used_percentage)
        .unwrap_or(0.0) as i64;
    let cost = s.cost.as_ref().and_then(|c| c.total_cost_usd).unwrap_or(0.0);
    let mut parts = vec![format!("[{model}] {pct}% · ${cost:.2}")];
    if let Some(rl) = &s.rate_limits {
        let mut sub = Vec::new();
        if let Some(f) = &rl.five_hour {
            sub.push(format!("5h:{:.0}%", f.used_percentage));
        }
        if let Some(w) = &rl.seven_day {
            sub.push(format!("7d:{:.0}%", w.used_percentage));
        }
        if !sub.is_empty() {
            parts.push(sub.join(" "));
        }
    }
    println!("{}", parts.join(" · "));
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
