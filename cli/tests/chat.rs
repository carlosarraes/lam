#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    dir: tempfile::TempDir,
    child: Option<Child>,
}

#[test]
fn chat_send_help_is_available_without_cloud_config() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["chat", "send", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--to"));
    assert!(help.contains("--message"));
    for args in [
        vec!["chat", "send", "--to", "owned", "--message", "body"],
        vec!["inbox", "show", "11111111-1111-4111-8111-111111111111"],
    ] {
        let result = fixture
            .command()
            .env_remove("CODEX_THREAD_ID")
            .args(args)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr)
            .contains("validated native participant binding"));
    }
    let override_sender = fixture
        .command()
        .args([
            "chat",
            "send",
            "--to",
            "owned",
            "--message",
            "body",
            "--sender",
            "Carlos",
        ])
        .output()
        .unwrap();
    assert!(!override_sender.status.success());
    assert!(!fixture.dir.path().join("config/chat.toml").exists());
    assert!(!fixture.dir.path().join("data/chat.sqlite3").exists());
}

#[test]
fn codex_hook_operational_errors_are_silent_and_privately_bounded() {
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args([
            "chat",
            "hook",
            "--client",
            "codex",
            "--event",
            "PostToolUse",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{invalid private body and credential")
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
    let path = fixture.dir.path().join("data/hook-errors.jsonl");
    let diagnostic = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        diagnostic,
        "{\"client\":\"codex\",\"stage\":\"input\",\"status\":\"error\"}\n"
    );
    assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
    assert!(diagnostic.len() < 128);
}

#[test]
fn pi_extension_helper_reports_failure_only_to_its_parent_process() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["chat", "hook", "--client", "pi", "--event", "SessionStart"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("data/hook-errors.jsonl")).unwrap(),
        "{\"client\":\"pi\",\"stage\":\"input\",\"status\":\"error\"}\n"
    );
}

#[test]
fn native_validator_is_explicitly_experimental_and_status_distinguishes_it() {
    let mut fixture = Fixture::new();
    let help = fixture
        .command()
        .args(["chat", "serve", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("--native-bindings"));
    assert!(String::from_utf8_lossy(&help.stdout).contains("unverified"));
    fixture.start_with_bindings(false);
    assert_eq!(fixture.status()["participant_auth"], "disabled");
    let plain = fixture.command().args(["chat", "status"]).output().unwrap();
    assert!(String::from_utf8_lossy(&plain.stdout).contains("disabled"));
    fixture.terminate();
    fixture.start_with_bindings(true);
    assert_eq!(
        fixture.status()["participant_auth"],
        "experimental native validator; native gates incomplete"
    );
    fixture.terminate();
}

#[test]
fn inbox_retry_is_discoverable_and_fails_closed_without_native_binding() {
    let fixture = Fixture::new();
    let help = fixture
        .command()
        .args(["inbox", "retry", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("duplicate delivery"));
    let retry = fixture
        .command()
        .args(["inbox", "retry", "11111111-1111-4111-8111-111111111111"])
        .output()
        .unwrap();
    assert!(!retry.status.success());
    assert!(String::from_utf8_lossy(&retry.stderr).contains("validated native participant binding"));
    assert!(!fixture.dir.path().join("config/chat.toml").exists());
    assert!(!fixture.dir.path().join("data/chat.sqlite3").exists());
}
impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            child: None,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lam"));
        cmd.env("LAM_CHAT_CONFIG", self.dir.path().join("config/chat.toml"))
            .env("LAM_CHAT_DATA_DIR", self.dir.path().join("data"))
            .env(
                "LAM_CONFIG",
                self.dir.path().join("nonexistent-cloud-config"),
            );
        cmd
    }
    fn status(&self) -> Value {
        let output = self
            .command()
            .args(["chat", "status", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn start(&mut self) -> (PathBuf, PathBuf) {
        self.start_with_bindings(false)
    }
    fn start_with_bindings(&mut self, native: bool) -> (PathBuf, PathBuf) {
        let status = self.status();
        let observer = PathBuf::from(status["observer_socket"].as_str().unwrap());
        let participant = PathBuf::from(status["participant_socket"].as_str().unwrap());
        let mut command = self.command();
        command.args(["chat", "serve", "--foreground"]);
        if native {
            command.arg("--native-bindings");
        }
        self.child = Some(
            command
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if UnixStream::connect(&observer).is_ok() {
                break;
            }
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "daemon exited during startup"
            );
            assert!(
                Instant::now() < deadline,
                "daemon readiness deadline expired"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        (observer, participant)
    }
    fn terminate(&mut self) {
        let child = self.child.as_mut().unwrap();
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "SIGTERM did not stop daemon");
            std::thread::sleep(Duration::from_millis(10));
        }
        self.child.take();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn write(stream: &mut UnixStream, value: Value) {
    let bytes = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
fn read(stream: &mut UnixStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let len = u32::from_be_bytes(header) as usize;
    assert!(len <= 1_048_576);
    let mut body = vec![0; len];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
fn request(path: &std::path::Path, operation: Value) -> Value {
    let mut stream = UnixStream::connect(path).unwrap();
    write(&mut stream, json!({"version": 1, "operation": operation}));
    read(&mut stream)
}

#[test]
fn chat_daemon_is_private_single_instance_and_survives_restart() {
    let mut f = Fixture::new();
    assert_eq!(f.status()["running"], false);
    assert!(f.status()["action"]
        .as_str()
        .unwrap()
        .contains("serve --foreground"));
    let (observer, participant) = f.start();
    assert_eq!(f.status()["running"], true);
    let second = f
        .command()
        .args(["chat", "serve", "--foreground"])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));
    let runtime = f.dir.path().join("other-runtime");
    std::fs::create_dir(&runtime).unwrap();
    let other_runtime = f
        .command()
        .env("XDG_RUNTIME_DIR", runtime)
        .args(["chat", "serve", "--foreground"])
        .output()
        .unwrap();
    assert!(!other_runtime.status.success());
    assert!(String::from_utf8_lossy(&other_runtime.stderr).contains("already running"));
    let mut unauthorized = UnixStream::connect(&participant).unwrap();
    write(
        &mut unauthorized,
        json!({"version": 1, "binding": "invented-observer-credential"}),
    );
    assert_eq!(read(&mut unauthorized)["ok"], false);
    let mut wrong_version = UnixStream::connect(&observer).unwrap();
    write(
        &mut wrong_version,
        json!({"version": 2, "operation": {"op": "status"}}),
    );
    assert_eq!(read(&mut wrong_version)["ok"], false);
    assert_eq!(
        request(&observer, json!({"op": "status", "role": "observer"}))["ok"],
        false
    );
    let machine = f.status()["machine"].as_str().unwrap().to_string();
    let project = "22222222-2222-4222-8222-222222222222";
    let sent = request(
        &observer,
        json!({"op": "send", "draft": {"key": "durable", "project": project, "to": [{"Human": {"machine": machine}}], "body": "retained", "reply_to": null}}),
    );
    assert_eq!(sent["ok"], true, "{sent}");
    f.terminate();
    assert!(!observer.exists());
    assert!(!participant.exists());
    let (observer, _) = f.start();
    let history = request(
        &observer,
        json!({"op": "history", "project": project, "limit": 10}),
    );
    assert_eq!(
        history["data"]["events"][0]["event"]["message"]["id"],
        sent["data"]["id"]
    );
    f.terminate();
}

#[test]
fn chat_subscription_wakes_on_commit_and_resumes_without_blocking_status() {
    let mut f = Fixture::new();
    let (observer, _) = f.start();
    let project = "22222222-2222-4222-8222-222222222222";
    let mut subscription = UnixStream::connect(&observer).unwrap();
    write(
        &mut subscription,
        json!({"version": 1, "operation": {"op": "subscribe", "project": project, "limit": 10}}),
    );
    let first = read(&mut subscription);
    assert_eq!(first["data"]["events"], json!([]));
    let machine = f.status()["machine"].as_str().unwrap().to_string();
    let sent = request(
        &observer,
        json!({"op": "send", "draft": {"key": "notify", "project": project, "to": [{"Human": {"machine": machine}}], "body": "wake now", "reply_to": null}}),
    );
    assert_eq!(
        read(&mut subscription)["data"]["events"][0]["event"]["message"]["id"],
        sent["data"]["id"]
    );
    assert_eq!(f.status()["running"], true);
    drop(subscription);
    let mut resumed = UnixStream::connect(&observer).unwrap();
    write(
        &mut resumed,
        json!({"version": 1, "operation": {"op": "subscribe", "project": project, "limit": 10, "cursor": first["data"]["cursor"]}}),
    );
    assert_eq!(
        read(&mut resumed)["data"]["events"][0]["event"]["message"]["id"],
        sent["data"]["id"]
    );
    f.terminate();
}

#[test]
fn chat_slow_subscriber_cannot_block_writes_or_exhaust_regular_slots() {
    let mut f = Fixture::new();
    let (observer, _) = f.start();
    let project = "22222222-2222-4222-8222-222222222222";
    let mut subscriptions = Vec::new();
    for _ in 0..16 {
        let mut stream = UnixStream::connect(&observer).unwrap();
        write(
            &mut stream,
            json!({"version": 1, "operation": {"op": "subscribe", "project": project, "limit": 1}}),
        );
        assert_eq!(read(&mut stream)["ok"], true);
        subscriptions.push(stream);
    }
    assert_eq!(
        request(
            &observer,
            json!({"op": "subscribe", "project": project, "limit": 1})
        )["ok"],
        false
    );
    let machine = f.status()["machine"].as_str().unwrap().to_string();
    for key in 0..12 {
        let sent = request(
            &observer,
            json!({"op": "send", "draft": {"key": key.to_string(), "project": project, "to": [{"Human": {"machine": machine}}], "body": "x".repeat(65_536), "reply_to": null}}),
        );
        assert_eq!(sent["ok"], true);
    }
    assert_eq!(f.status()["running"], true);
    // All subscriber sockets are intentionally left unread during shutdown.
    f.terminate();
}

#[test]
fn chat_refuses_unsafe_socket_permissions_and_never_initializes_on_help() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    assert!(f
        .command()
        .args(["chat", "--help"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(!f.dir.path().join("config").exists());
    assert!(!f.dir.path().join("data").exists());
    let (observer, _) = f.start();
    std::fs::set_permissions(&observer, std::fs::Permissions::from_mode(0o666)).unwrap();
    let output = f
        .command()
        .args(["chat", "status", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("private"));
    std::fs::set_permissions(&observer, std::fs::Permissions::from_mode(0o600)).unwrap();
    f.terminate();
}
