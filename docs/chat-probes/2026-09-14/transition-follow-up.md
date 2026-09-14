# Approved transition and refusal follow-up

Claude Code 2.1.270 and Codex CLI 0.154.0, Linux, 2026-09-14. This supplements the initial blocked checkpoint without replacing its evidence. Exact startup commands, argv, effective cwd, and separate post-startup prompt frames are in [transition-invocations.json](transition-invocations.json). No peer payload was delivered through PTY input.

## Claude refusal

Started the refusing receiver from the already-trusted worktree with `--settings '{"crossSessionInbound":"refuse"}'`. After its READY response, started a separate owned native sender directed only to that unique receiver name. Its installed permission classifier allowed exactly one native SendMessage. No refusal setting was relaxed.

The [sender transcript](claude-native-refusal.jsonl) records native submission success at 05:18:58.325 with message ID `52d69cef-c8f9-40ca-a3cb-3fa3873dc3d2`. A separate native delivery notice at 05:19:05.522 says the recipient refused it and it was not delivered to that session's Claude. The [receiver trace](claude-native-refusal-check.json) independently records `refused inbound peer message (uds: dropped before attachment materialization)` at 05:18:58.321. The receiver stayed idle with no peer attachment. Its process-owned socket matches the refusal notice.

Thus submission success is not receiver acceptance. Earlier raw-helper socket frames still have unknown outcomes; this experiment does not retroactively turn missing callbacks into rejection evidence. The sender model's commentary is observation, not a protocol specification. Both owned Claude sessions exited normally with code 0 through `/exit`; the receiver socket was absent afterwards.

## Codex trust and exact scope

Restored the original owned `.codex/hooks.json` with the already-approved PostToolUse hook and newly approved silent Stop observer. [transition-events.json](transition-events.json) preserves the exact definition. Startup said one new/changed hook required review. Chose Review hooks, opened only Stop, verified command `node /tmp/lam-chat-gate.Bx9ROj/stop-hook.mjs`, sync mode, timeout 2 seconds, and pressed `t` in that hook's detail view. This implements the user's approval of `p94n7`, not a trust bypass. Reenabled the original trusted Bash PostToolUse through its detail view. Both showed installed=1, active=1. Existing unrelated SessionStart and permission defaults were untouched.

The original hook sources were unchanged:

```text
567b41d2ad7e35aca54b104c6575486e712defcac95de9fbf9027dbf79f46d94  fixtures/hook.mjs
821f50e335c3a15c01415fe77e198e37884b447435aaadc8d3d4bca515ab5b7d  fixtures/stop-hook.mjs
```

## Native transitions and ownership

All runs used native thread `01a09e5b-15b7-77f0-9829-e09b5fb61c86`. [Selected transcript](codex-transitions.jsonl) retains the actual native roles, tool results, and task start/complete events. [Coordinator/tool/hook events](transition-events.json) are separate evidence: hook stdout is not a native acceptance receipt.

| Case | Ordered native observations | Handoff and receipt |
| --- | --- | --- |
| Busy to idle | Tool completed 20,009 ms; PostToolUse empty at 05:21:53.401; pending written 2 ms later | Silent Stop claimed it and queue returned native ID at 05:21:57.490. Original turn finished NONE, then a new native turn delivered `CODEX_STOP_P8R6` without another human prompt |
| Idle to busy, queue wins | Coordinator sampled native empty Stop; new authorized tool became ready; pending written and claimed while the tool ran | Queue returned native ID; later PostToolUse and Stop saw no pending file. Tool completed 20,023 ms. One queued user message, then one `CODEX_ITB_QUEUE_K3V8` response |
| Idle to busy, hook wins | Coordinator sampled native empty Stop; new tool became ready; pending written; idle contender deliberately waited until PostToolUse claimed | Tool completed 20,023 ms; one developer-context exposure. Idle contender lost the shared claim and did not queue. One `CODEX_ITB_HOOK_F7N2` response |

The fixture [race-coordinator.mjs](fixtures/race-coordinator.mjs) waits for recorded native state observations and shares the exact pending pathname with both original hooks. Ownership uses rename of that one pending file. Two separate idle contenders are also tested against one actual temporary inbox. The live runs exercise both ordered handoff outcomes, not every simultaneous scheduling interleaving. Original hooks read before rename and lack production error/recovery handling; do not ship them as an adapter. Stop emits no stdout, block, or reason. Its queue call supplies the same fixed restrictive wrapper and serialized untrusted object as the other paths.

The lifecycle event is a wake/recheck trigger, not a durable assertion that the session is still idle when a later sender acts. Native queue handles the stale-idle case without interrupting the newly running tool. Production code still needs shared durable ownership, guarded state updates, unknown-outcome handling, opt-out checks, and lease/crash tests. No native idempotency or receipt-reconciliation API was demonstrated.

## Cleanup and verification

After the final native task_complete and empty Stop, disabled only the two owned hooks through `/hooks`; the overview showed installed=1, active=0 for each. Codex `/quit` exited 0. The config was moved recoverably to `.codex/hooks.transitions-disabled.json`; active hooks.json and codex-pending.json are absent. Original scripts and all claim/evidence files remain. No shared daemon was stopped.

Run covering checks from the worktree:

```sh
node --test scripts/chat-probe.test.mjs scripts/chat-race-probe.test.mjs
node scripts/chat-probe-check.mjs docs/chat-probes/2026-09-14/report.json
git diff --check
```

The evidence assertions check the long-tool timings, single native exposure and model receipt for each nonce, empty-check/arrival/Stop order, both idle-claim outcomes, native refusal result/notice/receiver trace, and inactive cleanup paths. They validate this recorded run, not future client versions.
