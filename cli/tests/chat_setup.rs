#![cfg(unix)]

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lam"))
}

fn assert_codex_hook_deadlines(path: &std::path::Path) {
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let hooks = document["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 7);
    for groups in hooks.values() {
        assert_eq!(groups[0]["hooks"][0]["timeout"], 3);
    }
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
    assert_codex_hook_deadlines(&hook);
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

#[test]
fn user_scoped_codex_setup_preserves_existing_hooks() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".codex");
    std::fs::create_dir(&directory).unwrap();
    let hook = directory.join("hooks.json");
    let existing = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}]
            }],
            "PostToolUseFailure": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}]
            }],
            "PreToolUse": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}]
            }]
        }
    });
    std::fs::write(
        &hook,
        format!("{}\n", serde_json::to_string_pretty(&existing).unwrap()),
    )
    .unwrap();

    let applied = command()
        .args([
            "chat", "setup", "--client", "codex", "--scope", "user", "--root",
        ])
        .arg(home.path())
        .arg("--apply")
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );

    let installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&hook).unwrap()).unwrap();
    assert_eq!(
        installed["hooks"]["PreToolUse"],
        existing["hooks"]["PreToolUse"]
    );
    assert_eq!(
        installed["hooks"]["PostToolUseFailure"],
        existing["hooks"]["PostToolUseFailure"]
    );
    assert_eq!(
        installed["hooks"]["PostToolUse"][0],
        existing["hooks"]["PostToolUse"][0]
    );
    assert_eq!(
        installed["hooks"]["PostToolUse"].as_array().unwrap().len(),
        2
    );
    for event in [
        "SessionStart",
        "SubagentStart",
        "PostToolUse",
        "Stop",
        "UserPromptSubmit",
        "SubagentStop",
        "SessionEnd",
    ] {
        assert!(installed["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(" chat hook --client codex")));
    }
}

#[test]
fn user_scoped_codex_remove_preserves_existing_hooks() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".codex");
    std::fs::create_dir(&directory).unwrap();
    let hook = directory.join("hooks.json");
    let existing = serde_json::json!({
        "hooks": {
            "PostToolUse": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}]
            }],
            "PostToolUseFailure": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}]
            }],
            "PreToolUse": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}]
            }]
        }
    });
    std::fs::write(
        &hook,
        format!("{}\n", serde_json::to_string_pretty(&existing).unwrap()),
    )
    .unwrap();
    let invoke = |extra: &[&str]| {
        command()
            .args([
                "chat", "setup", "--client", "codex", "--scope", "user", "--root",
            ])
            .arg(home.path())
            .args(extra)
            .output()
            .unwrap()
    };
    assert!(invoke(&["--apply"]).status.success());

    let removed = invoke(&["--remove", "--apply"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let restored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&hook).unwrap()).unwrap();
    assert_eq!(restored, existing);
}

#[test]
fn user_scoped_codex_remove_is_byte_for_byte_noop_without_lam_hooks() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".codex");
    std::fs::create_dir(&directory).unwrap();
    let hook = directory.join("hooks.json");
    let existing = br#"{"hooks":{"PreToolUse":[{"matcher":"^Bash$","hooks":[{"command":"atuin hook codex","type":"command"}]}]}}
"#;
    std::fs::write(&hook, existing).unwrap();

    let removed = command()
        .args([
            "chat", "setup", "--client", "codex", "--scope", "user", "--root",
        ])
        .arg(home.path())
        .args(["--remove", "--apply"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(std::fs::read(&hook).unwrap(), existing);
}

#[test]
fn user_scoped_claude_setup_targets_user_settings_and_preserves_existing_hooks() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".claude");
    std::fs::create_dir(&directory).unwrap();
    let settings = directory.join("settings.json");
    let existing = serde_json::json!({
        "permissions": {"defaultMode": "acceptEdits"},
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }],
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }]
        }
    });
    std::fs::write(
        &settings,
        format!("{}\n", serde_json::to_string_pretty(&existing).unwrap()),
    )
    .unwrap();

    let applied = command()
        .args([
            "chat", "setup", "--client", "claude", "--scope", "user", "--root",
        ])
        .arg(home.path())
        .arg("--apply")
        .output()
        .unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );

    assert!(!directory.join(".claude/settings.local.json").exists());
    let installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(installed["permissions"], existing["permissions"]);
    assert_eq!(
        installed["hooks"]["PreToolUse"],
        existing["hooks"]["PreToolUse"]
    );
    assert_eq!(
        installed["hooks"]["PostToolUse"],
        existing["hooks"]["PostToolUse"]
    );
    for event in ["SessionStart", "SessionEnd"] {
        let groups = installed["hooks"][event].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(" chat hook --client claude"));
    }
}

#[test]
fn user_scoped_claude_remove_preserves_unrelated_user_settings() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".claude");
    std::fs::create_dir(&directory).unwrap();
    let settings = directory.join("settings.json");
    let existing = serde_json::json!({
        "permissions": {"defaultMode": "acceptEdits"},
        "hooks": {
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }]
        }
    });
    std::fs::write(
        &settings,
        format!("{}\n", serde_json::to_string_pretty(&existing).unwrap()),
    )
    .unwrap();
    let invoke = |extra: &[&str]| {
        command()
            .args([
                "chat", "setup", "--client", "claude", "--scope", "user", "--root",
            ])
            .arg(home.path())
            .args(extra)
            .output()
            .unwrap()
    };
    assert!(invoke(&["--apply"]).status.success());

    let removed = invoke(&["--remove", "--apply"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let restored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored, existing);
}

#[test]
fn setup_upgrades_the_previous_managed_codex_deadline() {
    let root = tempfile::tempdir().unwrap();
    let hook = root.path().join(".codex/hooks.json");
    let invoke = || {
        command()
            .args(["chat", "setup", "--client", "codex", "--root"])
            .arg(root.path())
            .arg("--apply")
            .output()
            .unwrap()
    };
    assert!(invoke().status.success());
    let previous = std::fs::read_to_string(&hook)
        .unwrap()
        .replace("\"timeout\": 3", "\"timeout\": 2");
    assert_ne!(previous, std::fs::read_to_string(&hook).unwrap());
    std::fs::write(&hook, previous).unwrap();

    let upgraded = invoke();
    assert!(
        upgraded.status.success(),
        "{}",
        String::from_utf8_lossy(&upgraded.stderr)
    );
    assert_codex_hook_deadlines(&hook);
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
        assert!(content.contains("<string>--foreground</string>"));
        assert!(!content.contains("<string>--native-bindings</string>"));
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
fn macos_codex_setup_is_explicit_and_reversible() {
    let root = tempfile::Builder::new()
        .prefix("lam-chat-mac-codex-setup-")
        .tempdir_in("/private/tmp")
        .unwrap();
    let hook = root.path().join(".codex/hooks.json");
    let invoke = |extra: &[&str]| {
        command()
            .args(["chat", "setup", "--client", "codex", "--root"])
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
        .contains("--client codex"));
    assert_codex_hook_deadlines(&hook);
    assert_eq!(hook.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert!(invoke(&["--apply"]).status.success());
    assert!(invoke(&["--remove", "--apply"]).status.success());
    assert!(!hook.exists());
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
