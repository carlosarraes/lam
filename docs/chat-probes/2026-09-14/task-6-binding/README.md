# Task 6 native binding checkpoint

This is an identity feasibility check, not a production adapter acceptance report. The only production change at this checkpoint is the required Codex serialization helper and regression. Native participant authentication remains disabled.

`root.json` and `child.json` are the complete whitelisted records from one owned Codex CLI 0.154.0 `exec --json` session on 2026-09-14. Both commands succeeded and the native root and child finished normally. `capture.mjs` produced the records. It reads only the native thread environment variable, boot identity and process ancestry through the first Codex executable. It never reads a transcript or dumps the environment.

Both commands descend directly from PID 2173536 with start counter 79385708 in the same boot. They receive distinct `CODEX_THREAD_ID` values. This establishes that process ancestry alone cannot separate the root and child. It does not establish whether an actual trusted hook receives another usable native association.

The [official hook reference](https://learn.chatgpt.com/docs/hooks) says ordinary subagent hooks carry the parent's `session_id`, and ordinary PostToolUse fields do not document an `agent_id`. The approved follow-up below found that actual child PostToolUse does include `agent_id` on the current shared server. Using only session_id would still risk routing a parent's batch into a child's context. For sending, a private integration-issued credential and actual native evidence still need to be proved together; native IDs can locate those private records but cannot alone authenticate a command.

## Executed invocation

Cwd was `/home/carraes/projs/lam/.worktrees/lam-chat`. The script and records were in `/tmp/lam-chat-task6-topology.TwJPU1`.

```sh
codex exec --json 'This is an owned LAM native binding topology test. You are a test subject, not an implementation agent. Do not read project files, skills, configuration, transcripts, or environment variables yourself. Do not edit code or change permissions, trust, hooks, or settings. First run exactly node /tmp/lam-chat-task6-topology.TwJPU1/capture.mjs root using your shell tool. Then create exactly one native child agent as an owned test subject with this exact bounded task: Run exactly node /tmp/lam-chat-task6-topology.TwJPU1/capture.mjs child using your shell tool, do not inspect anything else or change files, permissions, hooks, or settings; report the command result and finish. Wait for that child to finish, then report the root thread ID and child ID if available and exit normally. This explicit test authorizes that single native child test subject only, no implementation delegation. The capture script reads only whitelisted identity and process ancestry values and writes its owned temporary evidence files. If any tool is denied, preserve the denial and stop.'
```

No model, permissions, rules, hook or trust overrides were supplied. No trust prompt appeared. The existing experimental-feature warning was left unchanged. Root native thread was `01a09ef7-fbdb-7391-8154-f2ba97601ac9` and native child was `01a09ef8-25fc-77b1-8f20-39fc5d16f811`. Native exec exit was 0. Read-only cleanup checks found root PID 2173536 and command PIDs 2174177/2174427 absent. No signal or terminal interaction was used.

## Approved follow-up

`hook-identity.mjs` and `proposed-hooks.json` define a diagnostic only. After explicit approval for the corrected hashes below, the exact config was activated in the owned cwd and the four project handlers were reviewed/trusted through the normal native UI. The run produced five records in [hooks.jsonl](hooks.jsonl). The active definition was removed after both owned clients exited normally. The fixture emits zero stdout/stderr and never outputs context or changes a tool decision. It records only event, stdin `session_id`/`agent_id`, environment `CODEX_THREAD_ID`, boot identity and its own process ancestry. It does not retain tool inputs, responses, credentials, transcript paths or contents.

After validating the owned cwd and allowed event, operational failures append a separate private `hook-errors.jsonl` record containing only the allowlisted event, `status: "error"`, and `stage: "process"` or `"record"`. Each record is under 128 bytes. The diagnostic file is opened without following symlinks and must be a mode-0600 regular file owned by the current UID with one hard link. Exception text, identifiers and input fields are not copied into errors. Malformed JSON and out-of-scope input are intentionally unlogged because they cannot pass the cwd/event check. If diagnostic storage itself fails, the fixture stays silent and missing output remains inconclusive; absence does not prove the hook never ran.

The proposed cwd is the previously owned `/tmp/lam-chat-gate.Bx9ROj`. Read-only checks confirmed its active `.codex/hooks.json` and `codex-pending.json` were absent before this proposal. Recheck before any run. The old Task 1 hook approvals do not authorize the new fixture.

The proposed events are SessionStart for root enrollment evidence, SubagentStart for the native child ID, Bash PostToolUse for each root/child command's hook association, and SubagentStop for child completion association. Each uses the exact command `node /tmp/lam-chat-task6-topology.TwJPU1/hook-identity.mjs` and a two-second timeout. No additionalContext limit is needed because this fixture emits no model content. It does not test inline delivery or solve the separate output-spill gate.

Approval covered activating the exact proposed config at `/tmp/lam-chat-gate.Bx9ROj/.codex/hooks.json` and reviewing/trusting those handlers through the ordinary native hooks UI in an owned client. No trust bypass, direct trust-record write, shared config edit or permission change occurred. No folder trust prompt appeared. The previously inactive PostToolUse handler was enabled; the other three became active after trust. The existing unrelated SessionStart handler was left unchanged. Both owned clients exited normally, and only the newly owned active hook definition was removed afterward.

| File in `/tmp/lam-chat-task6-topology.TwJPU1` | SHA-256 |
| --- | --- |
| `capture.mjs` | `59ea77c53e4ffe0f58948d4bd09229e8063bbcc8bac59ff36064ae807e4bd758` |
| `hook-identity.mjs` | `81d8e98480a8bf32b2bcb028b9271f479e0302e75f3cd75a9c638fb6d819d5ef` |
| `proposed-hooks.json` | `abd111c81b64342d301d5586a0831e97a558be9cb12956f813d2de049129d513` |

The committed copies have identical bytes. Any edit changes the scope that must be reviewed.

The hook script hash changed during checkpoint review. The earlier proposal was withdrawn and did not authorize this version. A later explicit approval covered these corrected bytes. Any future script/config changes need their own applicable trust review.

### Observed association

The frontend reported Codex CLI 0.154.0, but the hook and tool ancestry identified the pre-existing executing server as release 0.153.4, PID 28534/start counter 76781. That shared server was neither restarted nor changed. These records characterize that combination, not standalone 0.154.0 execution.

| Hook | session_id | agent_id | CODEX_THREAD_ID |
| --- | --- | --- | --- |
| Root SessionStart | root thread | null | null |
| Root PostToolUse | root thread | null | null |
| SubagentStart | root thread | child thread | null |
| Child PostToolUse | root thread | child thread | null |
| SubagentStop | root thread | child thread | null |

Root thread was `01a09fdd-adb1-7271-9e56-3c88181ba077`; child thread was `01a09fdd-e5bd-71d3-bf43-c934c522ba7b`. The root command received its own native thread variable at PID 2214541, and the child command received the child's at PID 2214720. Both had parent PID 28534. All five hooks likewise descended from that shared server. No diagnostic error file was created.

This is positive evidence of root/child hook discrimination via actual `agent_id`, beyond the documented table. It does not require a parent-disabled policy, separate-process restriction or queue-only fallback. A practical binding may use these native IDs to locate private integration-issued records, then validate credential, version and live native process evidence under the stated same-UID trust boundary. Credential issuance/propagation and the production Rust path are still unproved.

[association-run.json](association-run.json) retains exact setup/probe argument arrays, native IDs, command results, trust actions and cleanup. Setup PTY 66687 and probe PTY 9296 exited zero after `/quit`. The child completed and the root reported PROBE_DONE before quitting. All owned command/hook PIDs were absent on cleanup; shared PID 28534 remained alive. No transcript was read. No LAM message, inline payload or production credential was sent.

## Direct diagnostic tests

Run `node --test docs/chat-probes/2026-09-14/task-6-binding/hook-identity.test.mjs` from the repository root. Seven tests passed. The tests launch the actual script in Node subprocesses with a test-only filesystem adapter, synthetic process evidence and output redirected to fresh owned temporary directories. They exercise success, forced process-read and record-write failures, malformed/out-of-scope input, preserving earlier diagnostics, symlink refusal and unavailable diagnostic storage. Every subprocess exits zero with empty stdout/stderr. Error records contain only the fixed fields above. Test directories are removed afterward.

Before the fix, the two forced operational failure tests failed because `hook-errors.jsonl` was missing; afterward they passed. These are direct isolated fixture tests, not native hook execution, credential proof or delivery evidence. `hook-test-loader.mjs` is test-only and is not referenced by the proposed hook configuration.

## Options still open

Separate top-level processes provide distinct process lifetimes, which could help bind commands if credential issuance and actual hook association also pass native tests. Restricting participants to those processes would be a product choice, and would still require refusing or distinguishing their child hooks before delivering a parent batch.

Shared-process participants need an actual thread-specific association in addition to process lifetime. The observed child hook agent_id supplies a candidate association on the tested executing server. Command credential binding still needs implementation and native proof.

Queue-only Codex delivery could avoid routing inline batches through ambiguous tool hooks, but would give up the required busy inline path and still would not settle sender command authentication. A mandatory launcher would change automatic startup expectations and would not alone distinguish native children sharing its process. No option has been selected or implemented.
