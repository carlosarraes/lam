# LAM Chat Peer Synchronization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep PC and Mac Chat messages, receipts, registry and human feed synchronized over one persistent SSH link, while local messaging works offline.

**Architecture:** Extend the local store with origin-owned durable events and acknowledged replay cursors. One configured initiator launches an SSH stdio bridge to the peer's local process; neither machine exposes agent sockets or a public server. Imported events never become locally originated events.

**Tech Stack:** Existing Rust Chat service, SQLite transactions, OpenSSH subprocess stdin/stdout, length-bounded protocol.

**Spec:** [Approved design](../specs/2026-09-14-lam-chat-design.md). Dependency: [local implementation plan](2026-09-14-lam-chat-local.md), including its exact domain types, IPC framing and store API. Follow with [rollout](2026-09-14-lam-chat-rollout.md).

## Global Constraints

- Use one persistent SSH connection carrying a versioned, length-bounded bidirectional Chat protocol.
- Configure one initiator per peer pair to avoid duplicate active links.
- SSH host-key checking stays enabled; existing keys/config can be used without copying private keys into LAM.
- No direct forwarding of native client sockets, public TCP listener, or exposure of Codex's app-server is required.
- The destination machine owns each recipient's inbox and delivery-attempt state.
- Commit received events before acknowledging them; do not re-export imported events back to their origin.
- Stale presence is not proof a remote process died.
- No cloud changes, no phone changes, no remote network calls in hooks, no deletion/expiration of history.
- Keep local plan limits: 64 KiB ingestion, 4 KiB inline body, 8 KiB rendered delivery, 1 MiB protocol frame.
- Commit each reviewed deliverable and immediately push its branch.

---

## File map and execution boundary

| File | Responsibility |
| --- | --- |
| `cli/src/chat/peer.rs` | Peer handshake, SSH child lifecycle, replay/ack loop and reconnect |
| `cli/src/chat/store.rs`, `schema.sql` | Origin events, imports, durable contiguous replay cursors |
| `cli/src/chat/types.rs`, `config.rs`, `protocol.rs` | Peer contracts, explicit project allowlist and identity checks |
| `cli/src/chat/daemon.rs`, `commands.rs` | Service-owned link and internal stdio bridge |
| `cli/src/chat/registry.rs`, `tui/draw.rs` | Remote staleness, frozen broadcast roster and observable queue state |
| `cli/tests/chat_peers.rs` | Isolated two-process peer acceptance tests |
| `docs/chat-compatibility.md` | Real PC/Mac link and installed-client evidence |

Work in the implementation worktree. Use test SSH executables/owned local peers until a real host is explicitly selected. Do not run `just sync` or `just deploy`: these also deploy the Worker or copy existing cloud configuration and are not the Chat installation path.

## Task 1: Add replayable, origin-authorized events

**Files:** Extend store/schema/types/config/registry; unit tests in store and peer modules.

**Interfaces:** Reuse `SessionRef`, `Actor`, `Draft`, `Message` from the local plan. Define:

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerEvent {
    pub id: String,
    pub origin: String,
    pub seq: u64,
    pub project: String,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportResult { Applied { through: u64 }, Duplicate { through: u64 } }
```

`Store::export_events(&self, project: &str, after: u64, limit: usize) -> anyhow::Result<Vec<PeerEvent>>` exports locally originated events only, using the local plan's contiguous `project_seq` as `PeerEvent.seq`. `Store::import_event(&mut self, authenticated_peer: &str, event: &PeerEvent, projects: &[String]) -> anyhow::Result<ImportResult>` validates the peer/project and typed payload, then durably applies one contiguous event. `Store::record_peer_ack(&mut self, peer: &str, project: &str, through: u64) -> anyhow::Result<()>` advances only after a validated acknowledgement. Project IDs must have explicit mappings on both machines. Sequence/cursor scope is `(origin, project)`, not a global stream with unauthorized projects filtered out after numbering.

Encode event payloads as internally tagged JSON with `kind` equal to `message`, `receipt`, or `presence`. Message payloads contain the immutable `Message`; receipt payloads contain message ID, recipient incarnation, receipt version and separate transport/exposure evidence; presence payloads contain the `Registration` contract and observation epoch. Validate these typed shapes after framing, not ad hoc unrestricted JSON writes. Observer rows derive from these events and are not another agent-message type.

- [ ] Add and run a failing test proving duplicate imports do not echo:

```rust
#[test]
fn imported_message_is_not_reexported() {
    use crate::chat::{store::Store, types::{Actor, Draft, SessionRef, Target}};
    let dir = tempfile::tempdir().unwrap();
    let mut pc = Store::open(&dir.path().join("pc.sqlite3")).unwrap();
    let mut mac = Store::open(&dir.path().join("mac.sqlite3")).unwrap();
    pc.set_machine("pc").unwrap();
    mac.set_machine("mac").unwrap();
    let sender = Actor::Agent(SessionRef { machine: "pc".into(), incarnation: "pm".into() });
    pc.send(&sender, &Draft {
        key: "one".into(), project: "lam".into(),
        to: vec![Target::Agent(SessionRef { machine: "mac".into(), incarnation: "dev".into() })],
        body: "Pause after the tool".into(), reply_to: None,
    }).unwrap();
    let event = pc.export_events("lam", 0, 10).unwrap().remove(0);
    let projects = vec!["lam".to_string()];
    mac.import_event("pc", &event, &projects).unwrap();
    mac.import_event("pc", &event, &projects).unwrap();
    let outbound = mac.export_events("lam", 0, 10).unwrap();
    assert!(!outbound.iter().any(|event| event.payload["kind"] == "message"));
    assert_eq!(outbound.iter().filter(|event| event.payload["kind"] == "receipt").count(), 1);
}
```

Use internal store fixtures with deterministic machine/session IDs; production IDs come from validated config and registration, not sender-supplied fields. The destination emits its own received-remotely receipt exactly once in the import transaction. That locally owned receipt must replay back to the sender; the imported message must not echo back as a new origin event.

- [ ] Add migration constraints and atomic import:

```sql
CREATE TABLE imported_events (
  event_id TEXT PRIMARY KEY,
  origin TEXT NOT NULL,
  project TEXT NOT NULL,
  origin_seq INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  UNIQUE(origin, project, origin_seq)
);
CREATE TABLE peer_cursors (
  peer TEXT NOT NULL,
  project TEXT NOT NULL,
  received_through INTEGER NOT NULL DEFAULT 0,
  acknowledged_through INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(peer, project)
);
```

Retain locally originated events in the local event stream. Imported message transactions create inbox entries only for frozen local recipients; human observer events do not create agent inboxes. Validate immutable sender ownership, local recipient ownership for receipts, project authorization, message/body bounds and referenced IDs before committing. Same event ID with changed payload is a protocol error, not a successful duplicate.

- [ ] Test duplicate ID/sequence, conflicting payload, wrong origin, unauthorized project, receipt authored by the wrong machine, interrupted transaction, sequence gaps, interleaved local-only projects, old replay cursor after restore, and an imported message referencing an ended incarnation. Apply contiguous batches in sender order; acknowledge only the committed contiguous cursor for that project. Do not infer cross-origin ordering from clocks.
- [ ] Preserve registry disconnected/ended distinctions. Importing a stale participant cannot reopen a confirmed ended incarnation. Reconnect requires destination validation before handoff. Broadcast candidates remain project scoped and frozen, including explicit stale-roster acknowledgement.
- [ ] Run `cargo test --manifest-path cli/Cargo.toml chat` and commit/push: `feat(chat): persist deduplicated peer event replay`.

## Task 2: Carry bidirectional replay over a managed SSH process

**Files:** Create `peer.rs`, `cli/tests/chat_peers.rs`; extend config/protocol/daemon/commands and peer store tests.

**Interfaces:** `PeerConfig` has `machine: String`, `ssh_host: String`, `initiator: bool`, `projects: Vec<String>`. `peer::ssh_command(config: &PeerConfig) -> anyhow::Result<std::process::Command>` returns a validated command. `peer::run_link(config: &PeerConfig, socket: &std::path::Path) -> anyhow::Result<()>` connects a peer loop to the local daemon through an internal authorized connection; it never opens SQLite. `lam chat peer-stdio` is the internal remote bridge, not an agent inbox or arbitrary socket forwarder.

- [ ] Add and run a failing argument-safety test:

```rust
#[test]
fn ssh_never_disables_host_verification() {
    let config = super::PeerConfig {
        machine: "mac".into(), ssh_host: "macbox".into(),
        initiator: true, projects: vec!["lam".into()],
    };
    let command = super::ssh_command(&config).unwrap();
    let args: Vec<_> = command.get_args().map(|v| v.to_string_lossy().into_owned()).collect();
    assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
    assert!(args.contains(&"BatchMode=yes".to_string()));
    assert!(!args.iter().any(|v| v.contains("UserKnownHostsFile=/dev/null")));
}
```

- [ ] Construct arguments without shell interpolation:

```rust
let mut command = std::process::Command::new("ssh");
command.args(["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
    "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=15",
    "-o", "ServerAliveCountMax=3", "--"]);
command.arg(&config.ssh_host).arg("lam chat peer-stdio");
command.stdin(std::process::Stdio::piped());
command.stdout(std::process::Stdio::piped());
command.stderr(std::process::Stdio::piped());
```

Validate the host as an explicit SSH alias/host, not an option or command. The fixed remote command assumes `lam` is discoverable in the peer's noninteractive SSH PATH; status/setup must test this and report failure instead of expanding an untrusted command. Handshake verifies protocol, configured peer machine ID, allowed projects and which endpoint initiates. Reject duplicate links. Keep stderr bounded and separate from protocol stdout. Do not log payload bodies or secret-bearing environment values.

- [ ] Implement independent read/write replay on the single child stdio stream. Reuse local framing limits. Backpressure must not stall the local writer or hook requests. Exchange durable cursors, replay locally originated events, ack imports only after the daemon confirms commit. Remote presence/receipt changes feed local subscriptions. Disconnection changes transport health, not an agent's proven liveness.
- [ ] Use a capped exponential reconnect delay with jitter, starting at one second and capped at 30 seconds. Injectable time/randomness make tests deterministic. Authentication/unknown-host/protocol errors are visible; no insecure fallback. Stop only the owned SSH child on service shutdown and wait for it; never kill shared SSH control masters.
- [ ] Build a two-service test harness with isolated Chat data directories and connected stdio bridge processes. Exercise both directions, an agent not among recipients observing no inbox entry, human-feed convergence, break/reconnect, duplicate replay, one permanently ended recipient among successful recipients, incompatible protocol, wrong machine, unknown host and slow peer backpressure. Local sends and empty hook timing must continue while the link is down.
- [ ] Run all Rust checks from the local plan. Record the two-process evidence. Commit and push: `feat(chat): synchronize peers over persistent SSH`.

## Peer acceptance checkpoint

Select the actual Mac SSH alias with read-only configuration inspection and user confirmation if ambiguous. With explicit installation authority from the rollout plan, verify versions and project mappings on both machines and repeat a local/remote PM exchange. Disconnect only the LAM-owned link, send local and remote messages, reconnect and show converged history with no duplicates. Changing the remote session incarnation must leave the old pending message unavailable, not deliver it to the replacement. A same-host pipe test is not evidence that macOS clients or SSH work.
