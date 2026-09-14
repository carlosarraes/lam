# Task 6 native binding checkpoint

This is an identity feasibility check, not a production adapter acceptance report. The only production change at this checkpoint is the required Codex serialization helper and regression. Native participant authentication remains disabled.

`root.json` and `child.json` are the complete whitelisted records from one owned Codex CLI 0.154.0 `exec --json` session on 2026-09-14. Both commands succeeded and the native root and child finished normally. `capture.mjs` produced the records. It reads only the native thread environment variable, boot identity and process ancestry through the first Codex executable. It never reads a transcript or dumps the environment.

Both commands descend directly from PID 2173536 with start counter 79385708 in the same boot. They receive distinct `CODEX_THREAD_ID` values. This establishes that process ancestry alone cannot separate the root and child. It does not establish whether an actual trusted hook receives another usable native association.

The [official hook reference](https://learn.chatgpt.com/docs/hooks) says ordinary subagent hooks carry the parent's `session_id`, and ordinary PostToolUse fields do not document an `agent_id`. Using only that input to claim pending messages could route a parent's batch into a child's context. For sending, selecting a token solely through an inherited native ID would not independently validate the exact command thread. A private integration-issued credential and actual native evidence still need to be proved together.

## Executed invocation

Cwd was `/home/carraes/projs/lam/.worktrees/lam-chat`. The script and records were in `/tmp/lam-chat-task6-topology.TwJPU1`.

```sh
codex exec --json 'This is an owned LAM native binding topology test. You are a test subject, not an implementation agent. Do not read project files, skills, configuration, transcripts, or environment variables yourself. Do not edit code or change permissions, trust, hooks, or settings. First run exactly node /tmp/lam-chat-task6-topology.TwJPU1/capture.mjs root using your shell tool. Then create exactly one native child agent as an owned test subject with this exact bounded task: Run exactly node /tmp/lam-chat-task6-topology.TwJPU1/capture.mjs child using your shell tool, do not inspect anything else or change files, permissions, hooks, or settings; report the command result and finish. Wait for that child to finish, then report the root thread ID and child ID if available and exit normally. This explicit test authorizes that single native child test subject only, no implementation delegation. The capture script reads only whitelisted identity and process ancestry values and writes its owned temporary evidence files. If any tool is denied, preserve the denial and stop.'
```

No model, permissions, rules, hook or trust overrides were supplied. No trust prompt appeared. The existing experimental-feature warning was left unchanged. Root native thread was `01a09ef7-fbdb-7391-8154-f2ba97601ac9` and native child was `01a09ef8-25fc-77b1-8f20-39fc5d16f811`. Native exec exit was 0. Read-only cleanup checks found root PID 2173536 and command PIDs 2174177/2174427 absent. No signal or terminal interaction was used.

## Prepared follow-up, not enabled or run

`hook-identity.mjs` and `proposed-hooks.json` are a proposed diagnostic only. They are retained in the owned topology directory, not in an active `.codex/hooks.json` path. There is no `hooks.jsonl` output because the fixture has not been run as a hook. The fixture emits zero stdout/stderr and never outputs context or changes a tool decision. It records only event, stdin `session_id`/`agent_id`, environment `CODEX_THREAD_ID`, boot identity and its own process ancestry. It does not retain tool inputs, responses, credentials, transcript paths or contents.

The proposed cwd is the previously owned `/tmp/lam-chat-gate.Bx9ROj`. Read-only checks confirmed its active `.codex/hooks.json` and `codex-pending.json` were absent before this proposal. Recheck before any run. The old Task 1 hook approvals do not authorize the new fixture.

The proposed events are SessionStart for root enrollment evidence, SubagentStart for the native child ID, Bash PostToolUse for each root/child command's hook association, and SubagentStop for child completion association. Each uses the exact command `node /tmp/lam-chat-task6-topology.TwJPU1/hook-identity.mjs` and a two-second timeout. No additionalContext limit is needed because this fixture emits no model content. It does not test inline delivery or solve the separate output-spill gate.

To run it, approval must cover activating the exact proposed config at `/tmp/lam-chat-gate.Bx9ROj/.codex/hooks.json` and reviewing/trusting those new handlers through the ordinary native `/hooks` flow in an owned client. No trust bypass, direct trust-record write, shared config edit or permission change is part of the proposal. If folder trust is requested again, that normal action also needs scoped approval. Afterward remove only the newly owned active hook definition and close the owned session normally, retaining evidence here. None of these activation steps has happened.

| File in `/tmp/lam-chat-task6-topology.TwJPU1` | SHA-256 |
| --- | --- |
| `capture.mjs` | `59ea77c53e4ffe0f58948d4bd09229e8063bbcc8bac59ff36064ae807e4bd758` |
| `hook-identity.mjs` | `a9b5f6fd726b9a9911294fdacd5300ea8913cafe9c5f5aec22f12a3ccdd7f877` |
| `proposed-hooks.json` | `abd111c81b64342d301d5586a0831e97a558be9cb12956f813d2de049129d513` |

The committed copies have identical bytes. Any edit changes the scope that must be reviewed.

## Options still open

Separate top-level processes provide distinct process lifetimes, which could help bind commands if credential issuance and actual hook association also pass native tests. Restricting participants to those processes would be a product choice, and would still require refusing or distinguishing their child hooks before delivering a parent batch.

Shared-process participants need an actual thread-specific association in addition to process lifetime. The unrun hook probe may reveal usable native evidence. The current records neither prove nor disprove that possibility.

Queue-only Codex delivery could avoid routing inline batches through ambiguous tool hooks, but would give up the required busy inline path and still would not settle sender command authentication. A mandatory launcher would change automatic startup expectations and would not alone distinguish native children sharing its process. No option has been selected or implemented.
