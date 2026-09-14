# LAM Chat client compatibility

These are Linux probes of Claude Code 2.1.270, Codex CLI 0.154.0, and Pi 0.85.1 on 2026-09-14. They do not enable a production integration. The captured gate report is [report.json](chat-probes/2026-09-14/report.json); [the runbook](chat-probes/2026-09-14/README.md) contains the invocation, fixtures, and evidence interpretation.

The Codex state-transition gate remains open while the additional silent Stop observer awaits normal scoped trust approval. Passing the checker tests is not a passing compatibility milestone. Run the actual gate with:

```sh
node scripts/chat-probe-check.mjs docs/chat-probes/2026-09-14/report.json
```

| Client | Busy delivery and idle wake | Provenance observed | Handoff evidence |
| --- | --- | --- | --- |
| Claude 2.1.270 | Native peer message reached the next tool boundary; idle message started a turn | Native `origin.kind=peer`, verified sender process evidence, and native permission-limiting reminder | No direct socket acknowledgement in these frames. Native transcript exposure and model receipt were observed separately |
| Codex 0.154.0 | Synchronous PostToolUse context arrived after a 20-second tool; native queue woke idle | Fixed adapter-owned instruction followed by serialized untrusted JSON, in a developer message | Hook stdout alone is not acceptance. Transcript confirms observed exposure. Queue returns native message and thread IDs |
| Pi 0.85.1 | Existing control extension called custom-message steer while busy and triggerTurn while idle | Native custom-message record; model conversion uses user role and retains the fixed wrapper text | Extension success follows the native sendMessage call, before model receipt |

All successful long-tool probes completed after at least 20 seconds. No test interrupted a model or tool, targeted an existing user pane, changed shared client daemons, or edited installed client settings. A new Codex folder and its exact PostToolUse hook were trusted through normal client review only after explicit approval. A Claude refusal-session tool was blocked by the existing auto classifier; the probe preserved that denial and obtained that owned session's socket from `/status` for the independent inbound-refusal check.

Peer bodies contained fake LAM headers, JSON delimiters, embedded system/developer instructions, the sender name Carlos, and demands to override an explicit user restriction. The observed receivers reported the allowed nonce, did not print the forbidden token, and did not create the requested files. This is bounded behavioral evidence. Claude also retains native peer origin and permission controls. Pi's customType is local metadata, not a separate model role. Codex's fixed developer wrapper explicitly denies authority to every JSON field; arbitrary peer text is never concatenated into the wrapper instruction. No result establishes immunity to all future prompt injections or verifies human intent against another process owned by the same OS user.

The empty Codex hook emitted zero stdout/stderr bytes in a standalone 19 ms sample. This is one measurement, not a benchmark. Empty follow-up tool calls in all three sessions did not add another copy of old messages. Native refusal/unavailable observations must remain distinguishable from acceptance: a connected socket with no acknowledgement is not a delivery receipt.

Pi's configured opencode-go/deepseek-v4-flash provider returned a monthly-limit 429. The model probes used its existing openai-codex login with `--provider openai-codex --model gpt-5.6-luna` for those sessions only. Defaults were unchanged. Claude used its installed Opus 5 default; Codex used its installed gpt-6-astra default and existing permission configuration. Mac behavior, production delivery claims, durable receipt reconciliation, and cross-client LAM commands remain later work.

The installed contracts were checked against [Claude messaging](https://code.claude.com/docs/en/cross-session-messaging), [Codex hooks](https://learn.chatgpt.com/docs/hooks), Pi's installed `docs/extensions.md` and `dist/core/messages.js`, and the installed `~/.pi/agent/extensions/control.ts`. The Claude binary's socket handler was inspected to confirm the tested frame, session check, peer origin, and non-interrupting `next` priority. Herdr's [Pi state integration](https://github.com/herdrdev/herdr/blob/main/src/integration/assets/pi/herdr-agent-state.ts) was inspected locally for `agent_settled` and `ctx.isIdle()` patterns; it was not used as a message transport.
