#![cfg(unix)]

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lam"))
}

#[cfg(target_os = "linux")]
#[test]
fn setup_preview_then_apply_creates_only_owned_project_hook() {
    let root = tempfile::tempdir().unwrap();
    let hook = root.path().join(".codex/hooks.json");
    let preview = command()
        .args(["chat", "setup", "--client", "codex", "--root"])
        .arg(root.path())
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(!hook.exists());
    let applied = command()
        .args(["chat", "setup", "--client", "codex", "--root"])
        .arg(root.path())
        .arg("--apply")
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let text = std::fs::read_to_string(&hook).unwrap();
    assert!(text.contains(" chat hook --client codex"));
    assert_eq!(
        std::fs::metadata(&hook).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn setup_refuses_to_clobber_an_existing_unowned_hook() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(".codex");
    std::fs::create_dir(&directory).unwrap();
    let hook = directory.join("hooks.json");
    std::fs::write(&hook, b"{\"hooks\":{\"SessionStart\":[]}}\n").unwrap();
    let applied = command()
        .args(["chat", "setup", "--client", "codex", "--root"])
        .arg(root.path())
        .arg("--apply")
        .output()
        .unwrap();
    assert!(!applied.status.success());
    assert_eq!(
        std::fs::read(&hook).unwrap(),
        b"{\"hooks\":{\"SessionStart\":[]}}\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn setup_repeat_apply_and_remove_preserve_recoverable_backups() {
    for (client, relative) in [
        ("codex", ".codex/hooks.json"),
        ("claude", ".claude/settings.local.json"),
        ("pi", ".pi/agent/extensions/lam-chat.ts"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let invoke = |extra: &[&str]| {
            command()
                .args(["chat", "setup", "--client", client, "--root"])
                .arg(root.path())
                .args(extra)
                .output()
                .unwrap()
        };
        assert!(invoke(&["--apply"]).status.success());
        let path = root.path().join(relative);
        let original = std::fs::read(&path).unwrap();
        assert!(invoke(&["--apply"]).status.success());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(invoke(&["--remove", "--dry-run"]).status.success());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(invoke(&["--remove", "--apply"]).status.success());
        assert!(!path.exists());
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("lam-chat-backup")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), original);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn setup_refuses_a_user_edit_even_when_managed_marker_remains() {
    for (client, relative) in [
        ("codex", ".codex/hooks.json"),
        ("claude", ".claude/settings.local.json"),
        ("pi", ".pi/agent/extensions/lam-chat.ts"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let invoke = |extra: &[&str]| {
            command()
                .args(["chat", "setup", "--client", client, "--root"])
                .arg(root.path())
                .args(extra)
                .output()
                .unwrap()
        };
        assert!(invoke(&["--apply"]).status.success());
        let path = root.path().join(relative);
        let mut edited = std::fs::read(&path).unwrap();
        edited.push(b' ');
        std::fs::write(&path, &edited).unwrap();
        let result = invoke(&["--remove", "--apply"]);
        assert!(!result.status.success(), "{client} accepted an edited file");
        assert_eq!(std::fs::read(path).unwrap(), edited);
    }
}

#[test]
fn service_install_is_explicit_and_reversible_in_a_staged_directory() {
    let root = tempfile::tempdir().unwrap();
    let unit = root.path().join(if cfg!(target_os = "linux") {
        "lam-chat.service"
    } else {
        "dev.lam.chat.plist"
    });
    let invoke = |action: &str, apply: bool| {
        let mut command = command();
        command
            .args(["chat", "service", action, "--root"])
            .arg(root.path());
        if apply {
            command.arg("--apply");
        }
        command.output().unwrap()
    };
    assert!(invoke("install", false).status.success());
    assert!(!unit.exists());
    assert!(invoke("install", true).status.success());
    let content = std::fs::read_to_string(&unit).unwrap();
    if cfg!(target_os = "linux") {
        assert!(content.contains("chat serve --foreground --native-bindings"));
    } else {
        assert!(content.contains("<string>serve</string>"));
        assert!(content.contains("<string>--native-bindings</string>"));
    }
    assert!(invoke("install", true).status.success());
    assert!(invoke("uninstall", true).status.success());
    assert!(!unit.exists());
    assert!(std::fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("lam-chat-backup")
    }));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_native_setup_remains_preview_only() {
    let root = tempfile::tempdir().unwrap();
    let preview = command()
        .args(["chat", "setup", "--client", "codex", "--root"])
        .arg(root.path())
        .output()
        .unwrap();
    assert!(preview.status.success());
    let apply = command()
        .args(["chat", "setup", "--client", "codex", "--root"])
        .arg(root.path())
        .arg("--apply")
        .output()
        .unwrap();
    assert!(!apply.status.success());
    assert!(!root.path().join(".codex/hooks.json").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_claude_setup_is_explicit_and_reversible() {
    let root = tempfile::Builder::new()
        .prefix("lam-chat-mac-setup-")
        .tempdir_in("/private/tmp")
        .unwrap();
    let hook = root.path().join(".claude/settings.local.json");
    let invoke = |extra: &[&str]| {
        command()
            .args(["chat", "setup", "--client", "claude", "--root"])
            .arg(root.path())
            .args(extra)
            .output()
            .unwrap()
    };
    assert!(invoke(&["--dry-run"]).status.success());
    assert!(!hook.exists());
    let applied = invoke(&["--apply"]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(std::fs::read_to_string(&hook)
        .unwrap()
        .contains("--client claude"));
    assert_eq!(hook.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert!(invoke(&["--apply"]).status.success());
    assert!(invoke(&["--remove", "--apply"]).status.success());
    assert!(!hook.exists());
}

#[test]
fn disabled_chat_hook_is_silent_and_creates_no_state() {
    let root = tempfile::tempdir().unwrap();
    let output = command()
        .args(["chat", "hook", "--client", "pi", "--event", "SessionStart"])
        .env("LAM_CHAT_DISABLE", "1")
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert!(!root.path().join("config/lam").exists());
    assert!(!root.path().join("data/lam").exists());
}
