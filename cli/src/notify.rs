use anyhow::Result;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// Whether a process exists, without spawning `kill` — a child's stderr would land on the
/// TUI's alternate screen and corrupt the frame.
pub fn pid_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// True when a live `lam watch` already owns desktop notifications here.
pub fn watch_running() -> bool {
    let Some(path) = lock_path() else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        return false;
    };
    pid != std::process::id() as i32 && pid_alive(pid)
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
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let urgency = if critical { "critical" } else { "normal" };
        Command::new("notify-send")
            .args(["-a", "lam", "-u", urgency, title, body])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
    }
    let _ = critical;
    Ok(())
}

#[cfg(target_os = "macos")]
fn osa_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_liveness_needs_no_child_process() {
        let mut child = Command::new("sleep").arg("5").spawn().expect("spawn");
        assert!(pid_alive(child.id() as i32));
        child.kill().ok();
        child.wait().ok();
        assert!(!pid_alive(child.id() as i32), "a reaped pid is not alive");
        assert!(!pid_alive(0));
        assert!(!pid_alive(-1));
    }
}
