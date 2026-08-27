use anyhow::{bail, Result};
use std::env;
use std::process::Command;

/// Candidate names in precedence order; each is `None` when that source isn't available.
pub struct Sources {
    pub explicit: Option<String>,
    pub lam_name: Option<String>,
    pub multiplexer: Option<String>,
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn pick(s: Sources) -> Result<String> {
    if let Some(n) = clean(s.explicit)
        .or_else(|| clean(s.lam_name))
        .or_else(|| clean(s.multiplexer))
    {
        return Ok(n);
    }
    bail!(
        "who is asking? pass --name <name> (or set LAM_NAME).\n\
         Inside tmux/zellij/screen the name is inferred as session:window automatically."
    )
}

/// `session:window` for the *current pane* — never the client's active window, which differs
/// when the agent works in a background window.
fn from_tmux() -> Option<String> {
    let pane = env::var("TMUX_PANE").ok()?;
    env::var("TMUX").ok()?;
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t", &pane, "#S:#W"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    clean(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

fn from_zellij() -> Option<String> {
    let session = clean(env::var("ZELLIJ_SESSION_NAME").ok())?;
    Some(match clean(env::var("ZELLIJ_PANE_ID").ok()) {
        Some(pane) => format!("{session}:{pane}"),
        None => session,
    })
}

/// `$STY` is `<pid>.<session>`; `$WINDOW` is the window number.
pub fn screen_name(sty: &str, window: &str) -> String {
    let session = sty.split_once('.').map_or(sty, |(_, rest)| rest);
    if window.is_empty() {
        session.to_string()
    } else {
        format!("{session}:{window}")
    }
}

fn from_screen() -> Option<String> {
    let sty = clean(env::var("STY").ok())?;
    Some(screen_name(&sty, &env::var("WINDOW").unwrap_or_default()))
}

pub fn resolve(explicit: Option<String>) -> Result<String> {
    pick(Sources {
        explicit,
        lam_name: env::var("LAM_NAME").ok(),
        multiplexer: from_tmux().or_else(from_zellij).or_else(from_screen),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(explicit: Option<&str>, lam: Option<&str>, mux: Option<&str>) -> Sources {
        Sources {
            explicit: explicit.map(String::from),
            lam_name: lam.map(String::from),
            multiplexer: mux.map(String::from),
        }
    }

    #[test]
    fn precedence_is_explicit_then_env_then_multiplexer() {
        assert_eq!(
            pick(sources(Some("cli"), Some("env"), Some("0:lam"))).unwrap(),
            "cli"
        );
        assert_eq!(
            pick(sources(None, Some("env"), Some("0:lam"))).unwrap(),
            "env"
        );
        assert_eq!(pick(sources(None, None, Some("0:lam"))).unwrap(), "0:lam");
    }

    #[test]
    fn blank_sources_are_ignored_and_absence_is_an_error() {
        assert_eq!(
            pick(sources(Some("  "), None, Some("0:lam"))).unwrap(),
            "0:lam"
        );
        let err = pick(sources(None, Some(""), None)).unwrap_err().to_string();
        assert!(err.contains("--name"), "{err}");
        assert!(err.contains("LAM_NAME"), "{err}");
    }

    #[test]
    fn screen_names_drop_the_pid() {
        assert_eq!(screen_name("12345.work", "2"), "work:2");
        assert_eq!(screen_name("12345.work", ""), "work");
    }
}
