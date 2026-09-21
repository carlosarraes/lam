# Owned macOS Codex cx relay gate

This gate used committed LAM `6bfeeac`, committed cx `ca97474`, and Codex CLI 0.155.1 on arm64 macOS. It ran under `/private/tmp/lam-cx-gate.vYoif1` with private Chat config/data, `CODEX_HOME`, cx state, project hooks, and release binaries. The project hook was trusted through Codex's normal review UI. No installed binary, production service, shared hook, cloud config, cx account state, or existing agent session changed.

The Mac release SHA-256 values were:

- LAM: `466633ec859fb6b8f70990992720d7cd0f96b9b274cd15b2b4d883bd621d8dfa`
- cx: `a466e852651e7d6cf0650da4216f0b1faf671f012219946954b7561495df0f77`
- reviewed hook: `426fb4e4808fb44893020227a27b9cbe0fb45530eb2c84517624fb59cd547025`

The LAM Mac suite passed 226 unit tests and 88 integration and doc tests with zero failures. The cx suite passed 58 tests, with one pre-existing installed-Codex probe ignored. Both passed `cargo fmt --check`, Clippy with warnings denied, and locked release builds. The same commits passed their Linux test, format, and Clippy gates.

The real Codex cases used one owned `screen` session at a time:

- Busy: the receiver ran one 45-second tool. LAM observed the turn as busy before the send. The message remained queued with no attempt during the tool, then appeared through PostToolUse in the same turn. The receipt ended Unknown because native acceptance cannot be inferred from hook output.
- Idle: cx issued `thread/queue/add`, matched the returned thread, attempt, and native receipt, and LAM recorded Accepted. The exact owned Codex thread displayed the peer payload.
- Daemon restart: only the isolated LAM daemon restarted. It reloaded the binding and observation, then a new idle message received a matched native receipt and appeared in the same owned thread.
- Startup: the final reviewed hook used a three-second process timeout around LAM's two-second internal deadline. The startup capture contained zero `Hook failed` lines. The session name came from `screen`; no `LAM_NAME` override was set.

The gate does not claim macOS Claude or Pi delivery, support outside Codex 0.155.1, acceptance for busy hook delivery, or immunity to arbitrary same-user compromise. cx exposes only versioned Bind, Inspect, and Queue requests on a private 0600 Unix socket. It checks the exact app-server descendant, process lifetime, peer PID, secret, thread, attempt, and native receipt. LAM remains the sole owner of durable messages and handoff state.

[run.json](run.json) contains the bounded case records. It omits message bodies, credentials, auth files, account identifiers, and native transcript content.
