# macOS Codex cx relay implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let new Codex 0.155.1 sessions launched through `cx` on the Mac enroll in experimental LAM Chat, receive peer text after tool calls, and wake the exact idle thread through an authenticated narrow relay.

**Architecture:** `cx` adds a private per-runtime relay with only Bind, Inspect, and Queue operations. LAM's existing native binding supplies the per-session secret, keeps ownership of claims and receipts, and uses `cx` only because `cx` owns the Mac Codex stdio app-server connection. Linux retains its direct Codex control-socket adapter.

**Tech stack:** Rust 2021, Tokio Unix sockets, length-prefixed JSON, macOS libproc process evidence, Codex app-server JSON-RPC, SQLite-backed LAM Chat, Cargo tests, Python fake Codex runtime.

**Spec:** `docs/superpowers/specs/2026-09-21-macos-codex-cx-relay-design.md`

## Global constraints

- Work inline. Do not dispatch subagents.
- LAM work stays in `/home/carraes/projs/lam/.worktrees/lam-chat` on `feat/lam-chat`.
- cx work stays in `/home/carraes/projs/cx/.worktrees/lam-relay` on `feat/lam-relay`; preserve the main checkout.
- Test-first for every behavior change. Run each focused test red before production code and green afterward.
- Push each repository immediately after every commit.
- macOS arm64, cx relay protocol 1, and Codex CLI 0.155.1 are the initial Mac gate.
- Linux keeps Codex 0.153.4 and 0.154.0 through its existing direct Unix-socket path.
- Relay frames are length-prefixed JSON, at most 32 KiB, under one absolute two-second deadline.
- Peer bodies and credentials never enter logs, process arguments, or Codex environment variables.
- Accepted requires a matched native receipt. Pre-write failures are NotSubmitted. Any outcome after a possible app-server write is Unknown.
- Existing agents remain on herdr. Tests and rollout may start or stop only owned disposable sessions.
- The ordinary LAM cloud config and cx account state remain byte-for-byte unchanged during rollout.

## File structure

### cx repository

- Create `build.rs`: compile the macOS process-evidence helper only for a macOS target.
- Create `src/macos_process.c`: return exact PID, parent PID, UID, start time, boot ID, executable, and peer ancestry evidence using libproc.
- Create `src/process.rs`: portable process lifetime and descendant checks; Linux uses `/proc`, macOS uses the C helper.
- Create `src/lam_relay.rs`: strict protocol types, bounded framing, binding table, request authentication, pending app-server correlations, and fixed responses.
- Modify `Cargo.toml`: add `cc` as a build dependency.
- Modify `src/lib.rs`: expose only the internal modules needed by the binary and integration tests.
- Modify `src/runtime.rs`: create `lam.sock`, export only its path to the Codex app-server, run the relay accept loop, and route three internal app-server requests without changing account switching.
- Modify `tests/fake_codex.py`: emulate thread/read and thread/queue/add, and let the fake app-server launch an owned descendant Bind helper.
- Modify `tests/runtime.rs`: exercise the real cx runtime, relay socket, fake app server, and failure cases.

### LAM repository

- Modify `cli/src/chat/adapters/binding.rs`: retain and validate the cx relay locator/process in Mac Codex bindings; bind cx after LAM enrollment.
- Modify `cli/src/chat/adapters/codex.rs`: run Codex hooks on macOS, add bounded Mac stdout, and implement the relay Inspect/Queue client.
- Modify `cli/src/chat/adapters/mod.rs`: export Codex queue helpers on Linux and macOS.
- Modify `cli/src/chat/daemon.rs`: run the existing Codex queue coordinator on macOS as well as Linux.
- Modify `cli/src/chat/adapters/macos_process.rs` and `.c` only if the existing snapshot lacks a required exact relation or socket-owner query.
- Modify `cli/tests/chat.rs` and platform-specific unit modules: lock down Mac hook and relay behavior.
- Modify `docs/chat-compatibility.md`: record evidence, limitations, installed commits, and rollback paths after the live gate.

---

### Task 1: cx relay protocol and bounded framing

**Files:**
- Create: `/home/carraes/projs/cx/.worktrees/lam-relay/src/lam_relay.rs`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/src/lib.rs`

**Interfaces:**
- Produces `lam_relay::Request::{Bind, Inspect, Queue}` with `#[serde(deny_unknown_fields)]` payloads.
- Produces `lam_relay::Response`, `lam_relay::Submission::{NotStarted, Uncertain}`, and fixed error codes.
- Produces async `read_frame(&mut UnixStream, deadline: Instant) -> Result<Request>` and `write_frame(&mut UnixStream, &Response, deadline: Instant) -> Result<()>`.
- Frame encoding is a four-byte big-endian length followed by UTF-8 JSON. Length must be `1..=32768`.

- [ ] **Step 1: Write protocol tests before the module exists**

Add tests that hand-encode literal frames and assert:

```rust
let request = decode(br#"{"version":1,"operation":"inspect","thread_id":"11111111-1111-4111-8111-111111111111","binding":"aaaa...64"}"#)?;
assert!(matches!(request, Request::Inspect { .. }));
assert!(decode(br#"{"version":2,"operation":"inspect",...}"#).is_err());
assert!(decode(br#"{"version":1,"operation":"queue",...,"extra":true}"#).is_err());
assert!(decode(&vec![b'x'; 32_769]).is_err());
```

Add an async UnixStream-pair test that withholds the body after the declared length and proves the absolute deadline expires. Add a response test proving provider text supplied to the internal error constructor never appears in serialized output.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test --locked lam_relay -- --nocapture
```

Expected: compilation fails because `lam_relay` and its types do not exist.

- [ ] **Step 3: Implement the strict frame and type layer**

Use this public shape:

```rust
pub const FRAME_LIMIT: usize = 32 * 1024;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Bind { version: u32, thread_id: String, binding: String },
    Inspect { version: u32, thread_id: String, binding: String },
    Queue { version: u32, thread_id: String, binding: String,
            attempt_id: String, text: String },
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Submission { NotStarted, Uncertain }
```

Validate UUIDs with `uuid::Uuid::parse_str` only after adding `uuid = "1"` if cx does not already depend on it. Validate bindings as exactly 64 lowercase hexadecimal characters and decode them to 32 secret bytes; request values carrying secrets or peer bodies must not derive `Debug`. Response constructors accept fixed enum error codes, never arbitrary upstream strings.

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run `cargo test --locked lam_relay -- --nocapture` and require all new tests to pass.

- [ ] **Step 5: Commit and push cx**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/lam_relay.rs
git commit -m "feat(relay): define bounded LAM protocol"
git push -u upstream feat/lam-relay
```

### Task 2: cx exact-process enrollment

**Files:**
- Create: `/home/carraes/projs/cx/.worktrees/lam-relay/build.rs`
- Create: `/home/carraes/projs/cx/.worktrees/lam-relay/src/macos_process.c`
- Create: `/home/carraes/projs/cx/.worktrees/lam-relay/src/process.rs`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/Cargo.toml`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/src/lib.rs`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/src/lam_relay.rs`

**Interfaces:**
- Produces `ProcessEvidence::read(pid)`, `validate()`, and `validate_descendant(pid)` with PID reuse protection.
- Produces `peer_pid(&UnixStream) -> Result<u32>` using `SO_PEERCRED` on Linux and `LOCAL_PEERPID` on macOS.
- `BindingTable::bind(thread, secret, peer, tui)` accepts only an exact descendant and is idempotent for the same secret.
- `BindingTable::authenticate(thread, secret)` uses a constant-time byte comparison and rejects conflicting or missing bindings.

- [ ] **Step 1: Write failing lifetime and binding tests**

Tests must use real child processes and Unix sockets. Cover:

```rust
let tui = ProcessEvidence::read(child.id())?;
assert!(tui.validate_descendant(grandchild_pid).is_ok());
assert!(tui.validate_descendant(unrelated_pid).is_err());

table.bind(THREAD, SECRET, descendant, &tui)?;
table.bind(THREAD, SECRET, descendant, &tui)?; // idempotent
assert!(table.bind(THREAD, OTHER_SECRET, descendant, &tui).is_err());
assert!(table.authenticate(THREAD, OTHER_SECRET).is_err());
```

Mutation target: accepting a same-UID unrelated process, PID reuse, ordinary string equality, or secret replacement must fail a test.

- [ ] **Step 2: Run focused tests and verify RED**

Run these separately:

```bash
cargo test --locked process -- --nocapture
cargo test --locked lam_relay -- --nocapture
```

Expected: missing process and binding APIs.

- [ ] **Step 3: Implement portable exact-process evidence**

Reuse LAM's proven macOS libproc fields and bounds. The macOS C helper returns PID, parent, UID, microsecond start marker, executable, and boot UUID. Linux reads `/proc/<pid>/stat`, `/proc/<pid>/exe`, `/proc/<pid>/status`, and boot ID. Both paths cap ancestry at 64 hops and validate before and after traversal.

Bind accepts a peer only when `tui.validate()` succeeds and `tui.validate_descendant(peer_pid)` succeeds. Keep only the 64-byte secret in memory; serialize no bindings to disk.

- [ ] **Step 4: Run focused and full cx tests**

Run:

```bash
cargo test --locked process -- --nocapture
cargo test --locked lam_relay -- --nocapture
cargo test --locked
```

Require zero failures and preserve the one existing ignored installed-Codex probe.

- [ ] **Step 5: Commit and push cx**

```bash
git add Cargo.toml Cargo.lock build.rs src/lib.rs src/lam_relay.rs src/process.rs src/macos_process.c
git commit -m "feat(relay): authenticate exact Codex descendants"
git push upstream feat/lam-relay
```

### Task 3: cx app-server relay state machine

**Files:**
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/src/lam_relay.rs`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/src/runtime.rs`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/tests/fake_codex.py`
- Modify: `/home/carraes/projs/cx/.worktrees/lam-relay/tests/runtime.rs`

**Interfaces:**
- `serve(listener, tui_evidence, tx)` accepts bounded relay clients without blocking the TUI relay.
- `RelayControl` carries one validated request plus peer PID and a oneshot response.
- `PendingRelay` maps cx-owned request IDs to Bind, Inspect, or Queue validation state.
- Bind and Inspect issue `thread/read`; Queue issues `thread/queue/add` only after secret validation.
- The TUI receives none of cx's relay-correlated app-server responses.

- [ ] **Step 1: Extend fake Codex and write RED integration tests**

The fake app server must record literal requests and answer:

```json
{"id":"cx.lam.1","result":{"thread":{"id":"THREAD","status":{"type":"idle"},"canAcceptDirectInput":true}}}
{"id":"cx.lam.2","result":{"queuedSubmission":{"id":"RECEIPT","clientUserMessageId":"ATTEMPT","input":[{"type":"text","text":"BODY","text_elements":[]}]}}}
```

The fake app-server spawns a descendant helper that reads `CX_LAM_RELAY` and performs Bind. The integration test then performs Inspect and Queue from the test process with the registered secret. Assert exact thread, attempt, and body in fake app-server events.

Add separate scenarios for wrong secret, wrong thread response, conflicting Bind, missing receipt, wrong client message ID, disconnect before queue write, and disconnect after queue write. Assert `submission=not_started` only for the pre-write cases and `submission=uncertain` for every post-write case.

- [ ] **Step 2: Run the new runtime tests and verify RED**

Run:

```bash
cargo test --locked --test runtime lam_relay -- --nocapture
```

Expected: no relay socket is exported and no cx-owned request state exists.

- [ ] **Step 3: Implement the relay accept loop and correlations**

Create `lam.sock` beside `tui.sock`, set mode `0600`, and set `CX_LAM_RELAY` only on the Codex app-server child. Capture that server's lifetime evidence after it has started, before accepting the first Bind. Keep the existing account control socket and line protocol unchanged.

Use cx-owned IDs `cx.lam.<monotonic u64>`. In the app-server response branch, check `PendingRelay` before `Requests::restore`. Bind stores a secret only after an exact `thread/read`. Inspect returns only `idle` or `active`. Queue builds exactly:

```rust
json!({"id": id, "method": "thread/queue/add", "params": {
    "threadId": thread_id,
    "clientUserMessageId": attempt_id,
    "input": [{"type":"text", "text":text, "text_elements":[]}]
}})
```

Mark the pending operation uncertain immediately before calling `send(&mut writer, ...)`. Correlation mismatches and app-server errors after that point remain uncertain.

- [ ] **Step 4: Verify focused and regression suites**

Run:

```bash
cargo test --locked --test runtime lam_relay -- --nocapture
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

- [ ] **Step 5: Commit and push cx**

```bash
git add src/lam_relay.rs src/runtime.rs tests/fake_codex.py tests/runtime.rs
git commit -m "feat(relay): route LAM messages to Codex"
git push upstream feat/lam-relay
```

### Task 4: LAM Mac Codex enrollment and PostToolUse

**Files:**
- Modify: `cli/src/chat/adapters/binding.rs`
- Modify: `cli/src/chat/adapters/codex.rs`
- Modify: `cli/src/chat/adapters/macos_process.rs`
- Modify: `cli/src/chat/adapters/macos_process.c`
- Modify: `cli/tests/chat.rs`

**Interfaces:**
- Mac `NativeBinding` stores `codex_relay: Option<RelayBinding>` with absolute path and exact cx process evidence.
- `codex_check` on SessionStart enrolls with LAM, then performs cx Bind using the integration secret.
- Mac Codex version parsing accepts 0.155.1 without widening Linux's version set.
- Mac `run_hook` parses the same strict hook input and calls `codex_check`.
- `write_output` preserves the inherited stdout flags or refuses output.

- [ ] **Step 1: Write Mac-only RED tests**

On the Mac build, add tests for:

```rust
assert!(parse_version_for_platform("codex", "codex-cli 0.155.1", Platform::Mac).is_ok());
assert!(parse_version_for_platform("codex", "codex-cli 0.155.1", Platform::Linux).is_err());
```

Use a private fake cx listener to prove SessionStart sends Bind only after LAM enrollment and persists exact cx evidence. Reject relative/symlinked relay paths, a relay owned by a different process, a changed cx lifetime, and a Codex process no longer parented by that cx instance.

Add an isolated descriptor test that captures parent flags, writes one full 8192-byte hook payload under the deadline, and confirms the parent flags remain byte-for-byte equal. Add blocked-reader and empty-output cases.

- [ ] **Step 2: Build on Mac and verify RED**

Copy the current LAM source snapshot, lockfile, and included skill to a private Mac staging directory. Run the exact focused Mac tests with `TMPDIR` under `/private/tmp`. Expected failures: 0.155.1 rejected, Codex hook no-op, missing relay binding, or missing bounded writer. The later acceptance gate uses committed source only.

- [ ] **Step 3: Implement Mac enrollment and bounded hook output**

Keep relay metadata optional for schema compatibility, but require it for Mac Codex queue delivery. On SessionStart:

1. discover Codex 0.155.1 and exact app-server process;
2. read `CX_LAM_RELAY` as a locator only;
3. validate the cx parent process and private socket;
4. enroll with the LAM owner;
5. send cx Bind with the integration secret;
6. retain the relay evidence only after both owners acknowledge the same thread.

If the bounded SessionStart is stopped after step 4, later trusted hooks may resume steps 5–6 only for the same exact process, native ID, session, secret, and relay. This is idempotent recovery, not re-enrollment.

Compile `run_hook` for Linux and macOS. Preserve the existing fail-open exit code and fixed private diagnostics.

- [ ] **Step 4: Verify Mac focused tests and Linux regressions**

Run the focused Mac tests first. On Linux run:

```bash
cargo test --locked chat::adapters::codex chat::adapters::binding -- --nocapture
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

- [ ] **Step 5: Commit and push LAM**

```bash
git add cli/src/chat/adapters/binding.rs cli/src/chat/adapters/codex.rs \
  cli/src/chat/adapters/macos_process.rs cli/src/chat/adapters/macos_process.c cli/tests/chat.rs
git commit -m "feat(chat): enroll Mac Codex through cx"
git push upstream feat/lam-chat
```

### Task 5: LAM Mac idle queue transport

**Files:**
- Modify: `cli/src/chat/adapters/binding.rs`
- Modify: `cli/src/chat/adapters/codex.rs`
- Modify: `cli/src/chat/adapters/mod.rs`
- Modify: `cli/src/chat/daemon.rs`

**Interfaces:**
- `codex_queue_target` returns a platform-specific verified transport target.
- Linux target remains `Direct(UnixStream)`; macOS target is `CxRelay { path, binding, cx_process }`.
- `submit_queue_owned` performs Inspect, calls the existing owner claim closure, then performs Queue.
- `QueueAction::{Claim, Done}` and `run_queue_worker` compile on Linux and macOS.

- [ ] **Step 1: Write RED adapter and owner tests**

Use a private fake relay and real LAM binding records. Assert this order:

```text
Inspect request
owner Claim with returned Idle or Busy event
Queue request containing claimed attempt
owner Finish with Accepted only after matched receipt
```

Add cases for wrong relay PID, expired cx process, wrong secret, Inspect refusal, claim returning None, pre-write error, lost Queue response, explicit uncertain response, wrong receipt, and owner restart. Assert no Queue request exists when claim returns None.

- [ ] **Step 2: Run focused tests and verify RED on Mac**

Run the new platform tests in the Mac staging tree. Expected: the macOS daemon only schedules Claude direct workers and Codex queue helpers are not compiled.

- [ ] **Step 3: Implement the platform transport and enable the owner worker**

Generalize only the cfg gates around the existing queue coordinator. Keep the SQLite state machine unchanged. The Mac adapter maps:

- successful matched Queue response to `Handoff::Accepted`;
- fixed `submission=not_started` response before cx app-server write to `Handoff::NotSubmitted`;
- missing response, `submission=uncertain`, or receipt mismatch to `Handoff::Unknown`.

The relay client validates cx kernel peer PID immediately after connecting and again validates stored process evidence before Queue.

- [ ] **Step 4: Run Mac and Linux full suites**

On each platform run:

```bash
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

On Linux also run `node scripts/chat-probe-check.mjs docs/chat-probes/2026-09-14/report.json`.

- [ ] **Step 5: Commit and push LAM**

```bash
git add cli/src/chat/adapters/binding.rs cli/src/chat/adapters/codex.rs \
  cli/src/chat/adapters/mod.rs cli/src/chat/daemon.rs
git commit -m "feat(chat): wake Mac Codex through cx"
git push upstream feat/lam-chat
```

### Task 6: Cross-repository isolated Mac gate

**Files:**
- Modify: `docs/chat-compatibility.md`
- Create: `docs/chat-probes/2026-09-21/mac-cx-relay/README.md`
- Create bounded JSON evidence files under `docs/chat-probes/2026-09-21/mac-cx-relay/` without bodies or credentials.

**Interfaces:**
- Uses committed cx and LAM heads only.
- Uses private `LAM_CHAT_CONFIG`, `LAM_CHAT_DATA_DIR`, `XDG_DATA_HOME`, and `CODEX_HOME` staging paths.
- Does not modify production services, hooks, account state, or sessions.

- [ ] **Step 1: Build committed heads on the Mac**

Create private staging directories with `mktemp -d`. Copy committed source archives, build with `cargo build --release --locked`, and run full tests, formatting, and Clippy for both repositories. Record commit IDs and binary hashes.

- [ ] **Step 2: Run owned fake-runtime security cases**

Prove wrong-secret, unrelated-process, wrong-thread, pre-write, post-write, and restart behavior through the release binaries. Save only case names, exit states, receipts, timings, and fixed diagnostics.

- [ ] **Step 3: Run owned real Codex cases**

Launch a disposable `cx` session with private Chat paths and the project-scoped hook. Send unique nonce messages from the PC peer:

- while the Mac session runs a tool for at least 20 seconds;
- while the Mac session is idle;
- after restarting only the isolated LAM daemon;
- after disabling only the isolated hook.

Inspect the exact owned thread history to confirm one exposure per expected nonce and zero exposure for the disabled-hook pending case. Run 100 empty PostToolUse hooks and capture zero-byte stdout/stderr plus median, p95, and maximum duration.

- [ ] **Step 4: Record results, commit, and push LAM**

Update compatibility claims only for cases that passed. Commit evidence without message bodies, credentials, auth headers, or account identifiers:

```bash
git add docs/chat-compatibility.md docs/chat-probes/2026-09-21/mac-cx-relay
git commit -m "docs(chat): record Mac cx relay gate"
git push upstream feat/lam-chat
```

### Task 7: Scoped production rollout and acceptance

**Files:**
- Mac: `/Users/carraesmb/.local/bin/cx`
- Mac: `/Users/carraesmb/.local/bin/lam`
- Mac: `/Users/carraesmb/Library/LaunchAgents/dev.lam.chat.plist`
- Mac project: `/Users/carraesmb/mondrio/mondrio-platform/.codex/hooks.json`
- Modify after verification: `docs/chat-compatibility.md`

**Interfaces:**
- Installs only tested release binaries with named backups and atomic sibling renames.
- Enables native bindings only in the existing Mac LAM launch agent.
- Installs only the exact reviewed project hook template.

- [ ] **Step 1: Snapshot production state**

Record hashes, resolved paths, permissions, launch-agent contents, cloud-config hash, cx account-state hash, hook presence or hash, running service PID, and peer status. Do not print file contents containing credentials.

- [ ] **Step 2: Stage and smoke-test both binaries**

Copy the release binaries to sibling staging paths. Verify hashes and run `cx --version`, `lam --help`, `lam chat --help`, and `lam --llm` from staged paths before replacement.

- [ ] **Step 3: Install recoverably**

Create named backups containing the pre-install short hash. Atomically rename the staged binaries. Back up the launch agent and hook target. Update the launch agent to:

```text
/Users/carraesmb/.local/bin/lam chat serve --foreground --native-bindings
```

Install the exact Codex hook via `lam chat setup --client codex --root /Users/carraesmb/mondrio/mondrio-platform --apply`. Restart only `dev.lam.chat`.

- [ ] **Step 4: Start new owned sessions and run acceptance**

Leave existing sessions running on herdr. Start disposable new `cx` sessions and repeat one busy, one idle, one reply, and one PC/Mac cross-peer case. Confirm `lam chat status --json` reports the experimental native validator and the PC peer connected. Confirm the TUI Chat tab updates without refresh.

- [ ] **Step 5: Verify preserved state and document rollback**

Require the ordinary cloud-config and cx account-state hashes to equal their snapshots. Verify old user sessions were not restarted. Record installed commit hashes, binary hashes, service status, hook path, limitations, and exact backup paths in `docs/chat-compatibility.md`.

- [ ] **Step 6: Commit and push final rollout evidence**

```bash
git add docs/chat-compatibility.md
git commit -m "docs(chat): record Mac cx production rollout"
git push upstream feat/lam-chat
```

## Final verification

Before claiming completion:

1. Run full test, format, Clippy, and release-build gates in both repositories on Linux and Mac.
2. Re-run the original Mac symptom: a new cx-launched Codex session must make `lam chat sessions` succeed and appear in the exact mapped project.
3. Re-run busy, idle, peer reconnect, and empty-hook acceptance from Task 7.
4. Inspect both git worktrees for uncommitted changes and verify every commit is on its upstream branch.
5. Confirm production cloud config and cx account state retained their original hashes.
