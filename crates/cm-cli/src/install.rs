use anyhow::{anyhow, Context, Result};
use cm_core::paths;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LAUNCHD_LABEL: &str = "com.claude-monitor.daemon";

pub fn install(binary: Option<PathBuf>) -> Result<()> {
    require_macos()?;
    let cm_app = resolve_cm_app(binary)?;
    let cm_bin = std::env::current_exe()?;

    write_plist(&cm_app)?;
    load_plist()?;
    let statusline = update_settings(&cm_bin)?;

    println!("installed:");
    println!("  daemon:      {}", cm_app.display());
    println!("  plist:       {}", plist_path().display());
    println!("  settings:    {}", settings_path().display());
    println!("  statusLine:  {statusline}");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    require_macos()?;
    let cm_bin = std::env::current_exe().ok();
    unload_plist();
    let plist_removed = remove_plist()?;
    let settings_touched = revert_settings(cm_bin.as_deref())?;

    println!("uninstalled:");
    println!(
        "  plist:       {} {}",
        plist_path().display(),
        if plist_removed { "(removed)" } else { "(not present)" }
    );
    println!(
        "  settings:    {} {}",
        settings_path().display(),
        if settings_touched {
            "(statusLine removed)"
        } else {
            "(unchanged)"
        }
    );
    Ok(())
}

fn require_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(anyhow!("cm install/uninstall is only supported on macOS"))
    }
}

fn resolve_cm_app(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if !p.exists() {
            return Err(anyhow!("{} does not exist", p.display()));
        }
        return Ok(fs::canonicalize(&p)?);
    }
    let me = std::env::current_exe()?;
    let dir = me
        .parent()
        .ok_or_else(|| anyhow!("current exe has no parent dir"))?;
    let candidate = dir.join("cm-app");
    if candidate.exists() {
        return Ok(fs::canonicalize(&candidate)?);
    }
    Err(anyhow!(
        "couldn't find cm-app next to {}; pass --binary /path/to/cm-app",
        me.display()
    ))
}

fn plist_path() -> PathBuf {
    paths::home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

fn settings_path() -> PathBuf {
    paths::home().join(".claude").join("settings.json")
}

fn write_plist(cm_app: &Path) -> Result<()> {
    let data_dir = paths::app_data_dir();
    fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let log = data_dir.join("cm-app.log");
    let err = data_dir.join("cm-app.err");

    let path = plist_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{cm_app}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        cm_app = xml_escape(&cm_app.display().to_string()),
        log = xml_escape(&log.display().to_string()),
        err = xml_escape(&err.display().to_string()),
    );
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn load_plist() -> Result<()> {
    let path = plist_path();
    let path_str = path.to_string_lossy().to_string();
    // unload first so re-runs pick up a changed plist body.
    let _ = Command::new("launchctl")
        .args(["unload", &path_str])
        .output();
    let out = Command::new("launchctl")
        .args(["load", "-w", &path_str])
        .output()
        .context("running launchctl load")?;
    if !out.status.success() {
        return Err(anyhow!(
            "launchctl load failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn unload_plist() {
    let path = plist_path();
    if !path.exists() {
        return;
    }
    let _ = Command::new("launchctl")
        .args(["unload", "-w", &path.to_string_lossy()])
        .output();
}

fn remove_plist() -> Result<bool> {
    let path = plist_path();
    if path.exists() {
        fs::remove_file(&path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn update_settings(cm_bin: &Path) -> Result<String> {
    let path = settings_path();
    let mut root: serde_json::Value = if path.exists() {
        let raw = fs::read_to_string(&path)?;
        if raw.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?
        }
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        serde_json::json!({})
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("{} is not a JSON object", path.display()))?;
    let command = format!("{} statusline", cm_bin.display());
    obj.insert(
        "statusLine".into(),
        serde_json::json!({
            "type": "command",
            "command": command,
        }),
    );
    let rendered = serde_json::to_string_pretty(&root)? + "\n";
    fs::write(&path, rendered)?;
    Ok(command)
}

/// Remove the `statusLine` entry if it points at our `cm` binary.
/// Returns true if the file was modified.
fn revert_settings(cm_bin: Option<&Path>) -> Result<bool> {
    let path = settings_path();
    if !path.exists() {
        return Ok(false);
    }
    let raw = fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let mut root: serde_json::Value = serde_json::from_str(&raw)?;
    let Some(obj) = root.as_object_mut() else {
        return Ok(false);
    };
    let current_cmd = obj
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let ours = cm_bin
        .map(|p| current_cmd == format!("{} statusline", p.display()))
        .unwrap_or(false)
        || current_cmd.ends_with("/cm statusline");
    if !ours {
        return Ok(false);
    }
    obj.remove("statusLine");
    let rendered = serde_json::to_string_pretty(&root)? + "\n";
    fs::write(&path, rendered)?;
    Ok(true)
}
