use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

fn lock_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("lam").join("watch.pid"))
}

/// Removes the watch lock when `lam watch` exits.
pub struct WatchLock(PathBuf);

impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Marks this process as the desktop notifier, so a TUI on the same machine stays quiet.
pub fn claim_watch() -> Option<WatchLock> {
    let path = lock_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, std::process::id().to_string()).ok()?;
    Some(WatchLock(path))
}

/// True when a live `lam watch` already owns desktop notifications here.
pub fn watch_running() -> bool {
    let Some(path) = lock_path() else {
        return false;
    };
    let Ok(pid) = std::fs::read_to_string(&path) else {
        return false;
    };
    let pid = pid.trim();
    if pid.is_empty() || pid == std::process::id().to_string() {
        return false;
    }
    Command::new("kill")
        .args(["-0", pid])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Desktop notification: notify-send on Linux, osascript on macOS.
pub fn desktop(title: &str, body: &str, critical: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title {} subtitle \"lam\"",
            osa_quote(body),
            osa_quote(title)
        );
        Command::new("osascript").arg("-e").arg(script).status()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let urgency = if critical { "critical" } else { "normal" };
        Command::new("notify-send")
            .args(["-a", "lam", "-u", urgency, title, body])
            .status()?;
    }
    let _ = critical;
    Ok(())
}

#[cfg(target_os = "macos")]
fn osa_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
