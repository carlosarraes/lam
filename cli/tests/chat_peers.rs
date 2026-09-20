#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PC: &str = "11111111-1111-4111-8111-111111111111";
const MAC: &str = "22222222-2222-4222-8222-222222222222";
const PROJECT: &str = "33333333-3333-4333-8333-333333333333";

struct Machine {
    config: PathBuf,
    data: PathBuf,
    observer: PathBuf,
    child: Option<Child>,
}

impl Machine {
    fn new(root: &Path, machine: &str, other: &str, initiator: bool) -> Self {
        let config_dir = root.join("config");
        let data = root.join("data");
        let project_root = root.join("project");
        for path in [&config_dir, &data, &project_root] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = config_dir.join("chat.toml");
        let raw = format!(
            "schema_version = 1\nmachine = \"{machine}\"\ninline_bytes = 4096\nbatch_bytes = 8192\n\
             [[projects]]\nid = \"{PROJECT}\"\nroots = [\"{}\"]\n\
             [[peers]]\nmachine = \"{other}\"\nssh_host = \"fixture\"\ninitiator = {initiator}\nprojects = [\"{PROJECT}\"]\n",
            project_root.display(),
        );
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config)
            .unwrap();
        file.write_all(raw.as_bytes()).unwrap();
        Self {
            config,
            data,
            observer: PathBuf::new(),
            child: None,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lam"));
        command
            .env("LAM_CHAT_CONFIG", &self.config)
            .env("LAM_CHAT_DATA_DIR", &self.data)
            .env("LAM_CONFIG", self.data.join("unused-cloud.toml"));
        command
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

    fn start(&mut self, ssh_shim: Option<&Path>, remote: Option<&Machine>) {
        let mut command = self.command();
        command
            .args(["chat", "serve", "--foreground"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let (Some(shim), Some(remote)) = (ssh_shim, remote) {
            let mut path = std::ffi::OsString::from(shim.as_os_str());
            path.push(":");
            path.push(std::env::var_os("PATH").unwrap_or_default());
            command
                .env("PATH", path)
                .env("LAM_TEST_REMOTE_BINARY", env!("CARGO_BIN_EXE_lam"))
                .env("LAM_TEST_REMOTE_CONFIG", &remote.config)
                .env("LAM_TEST_REMOTE_DATA", &remote.data);
        }
        let status = self.status();
        self.observer = PathBuf::from(status["observer_socket"].as_str().unwrap());
        self.child = Some(command.spawn().unwrap());
        eventually(Duration::from_secs(6), || {
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "Chat daemon exited"
            );
            UnixStream::connect(&self.observer).is_ok()
        });
    }

    fn stop(&mut self) {
        let child = self.child.as_mut().unwrap();
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        eventually(Duration::from_secs(12), || {
            child.try_wait().unwrap().is_some()
        });
        assert!(child.try_wait().unwrap().unwrap().success());
        self.child = None;
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "Chat peer convergence timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn request(socket: &Path, operation: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let raw = serde_json::to_vec(&json!({"version": 1, "operation": operation})).unwrap();
    stream.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
    stream.write_all(&raw).unwrap();
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).unwrap();
    let size = u32::from_be_bytes(header) as usize;
    assert!(size <= 1_048_576);
    let mut body = vec![0; size];
    stream.read_exact(&mut body).unwrap();
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert!(response["ok"] == true, "{response}");
    response["data"].clone()
}

fn send(machine: &Machine, target: &str, key: &str) -> String {
    let data = request(
        &machine.observer,
        json!({"op":"send","draft":{
            "key":key,"project":PROJECT,"to":[{"Human":{"machine":target}}],
            "body":key,"reply_to":null
        }}),
    );
    data["id"].as_str().unwrap().to_owned()
}

fn send_agent(machine: &Machine, incarnation: &str, key: &str) -> String {
    let data = request(
        &machine.observer,
        json!({"op":"send","draft":{
            "key":key,"project":PROJECT,
            "to":[{"Agent":{"machine":MAC,"incarnation":incarnation}}],
            "body":key,"reply_to":null
        }}),
    );
    data["id"].as_str().unwrap().to_owned()
}

fn seed_mac_session(mac: &Machine, incarnation: &str, ended: bool) {
    let connection = rusqlite::Connection::open(mac.data.join("chat.sqlite3")).unwrap();
    let sequence: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(origin_seq), 0) + 1 FROM peer_outbox WHERE project = ?1",
            [PROJECT],
            |row| row.get(0),
        )
        .unwrap();
    if ended {
        connection.execute(
            "UPDATE sessions SET eligible = 0, connected = 0, ended = 1 WHERE machine = ?1 AND incarnation = ?2",
            rusqlite::params![MAC, incarnation],
        ).unwrap();
    } else {
        connection
            .execute(
                "INSERT INTO sessions(machine, incarnation, project, name, client, native_id,
                process_start, eligible, connected, ended)
             VALUES (?1, ?2, ?3, 'dev', 'codex', 'owned-thread', 'owned-process', 1, 1, 0)",
                rusqlite::params![MAC, incarnation, PROJECT],
            )
            .unwrap();
    }
    let state = json!({"registration":{
        "session":{"machine":MAC,"incarnation":incarnation},"project":PROJECT,
        "name":"dev","client":"codex","native_id":"owned-thread",
        "process_start":"owned-process","eligible":!ended
    },"connected":!ended,"ended":ended});
    connection.execute(
        "INSERT INTO peer_outbox(project, origin_seq, event_id, payload_json) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![PROJECT, sequence, uuid::Uuid::new_v4().to_string(),
            json!({"kind":"presence","session":state,"epoch":sequence}).to_string()],
    ).unwrap();
}

fn receipt_state(machine: &Machine, message_id: &str) -> Option<String> {
    let connection = rusqlite::Connection::open(machine.data.join("chat.sqlite3")).unwrap();
    connection
        .query_row(
            "SELECT state FROM delivery_receipts WHERE message_id = ?1",
            [message_id],
            |row| row.get(0),
        )
        .ok()
}

fn inbox_count(machine: &Machine, message_id: &str) -> i64 {
    let connection = rusqlite::Connection::open(machine.data.join("chat.sqlite3")).unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM inbox_entries WHERE message_id = ?1",
            [message_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn ssh_shim(root: &Path, script: &str) -> PathBuf {
    let shim = root.join("bin");
    std::fs::create_dir(&shim).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
    let executable = shim.join("ssh");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&executable)
        .unwrap();
    file.write_all(script.as_bytes()).unwrap();
    drop(file);
    shim
}

fn count(machine: &Machine, id: &str) -> usize {
    let data = request(
        &machine.observer,
        json!({"op":"history","project":PROJECT,"limit":100}),
    );
    data["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event"]["message"]["id"] == id)
        .count()
}

#[test]
fn two_daemons_replay_bidirectionally_after_an_owned_link_outage() {
    let root = tempfile::tempdir().unwrap();
    let script = "#!/bin/sh\nexport LAM_CHAT_CONFIG=\"$LAM_TEST_REMOTE_CONFIG\"\nexport LAM_CHAT_DATA_DIR=\"$LAM_TEST_REMOTE_DATA\"\nexec \"$LAM_TEST_REMOTE_BINARY\" chat peer-stdio\n";
    let shim = ssh_shim(root.path(), script);
    let mut pc = Machine::new(&root.path().join("pc"), PC, MAC, true);
    let mut mac = Machine::new(&root.path().join("mac"), MAC, PC, false);
    let incarnation = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    mac.start(None, None);
    mac.stop();
    seed_mac_session(&mac, incarnation, false);
    mac.start(None, None);
    pc.start(Some(&shim), Some(&mac));
    eventually(Duration::from_secs(8), || {
        let pc_status = pc.status();
        let mac_status = mac.status();
        pc_status["peers"][MAC] == true && mac_status["peers"][PC] == true
    });
    let first = send(&pc, MAC, "pc-to-mac");
    let reverse = send(&mac, PC, "mac-to-pc");
    eventually(Duration::from_secs(8), || {
        let roster = request(&pc.observer, json!({"op":"sessions","project":PROJECT}));
        roster["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["session"]["incarnation"] == incarnation)
    });
    let addressed = send_agent(&pc, incarnation, "pc-to-mac-agent");
    eventually(Duration::from_secs(8), || {
        count(&mac, &first) == 1
            && count(&pc, &reverse) == 1
            && inbox_count(&mac, &addressed) == 1
            && receipt_state(&pc, &addressed).as_deref() == Some("received_remotely")
    });
    mac.stop();
    let offline = send(&pc, MAC, "while-mac-offline");
    let pinned = send_agent(&pc, incarnation, "ended-before-replay");
    seed_mac_session(&mac, incarnation, true);
    mac.start(None, None);
    eventually(Duration::from_secs(25), || {
        count(&mac, &offline) == 1
            && count(&mac, &pinned) == 1
            && receipt_state(&pc, &pinned).as_deref() == Some("unavailable")
    });
    assert_eq!(inbox_count(&mac, &pinned), 0);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(count(&mac, &first), 1);
    assert_eq!(count(&pc, &reverse), 1);
    pc.stop();
    mac.stop();
}

#[test]
fn local_send_does_not_wait_for_a_slow_ssh_peer() {
    let root = tempfile::tempdir().unwrap();
    let shim = ssh_shim(root.path(), "#!/bin/sh\nexec sleep 30\n");
    let mut pc = Machine::new(&root.path().join("pc"), PC, MAC, true);
    let remote = Machine::new(&root.path().join("remote"), MAC, PC, false);
    pc.start(Some(&shim), Some(&remote));
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    let id = send(&pc, MAC, "link-is-slow");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(count(&pc, &id), 1);
    pc.stop();
}
