# Owned six-pair native Chat matrix, 2026-09-20

This checkpoint used the candidate release binary with one private SQLite store and three owned interactive sessions in `/tmp/lam-chat-task6-native.KlGYDF/claude-case-20260920/project`. The Codex frontend was 0.154.0 on the unchanged 0.153.4 app-server; Claude was 2.1.278 and Pi was 0.86.1. No global hook, extension, daemon, or permission default was changed. The daemon alone ran with experimental `--native-bindings`.

Every row below was an actual `lam chat send` from the sender's native Bash tool (the last two rows were one multi-recipient command). Stored sender incarnations, frozen recipient IDs and full body were checked in the private observer store; the exact receiver's native transcript independently contained the full attributed LAM wrapper and nonce. These are native handoff and transcript observations, not proof the recipient agreed or acted.

| Sender → receiver | Message ID | Nonce | Persisted transport state |
| --- | --- | --- | --- |
| Claude → Pi | `d9a5eb19-1210-463c-8aa3-9c99e40137e8` | `CLAUDE_TO_PI_IDLE_SOCKETFIX` | Accepted after Pi's `sendMessage` call returned |
| Pi → Claude | `83d13dbb-99a9-4b05-951e-942a80b3e64e` | `PI_TO_CLAUDE_IDLE_418D35E` | Unknown: Claude socket gives no native receipt |
| Pi → Codex | `e2b40a47-9212-46db-b239-c0c5701e3ca2` | `PI_TO_CODEX_IDLE_418D35E` | Accepted with exact thread/message queue IDs |
| Claude → Codex | `792c1228-41e4-4d83-9c50-67194109a896` | `CLAUDE_TO_CODEX_IDLE_418D35E` | Accepted with exact thread/message queue IDs |
| Codex → Pi | `23c8c805-bc4f-468d-bf4c-c8c005afd5e4` | `CODEX_TO_PI_AND_CLAUDE_418D35E` | Accepted after Pi's native call returned |
| Codex → Claude | same frozen multi-recipient message | same nonce | Unknown; Claude native queue operation observed |

Codex then used `lam chat reply` to Pi's `e2b40a47-...` message. The reply `6c7c492f-4a57-4659-b7a5-1ef3dba251a6` stores that exact parent ID, has one Pi recipient and nonce `CODEX_REPLY_TO_PI_418D35E`, and Pi's native custom-message record contains it. No automatic acknowledgement loop was installed.

The private daemon was then terminated normally and restarted with the same store while all three native clients remained open. Pi's event bridge reconnected under a new child PID. Pi sent `PI_TO_CODEX_AFTER_DAEMON_RESTART_920` as message `bdbad26a-4e4d-4cae-a066-667e1add78c6`; Codex's previously persisted idle observation woke the exact thread without another Stop hook. The Store recorded Accepted with queue message `01a0c021-d172-7452-9a87-f0842e65d45b`, and Codex's exact-thread transcript recorded the full peer wrapper at 18:43:57.074 UTC. The accompanying red/green unit tests show that a persisted Busy observation cannot be promoted to Idle by restart.

Pi's earlier [busy case](../task-6-pi/README.md) and Claude's [busy case](../task-6-claude-delivery/README.md) were also non-interrupting. Claude's silent inbound-refusal behavior remains an unresolved product gate; socket submission is deliberately Unknown, never Accepted or automatically retried. This matrix closes the six directed local Linux pair check, but does not close refusal, all scheduler races, lifecycle rotation, macOS, SSH, or unattended rollout.
