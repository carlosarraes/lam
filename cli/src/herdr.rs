//! Herdr pane integration. Inside a Herdr pane, a fresh push marks the pane `blocked`, which
//! flags the pane, tab, and workspace in the sidebar and lets `prefix+o` jump to it; the next
//! keypress clears the flag, the way tmux drops its activity mark once you look. Outside Herdr
//! this is inert and the terminal bell stands alone.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SOURCE: &str = "lam";

pub struct Herdr {
    pane: String,
    bin: PathBuf,
    seq: u64,
    flagged: bool,
}

impl Herdr {
    pub fn detect() -> Option<Self> {
        Self::from_env(
            std::env::var("HERDR_PANE_ID").ok(),
            std::env::var("HERDR_BIN_PATH").ok(),
        )
    }

    fn from_env(pane: Option<String>, bin: Option<String>) -> Option<Self> {
        let pane = pane.filter(|p| !p.trim().is_empty())?;
        Some(Self {
            pane,
            bin: binary(bin),
            seq: 0,
            flagged: false,
        })
    }

    pub fn flag(&mut self, title: &str) {
        self.report("blocked", Some(title));
        self.flagged = true;
    }

    pub fn clear(&mut self) {
        if self.flagged {
            self.report("idle", None);
            self.flagged = false;
        }
    }

    fn report(&mut self, state: &str, message: Option<&str>) {
        self.seq += 1;
        let mut cmd = Command::new(&self.bin);
        cmd.args(["pane", "report-agent", &self.pane])
            .args(["--source", SOURCE, "--agent", SOURCE, "--state", state])
            .args(["--seq", &self.seq.to_string()]);
        if let Some(message) = message {
            cmd.args(["--message", message]);
        }
        quiet(&mut cmd);
        // Waiting inline would stall the draw loop on a slow socket; a detached child would
        // linger as a zombie until exit. A reaper thread avoids both.
        std::thread::spawn(move || {
            let _ = cmd.status();
        });
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        if self.seq == 0 {
            return;
        }
        let mut cmd = Command::new(&self.bin);
        cmd.args(["pane", "release-agent", &self.pane])
            .args(["--source", SOURCE, "--agent", SOURCE]);
        quiet(&mut cmd);
        let _ = cmd.status();
    }
}

/// `HERDR_BIN_PATH` can name a binary that `herdr update` has since replaced, so it only wins
/// while it still exists; otherwise the `herdr` on PATH is the live one.
pub fn binary(hint: Option<String>) -> PathBuf {
    match hint {
        Some(p) if Path::new(&p).is_file() => PathBuf::from(p),
        _ => PathBuf::from("herdr"),
    }
}

/// A child's output would land on the alternate screen and corrupt the frame.
fn quiet(cmd: &mut Command) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_inside_a_pane() {
        assert!(Herdr::from_env(None, None).is_none());
        assert!(Herdr::from_env(Some("  ".into()), None).is_none());
        let h = Herdr::from_env(Some("w1:p2".into()), None).expect("pane id present");
        assert_eq!(h.pane, "w1:p2");
        assert!(!h.flagged);
    }

    #[test]
    fn stale_bin_hint_falls_back_to_path() {
        assert_eq!(
            binary(Some("/nonexistent/herdr (deleted)".into())),
            PathBuf::from("herdr")
        );
        assert_eq!(binary(None), PathBuf::from("herdr"));
        let exe = std::env::current_exe().expect("test binary path");
        assert_eq!(binary(Some(exe.display().to_string())), exe);
    }

    #[test]
    fn clear_is_a_no_op_until_flagged() {
        let mut h = Herdr::from_env(Some("w1:p2".into()), Some("/nonexistent".into())).unwrap();
        h.clear();
        assert_eq!(h.seq, 0, "nothing reported, nothing to clear");
        h.flag("ping");
        assert!(h.flagged);
        h.clear();
        assert!(!h.flagged);
        assert_eq!(h.seq, 2);
        // Drop runs release-agent against a missing binary and must not panic.
    }
}
