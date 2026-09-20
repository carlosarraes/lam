//! Reversible, project-scoped native hook installation. Existing unrelated
//! client settings are never rewritten; an occupied target needs manual review.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

pub struct SetupChange {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
    pub owner: String,
}

fn shell_quote(path: &Path) -> Result<String> {
    let path = path.to_str().context("LAM binary path is not UTF-8")?;
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}

fn hook_command(client: &str, event: &str, binary: &Path) -> Result<String> {
    Ok(format!(
        "{} chat hook --client {client} --event {event}",
        shell_quote(binary)?
    ))
}

fn hook_group(client: &str, event: &str, binary: &Path, matcher: Option<&str>) -> Result<Value> {
    let mut group = json!({"hooks": [{
        "type": "command", "command": hook_command(client, event, binary)?, "timeout": 2
    }]});
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    Ok(group)
}

fn desired(client: &str, binary: &Path) -> Result<Vec<u8>> {
    let content = match client {
        "codex" => {
            let mut hooks = serde_json::Map::new();
            for event in [
                "SessionStart",
                "SubagentStart",
                "PostToolUse",
                "Stop",
                "UserPromptSubmit",
                "SubagentStop",
                "SessionEnd",
            ] {
                hooks.insert(
                    event.into(),
                    json!([hook_group(
                        "codex",
                        event,
                        binary,
                        (event == "PostToolUse").then_some("Bash")
                    )?]),
                );
            }
            let mut content = serde_json::to_vec_pretty(&json!({
                "description": "LAM Chat managed project hooks; review through Codex's native hook trust UI",
                "hooks": hooks,
            }))?;
            content.push(b'\n');
            content
        }
        "claude" => {
            let mut content = serde_json::to_vec_pretty(&json!({
                "hooks": {
                    "SessionStart": [hook_group("claude", "SessionStart", binary, None)?],
                    "SessionEnd": [hook_group("claude", "SessionEnd", binary, None)?],
                }
            }))?;
            content.push(b'\n');
            content
        }
        "pi" => {
            let source = include_str!("../../../integrations/pi/lam-chat.ts");
            let binary =
                serde_json::to_string(binary.to_str().context("LAM binary path is not UTF-8")?)?;
            let source = source.replace(
                "process.env.LAM_CHAT_BIN ?? \"lam\"",
                &format!("process.env.LAM_CHAT_BIN ?? {binary}"),
            );
            format!("// LAM Chat managed extension\n{source}").into_bytes()
        }
        _ => bail!("unsupported Chat client: {client}"),
    };
    Ok(content)
}

fn target(client: &str, root: &Path) -> Result<PathBuf> {
    Ok(match client {
        "codex" => root.join(".codex/hooks.json"),
        "claude" => root.join(".claude/settings.local.json"),
        "pi" => root.join(".pi/agent/extensions/lam-chat.ts"),
        _ => bail!("unsupported Chat client: {client}"),
    })
}

pub fn plan(client: &str, root: &Path, remove: bool) -> Result<Vec<SetupChange>> {
    super::config::validate_path_components(root)?;
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot resolve setup root {}", root.display()))?;
    ensure!(root.is_dir(), "Chat setup root must be a directory");
    let path = target(client, &root)?;
    super::config::validate_path_components(&path)?;
    let before = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let expected = desired(client, &std::env::current_exe()?)?;
    if before.as_ref().is_some_and(|bytes| bytes != &expected) {
        bail!(
            "{} differs from the exact LAM Chat template; preserve it and review or merge manually",
            path.display()
        );
    }
    let after = if remove { None } else { Some(expected) };
    if before == after {
        return Ok(Vec::new());
    }
    Ok(vec![SetupChange {
        path,
        before,
        after,
        owner: "lam-chat".into(),
    }])
}

pub fn apply_change(change: &SetupChange) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    ensure!(change.owner == "lam-chat", "unowned Chat setup change");
    super::config::validate_path_components(&change.path)?;
    let parent = change
        .path
        .parent()
        .context("Chat setup target has no parent")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(parent)?;
    super::config::validate_path_components(&change.path)?;
    let metadata = parent.symlink_metadata()?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "Chat setup directory is not private to its owner"
    );
    let current = match fs::read(&change.path) {
        Ok(bytes) => {
            let metadata = change.path.symlink_metadata()?;
            ensure!(
                metadata.is_file()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.nlink() == 1,
                "Chat setup target is not an owned regular file"
            );
            Some(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    ensure!(
        current == change.before,
        "configuration changed; rerun setup preview"
    );
    let filename = change
        .path
        .file_name()
        .context("Chat setup target has no filename")?
        .to_string_lossy();
    let suffix = uuid::Uuid::new_v4();
    if let Some(bytes) = &current {
        let backup = parent.join(format!(".{filename}.lam-chat-backup-{suffix}"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&backup)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        eprintln!("LAM Chat recoverable backup: {}", backup.display());
    }
    if let Some(after) = &change.after {
        let temp = parent.join(format!(".{filename}.lam-chat-{suffix}.tmp"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(after)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &change.path)?;
    } else if current.is_some() {
        fs::remove_file(&change.path)?;
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn run(client: &str, root: &Path, apply: bool, remove: bool) -> Result<()> {
    let changes = plan(client, root, remove)?;
    if changes.is_empty() {
        println!("LAM Chat {client} setup is already in the requested state.");
        return Ok(());
    }
    for change in &changes {
        println!(
            "{} {}",
            if remove { "Remove" } else { "Install" },
            change.path.display()
        );
        if !remove && !apply {
            println!(
                "{}",
                String::from_utf8_lossy(change.after.as_ref().unwrap())
            );
        }
    }
    if apply {
        ensure!(
            cfg!(target_os = "linux") || (cfg!(target_os = "macos") && client == "claude"),
            "native Chat integration is not supported on this platform"
        );
        for change in &changes {
            apply_change(change)?;
        }
        println!("LAM Chat {client} setup applied. New client sessions must review native hook trust where prompted.");
    } else {
        println!("Dry run only; pass --apply to install the reviewed files.");
    }
    Ok(())
}

pub fn default_service_root() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot find home for Chat service")?;
    #[cfg(target_os = "linux")]
    return Ok(home.join(".config/systemd/user"));
    #[cfg(target_os = "macos")]
    return Ok(home.join("Library/LaunchAgents"));
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!("Chat user service is not supported on this platform")
}

fn service_filename() -> Result<&'static str> {
    #[cfg(target_os = "linux")]
    return Ok("lam-chat.service");
    #[cfg(target_os = "macos")]
    return Ok("dev.lam.chat.plist");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!("Chat user service is not supported on this platform")
}

fn service_content(binary: &Path) -> Result<Vec<u8>> {
    let path = binary.to_str().context("LAM binary path is not UTF-8")?;
    ensure!(
        path.starts_with('/') && !path.chars().any(char::is_control),
        "LAM binary path is unsuitable for a user service"
    );
    #[cfg(target_os = "linux")]
    {
        // systemd expands percent specifiers even inside quotes.
        let escaped = path
            .replace('%', "%%")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        Ok(
            include_str!("../../../integrations/systemd/lam-chat.service")
                .replace("@LAM_BINARY@", &format!("\"{escaped}\""))
                .into_bytes(),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let escaped = path
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;");
        Ok(
            include_str!("../../../integrations/launchd/dev.lam.chat.plist")
                .replace("@LAM_BINARY@", &escaped)
                .into_bytes(),
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!("Chat user service is not supported on this platform")
}

pub fn service_plan(root: &Path, remove: bool) -> Result<Vec<SetupChange>> {
    ensure!(root.is_absolute(), "Chat service root must be absolute");
    super::config::validate_path_components(root)?;
    let path = root.join(service_filename()?);
    super::config::validate_path_components(&path)?;
    let before = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let expected = service_content(&std::env::current_exe()?)?;
    if before.as_ref().is_some_and(|bytes| bytes != &expected) {
        bail!(
            "{} differs from the exact LAM Chat service template; preserve it and review manually",
            path.display()
        );
    }
    let after = if remove { None } else { Some(expected) };
    if before == after {
        return Ok(Vec::new());
    }
    Ok(vec![SetupChange {
        path,
        before,
        after,
        owner: "lam-chat".into(),
    }])
}

pub fn run_service(root: &Path, apply: bool, remove: bool, status: bool) -> Result<()> {
    if status {
        let path = root.join(service_filename()?);
        let changes = service_plan(root, false)?;
        println!(
            "LAM Chat service file: {} ({})",
            path.display(),
            if path.exists() {
                "installed, not checked for running state"
            } else {
                "absent"
            }
        );
        ensure!(
            changes.is_empty() || !path.exists(),
            "Chat service file is not the exact owned template"
        );
        return Ok(());
    }
    let changes = service_plan(root, remove)?;
    if changes.is_empty() {
        println!("LAM Chat service file is already in the requested state.");
        return Ok(());
    }
    for change in &changes {
        println!(
            "{} {}",
            if remove { "Remove" } else { "Install" },
            change.path.display()
        );
        if !remove && !apply {
            println!(
                "{}",
                String::from_utf8_lossy(change.after.as_ref().unwrap())
            );
        }
    }
    if apply {
        for change in &changes {
            apply_change(change)?;
        }
        println!(
            "LAM Chat service file changed. No service manager was enabled, started, or stopped."
        );
    } else {
        println!("Dry run only; pass --apply to change the reviewed service file.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn service_quotes_spaced_binary_paths_and_rejects_control_chars() {
        let unit =
            String::from_utf8(service_content(Path::new("/tmp/LAM build/lam")).unwrap()).unwrap();
        assert!(unit.contains(
            "ExecStart=\"/tmp/LAM build/lam\" chat serve --foreground --native-bindings"
        ));
        assert!(service_content(Path::new("/tmp/lam\nunsafe")).is_err());
    }

    #[test]
    fn setup_refuses_to_clobber_a_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"user edited").unwrap();
        let change = SetupChange {
            path: path.clone(),
            before: Some(b"old".to_vec()),
            after: Some(b"new".to_vec()),
            owner: "lam-chat".into(),
        };
        assert!(apply_change(&change).is_err());
        assert_eq!(fs::read(path).unwrap(), b"user edited");
    }
}
