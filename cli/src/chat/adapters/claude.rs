/// Claude hook input is a locator only; private enrollment and live process
/// evidence supply the integration authority.
#[derive(serde::Deserialize)]
pub(super) struct HookInput {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub hook_event_name: String,
    #[serde(default)]
    agent_id: Option<String>,
}

impl HookInput {
    fn parse(bytes: &[u8], event: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 1_048_576,
            "native hook input exceeds its bound"
        );
        let input: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("invalid native hook input"))?;
        anyhow::ensure!(
            input.hook_event_name == event && matches!(event, "SessionStart" | "SessionEnd"),
            "native hook event mismatch or unsupported event"
        );
        crate::chat::protocol::validate_uuid(&input.session_id)?;
        anyhow::ensure!(input.cwd.is_absolute(), "native hook cwd must be absolute");
        anyhow::ensure!(
            input.agent_id.is_none(),
            "Claude child binding is not available"
        );
        Ok(input)
    }
}

#[cfg(target_os = "linux")]
fn write_locator(path: &std::path::Path, id: &str) -> anyhow::Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    crate::chat::protocol::validate_uuid(id)?;
    anyhow::ensure!(
        path.is_absolute(),
        "Claude environment file is not absolute"
    );
    crate::chat::config::validate_path_components(path)?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
            && metadata.nlink() == 1,
        "Claude environment file is not private"
    );
    writeln!(file, "export LAM_CHAT_CLAUDE_SESSION_ID='{id}'")?;
    Ok(())
}

/// Hook failures never block Claude's native work or inject recurring warnings.
pub fn run_hook(event: &str, name: Option<String>) -> i32 {
    #[cfg(target_os = "linux")]
    {
        let deadline = std::time::Instant::now() + crate::chat::delivery::HOOK_DEADLINE;
        let mut stage = "input";
        let result = (|| -> anyhow::Result<()> {
            let bytes = super::hooks::read_input(deadline)?;
            let input = HookInput::parse(&bytes, event)?;
            stage = "binding";
            let paths = crate::chat::config::Paths::discover()?;
            super::binding::claude_check(&paths, &input, name, deadline)?;
            if event == "SessionStart" {
                stage = "output";
                let env_file = std::env::var_os("CLAUDE_ENV_FILE")
                    .ok_or_else(|| anyhow::anyhow!("missing Claude environment file"))?;
                write_locator(std::path::Path::new(&env_file), &input.session_id)?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = super::hooks::record_error("claude", stage);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (event, name);
    }
    0
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{write_locator, HookInput};
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn hook_input_rejects_inherited_child_and_wrong_event() {
        let root = br#"{"session_id":"11111111-1111-4111-8111-111111111111","cwd":"/tmp/owned","hook_event_name":"SessionStart"}"#;
        let input = HookInput::parse(root, "SessionStart").unwrap();
        assert_eq!(input.session_id, "11111111-1111-4111-8111-111111111111");
        assert!(HookInput::parse(root, "SessionEnd").is_err());
        let child = br#"{"session_id":"11111111-1111-4111-8111-111111111111","agent_id":"agent-child","cwd":"/tmp/owned","hook_event_name":"SessionStart"}"#;
        assert!(HookInput::parse(child, "SessionStart").is_err());
    }

    #[test]
    fn locator_export_requires_private_regular_owned_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("claude-env");
        std::fs::write(&file, "export EXISTING=1\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        let fresh = dir.path().join("new-env");
        write_locator(&fresh, id).unwrap();
        assert_eq!(
            std::fs::read_to_string(&fresh).unwrap(),
            format!("export LAM_CHAT_CLAUDE_SESSION_ID='{id}'\n")
        );
        assert_eq!(
            fresh.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        write_locator(&file, id).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!("export EXISTING=1\nexport LAM_CHAT_CLAUDE_SESSION_ID='{id}'\n")
        );
        let link = dir.path().join("link");
        symlink(&file, &link).unwrap();
        assert!(write_locator(&link, id).is_err());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(write_locator(&file, id).is_err());
        assert!(write_locator(&file, "not-a-uuid").is_err());
    }
}
