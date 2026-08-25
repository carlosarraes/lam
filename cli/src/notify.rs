use anyhow::Result;
use std::process::Command;

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
        Command::new("notify-send").args(["-a", "lam", "-u", urgency, title, body]).status()?;
    }
    let _ = critical;
    Ok(())
}

#[cfg(target_os = "macos")]
fn osa_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
