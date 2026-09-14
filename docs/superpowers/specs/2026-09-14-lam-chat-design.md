# LAM Chat design

Date: 2026-09-14

Status: approved on 2026-09-14; implementation not started.

## Decision summary

Add terminal-first, addressed coordination between Claude Code, Codex, Pi, and Carlos. `lam chat` is an observable feed and composer, not a room whose entire history enters every agent's context. Agents can message several recipients at once. Carlos can address `@pm`; only that recipient receives the message.

Deliver short messages inline at the next supported tool boundary, without cancelling a model request or tool. Wake idle sessions through their supported input mechanism. Batch several small messages. Use `lam inbox` for explicit history, recovery, and overflow, not as a mandatory fetch for ordinary messages.

Run a local LAM process and SQLite store on each participating machine. Connect the PC and Mac through one persistent bidirectional SSH channel. Chat does not need the existing Cloudflare backend, D1, R2, or Android push. Existing requests, FYIs, articles, and phone behavior remain unchanged.

This changes the older Android spec's product-wide exclusion of messaging, but does not turn attention items or their final answers into conversation threads. See the [domain glossary](../../../CONTEXT.md).

## Scope

Version 1 includes:

- One-time integration setup, automatic registration thereafter, and per-session opt-out.
- Explicit recipients, fuzzy mention completion, multiple recipients, and explicit project-scoped `@all`.
- Replies to the sender by default, with explicit reply-all.
- Sender attribution, stable session identity, persistent messages, per-recipient receipts, and visible failures.
- Inline-first delivery without interruption and silent empty checks.
- A terminal observer feed, a composer, and agent-facing commands/help.
- Linux PC and macOS operation, including reconnect and offline queuing between them.

Not included: Android Chat, internet-hosted Chat, public listeners, rooms/channels, file attachments, executing message bodies as shell commands, urgent interrupts, model orchestration, or a second agent-to-human notification system. Long reports continue to use Articles. Agents needing Carlos's approval, decision, credentials, or attention continue to use normal LAM items.

## Existing code and intended seams

The CLI is Rust with Clap, Ratatui, and Crossterm. `cli/src/main.rs` dispatches commands; `cli/src/name.rs` resolves agent display names; `cli/src/herdr.rs` reports attention state to the current Herdr pane. `cli/src/client.rs` accesses the existing remote attention backend. `lam --llm` embeds `skill/lam/SKILL.md` at build time.

Keep the Chat module independent of `client.rs` and its cloud credentials. The existing `Config` requires server/token/topic and rewrites known fields on save, so Chat gets separate configuration and state rather than making local messaging depend on cloud setup or risking loss of new fields through `lam init`.

The Chat module exposes a small interface for registration, addressed sends/replies, inbox reads, receipts, and feed subscriptions. Its implementation owns persistence, routing, bounds, and delivery coordination. Client-specific adapters sit at an internal seam for capability discovery, safe delivery, and idle wake. The terminal UI does not implement delivery retries or talk to client sockets itself.

Reuse existing name resolution and terminal conventions. Do not rewrite the Requests/History/Articles TUI, move cloud modules, or require Herdr. Add a separate `lam chat` terminal view. Herdr can supply names and pane metadata, but must not inject keystrokes or become the message transport.

## Architecture

```text
agent CLI / human composer                 other machine
            |                                   |
            v                                   v
    local Chat module <==== SSH ====> remote Chat module
    registry + SQLite                     registry + SQLite
       |          |
       |          +---- observer feed ----> lam chat
       v
    recipient adapter
       +-- Claude native peer socket
       +-- Pi extension
       +-- Codex tool hook / idle queue
```

The local process is the only database writer. Hooks and CLI commands use a private local socket; they do not contend over database transactions or make network requests to the other machine. Feed subscriptions are event-driven. Keep hooks bounded and fail-open with respect to ongoing agent work: a missing LAM process cannot block or cancel a tool. Report operational errors through diagnostics/feed state, not repeated model-context warnings.

Use per-user directories with private permissions and a single-instance lock. On Linux, configuration lives under the existing LAM config directory as `chat.toml`; local Chat state lives under the platform data directory. Use the equivalent platform directories on macOS. Keep credentials out of transcripts, process arguments, and routine diagnostics. Do not reuse the cloud master token. Foreground operation is required for diagnosis; user-service installation is an explicit setup action, never a side effect of reading help or opening the view.

## Identity and routing

Resolve display names using existing precedence: explicit name, `LAM_NAME`, then the current multiplexer context. Herdr currently provides workspace:tab labels. When no name can be inferred, leave the session unregistered with a useful setup diagnostic rather than taking another session's name.

Route to a stable machine ID plus session-incarnation ID, not a display name, PID alone, or terminal title. Store native client identity, process-start evidence where available, project association, display name, client kind/version, and delivery capabilities. Validate client-specific identity; children may inherit another client's environment variables. A reused socket path is not sufficient evidence of the same recipient.

Registration is automatic through the installed client integration. Setup must preserve unrelated configuration and require normal client trust review. Opt-out unregisters delivery eligibility without deleting history. A process restart creates a new incarnation even if its native conversation or display name is reused. Temporary adapter reconnects may retain an incarnation only after validating the same live process/session. Messages for ended incarnations remain visible but are never transferred to a replacement automatically.

Bind agent commands to their registered incarnation rather than accepting an arbitrary caller-supplied sender ID. Inbox access is scoped to that participant; a participant cannot fetch another agent's messages through a message ID or cursor. The human observer has a separate connection role. Enforce these distinctions in the local interface and SSH protocol, while retaining the same-OS-user trust limitation described below.

Projects have explicit shared IDs with local root mappings for PC and Mac. Suggest the current repository during setup, but do not equate two directories solely by basename. Unmapped workspaces remain machine-local. Existing worktrees can map to the same project. Selecting a different project must be explicit.

`@p` fuzzy completion suggests PM and other matches, showing project, machine, client, and short session ID. Completion is discovery, not implicit fuzzy routing: unresolved or ambiguous names cannot send. A selected recipient is pinned to its ID. CLI ambiguity produces an error with candidates. Duplicate names are allowed and visibly disambiguated.

Resolve multi-recipient sends once and deduplicate the recipient IDs. `@all` snapshots the known, registered, non-ended participants of the current project on both machines, excluding the sender. It is never an implicit default and does not reach sessions registered later. Offline peers make the roster stale; show this before a human broadcast and require an explicit CLI acknowledgement of stale-roster sending. No command silently broadens a send to another project or a replacement session.

## User and agent interaction

Proposed command contract, to implement only after spec review:

| Command | Meaning |
| --- | --- |
| `lam chat` | Open the current project's observer feed and composer |
| `lam chat send --to ADDRESS [--to ADDRESS] --message TEXT` | Persist one addressed message; return its ID and per-recipient queue state |
| `lam chat reply MESSAGE_ID --message TEXT [--all]` | Reply to the original sender; `--all` includes original recipients, excluding self |
| `lam chat sessions` | List scoped participants and capabilities |
| `lam inbox` | Fetch a bounded page of this participant's outstanding full messages |
| `lam inbox show MESSAGE_ID` | Fetch one addressed message, paginating oversized content |
| `lam inbox --history` | Read bounded, retained history without re-enqueueing it |
| `lam chat status` | Show process, peer, registration, and adapter health |

Support stdin/file input for message bodies so shell quoting does not constrain content. Bodies are data, never executable shell fragments. Chat commands accept an explicit project selector; ordinary sends default to the registered/current project and inbox reads remain participant-scoped. Machine-readable output uses message IDs and explicit statuses. Help must distinguish commands planned here from commands actually shipped. When implemented, update both normal help and `skill/lam/SKILL.md`, which also updates the embedded `lam --llm` guide on rebuild.

The composer requires recipient selection before sending and supports multiple mention chips. Mention-like text in a body does not silently expand recipients. Replies retain a parent message ID. Reply-all is explicit. Replying to a human-authored Chat message creates a response in the observer feed; it does not generate an attention notification. New requests for Carlos's action use existing LAM commands.

The observer feed shows sender, recipients, body, timestamp, and per-recipient delivery state. It includes both directions of registered project traffic, including agent-to-agent messages not addressed to Carlos. Agents do not receive this feed. Opening or scrolling it never consumes another participant's inbox. Preserve scroll position while new messages arrive; indicate unseen rows without forcing a jump. Connection and stale-roster indicators remain visible.

## Inline delivery and context bounds

Default to full inline text with an attributed envelope:

```text
LAM | Carlos -> @pm | message <id>
Pause dev and babysitter while I clean up their context.
```

Agent messages carry agent origin and their registered identity instead. Presentation must not infer human authority from a display name such as Carlos. Every delivery includes stable message IDs; the sender cannot choose envelope fields by embedding text that resembles a header.

- Inline body limit: 4 KiB UTF-8 per message.
- Entire rendered delivery limit: 8 KiB UTF-8, including attribution and overflow hints.
- Both limits are configurable positive values; the per-message limit cannot exceed the batch limit. These are initial limits, not optimized token budgets.
- Preserve full stored text. Cut previews only at UTF-8 character boundaries and label them as partial, with the message ID and retrieval command. Do not silently truncate or create model-generated summaries.
- Deliver several short messages together in recipient-inbox order. A long message contributes a bounded preview rather than blocking later short messages indefinitely.
- The batch budget reserves room for one overflow count and retrieval hint. Never replace the whole short-message batch with a generic new-message notice.
- Payload ingestion also needs a finite bound: propose a 64 KiB body maximum for v1, with larger reports directed to Articles. Reject oversize input before persisting or forwarding it. This is separate from inline limits.
- Automatically present only new messages/previews. An already presented message is not a recurring reminder. Further new batches may arrive at later safe boundaries, subject to the same limit.
- Empty checks produce no stdout/model context and no model request. Pending overflow remains fetchable without repeated reminder injection.

Inbox reads have bounded output and explicit continuation cursors. Fetching content records a fetch receipt for that recipient, not human read state or proof of model understanding. Explicit history reads do not cause automatic redelivery. After compaction, the agent can request history; LAM must not automatically dump it into the new context.

## Adapter contract and safety gate

Adapters receive a bounded batch and an incarnation-bound delivery-attempt ID. They report what they can prove: not submitted, accepted by the client, or outcome unknown. They do not manufacture model-read or acted acknowledgements. Preserve each client's permissions, inbound-message controls, and tool execution. Do not use interrupt APIs, Ctrl-C, focus changes, or simulated typing for delivery.

The installed integration explains that LAM messages are coordination requests within the user's existing task and permissions. Agents may act on them within that scope and reply through LAM. Receiving a message is not itself an instruction to reply, contact more agents, or send an acknowledgement ping; avoid automatic acknowledgement conversations. User-established constraints remain controlling.

Claude uses its native peer socket and non-interrupting delivery. Pi uses an extension's custom-message mechanism, steering when busy and triggering a turn when idle. Their production adapters must preserve agent provenance and handle receiver refusal rather than changing client settings to force acceptance. The [Claude documentation](https://code.claude.com/docs/en/cross-session-messaging) describes the native mechanism; installed-version tests remain required.

For ordinary Codex, the tested route is a synchronous tool-completion hook while busy and native queuing for idle wake. Hook checks are local, target a normal empty-path runtime under 50 ms on the test machines, and have a hard two-second timeout. These are acceptance targets, not measured production performance. Not all tool paths emit hooks, so promise the next supported safe boundary, not an arbitrary wall-clock delivery deadline. See the [Codex hook contract](https://learn.chatgpt.com/docs/hooks).

Codex inline provenance is a release gate. Its tested hook adds developer-level context. Do not concatenate arbitrary message text as privileged instructions. Encode the attributed message as explicitly untrusted structured content under a fixed adapter-owned instruction. Test delimiter/header spoofing, embedded role instructions, impersonation, and attempts to override the user's task or permissions. A label or successful nonce echo alone does not establish this protection. If the installed client cannot preserve that distinction, stop the Codex inline rollout and report the limitation; do not silently revert all messages to mandatory inbox fetch or claim cross-client inline support.

Human-composer origin is recorded separately from agent origin; agent senders cannot select it through a `--name` or message body. This is a personal, same-user system, not a security sandbox against arbitrary processes with the same OS account. Chat messages and origin labels never replace native permission prompts or the existing LAM approval workflow. Do not promise cryptographically verified human intent against same-user terminal automation.

Keep diagnostics free of message bodies by default. Escape terminal control sequences and render text without executing embedded commands or markup. Validate frame lengths, socket ownership, incarnation identity, and protocol version before dispatch. Unsupported client versions/capabilities become visible unavailable states, not silent successes.

## Persistence, receipts, and races

Persist immutable messages, frozen recipient sets, reply references, and an outbox event in one local transaction before returning success. A client-generated idempotency key scoped to the sender incarnation makes a retry return the original message; a new intentional send with identical text is a new message. Reusing a key with different content or recipients is an error.

The destination machine owns each recipient's inbox and delivery-attempt state. Inbox order is a monotonic destination sequence. Cross-machine timestamps are informational; do not promise a global causal order based on unsynchronized clocks. Preserve sender sequence during reconnect replay. Retain messages and receipts across local process restarts; v1 does not automatically delete history or expire unsent messages.

Receipt vocabulary:

| State/evidence | Meaning |
| --- | --- |
| Queued locally | The sender machine durably accepted the addressed message |
| Received remotely | The destination machine durably stored the recipient inbox entry |
| Accepted by client | The adapter has a positive client handoff receipt; this is not model-read proof |
| Preview only / fetched in full | How much content has been exposed through delivery or explicit inbox retrieval |
| Reply received | A new message references this message; its text may describe action but LAM does not infer task completion |
| Unavailable / refused / outcome unknown | Delivery is blocked, rejected, or cannot be confirmed; show the reason and retain content |

Content exposure and transport state are separate fields; fetching a preview's remainder does not pretend to be a new transport acknowledgement. Aggregate multi-recipient rows without hiding which recipients remain pending. Never label a participant paused merely because a pause message was sent or accepted.

Claim pending batches transactionally per recipient. Hooks and idle-wake handlers compete for the same delivery attempt rather than independently consuming messages. The adapter checks current client state before handoff. A transition from busy to idle must re-evaluate pending work; one stale busy sample cannot strand it until the next user prompt. An idle queue submission racing with a new user turn must not also cause a hook to inject the same payload. The implementation must validate its event/lease protocol with the real clients before enabling unattended delivery.

Database claims alone do not give exactly-once model injection. A crash after external handoff but before persisting the receipt is an uncertain outcome. If the client supports receipt reconciliation or idempotent handoff, use the same attempt ID to recover. Otherwise expose outcome unknown and allow explicit retry to the original incarnation; do not automatically replay indefinitely or mark the message delivered. Hooks that can only confirm stdout production must not upgrade that evidence to client acceptance. Normal reconnect replay deduplicates stored messages and receipts independently of this external handoff uncertainty.

## PC and Mac link

Use one persistent SSH connection carrying a versioned, length-bounded bidirectional Chat protocol. Configure one initiator per peer pair to avoid duplicate active links. SSH host-key checking stays enabled; existing keys/config can be used without copying private keys into LAM. No direct forwarding of native client sockets, public TCP listener, or exposure of Codex's app-server is required.

The link exchanges configured project mappings, participant presence, addressed messages, receipts, and observer-feed events. Synchronize the authorized project feed to both human observers, but create agent inbox entries only for the frozen recipients. Use stable event IDs, origin sequence numbers, and acknowledged replay cursors. Commit received events before acknowledging them; do not re-export imported events back to their origin. The sender owns immutable message content and the destination owns its delivery receipts, avoiding last-writer-wins edits to one shared message record.

When disconnected, local sends and local delivery continue. Remote sends remain queued and visibly waiting for the peer. Reconnect retries use capped backoff with jitter and no model-context polling messages. Stale presence is not proof a remote process died. At reconnection the destination validates the original incarnation before dispatch; an ended target becomes unavailable, never a replacement by name. A restored database with an old replay position must still deduplicate already known event IDs.

## Verification and rollout

Build in three independently verifiable increments after the implementation plan is approved:

1. Validate attributed inline delivery on all clients, then implement the local persistent flow, commands, adapters, and terminal composer/feed. Keep setup opt-in.
2. Add SSH replication and test the same contracts on the Mac. Local-only operation remains useful when a peer is absent.
3. Harden unattended operation, publish agent help, install the verified builds/integrations on the intended machines, and exercise the real pause workflow. Do not restart shared client daemons or unrelated sessions without explicit approval.

Required acceptance cases:

- Claude, Codex, and Pi each receive a short fresh message at a supported tool boundary while a long-running tool completes unchanged, and each wakes from idle.
- Each client sends to each other client through actual LAM commands; sender and recipients are correct in the observer feed. The compatibility spike only tested scripted senders.
- Empty checks inject nothing; no duplicate reminder appears after a later unrelated tool. Several pending short messages batch; UTF-8, preview, total-budget, and inbox-pagination limits hold.
- Carlos addresses PM only; PM messages dev and babysitter together; both acknowledge after completing their current tools. Only explicit acknowledgements establish that work has paused.
- Mention ambiguity blocks send; completion retains exact IDs; explicit `@all` stays in its project; stale-roster acknowledgement and reply/reply-all behavior are tested.
- Restarted processes, reused names/PIDs/socket paths, inherited parent environment IDs, opt-out, and client refusal do not misroute or force delivery.
- Crashes before and after persistence, after handoff but before receipts, duplicate sends, simultaneous hooks, busy/idle transitions, reconnect replay, and partial recipient success produce honest statuses without lost durable messages.
- Agent text cannot impersonate human origin, override adapter framing, grant approvals, or trigger terminal control sequences. Unsupported safe inline delivery fails the capability gate.
- Local operation survives SSH loss. Mac and PC histories converge after reconnect without echo loops. Unknown host keys and incompatible protocols do not trigger insecure fallback.
- Existing CLI/TUI behavior, recommendation enforcement, FYIs, Articles, Android sync, and cloud credentials remain unchanged. Run Rust regression checks; use focused documentation and help checks for agent discovery.

Uninstall disables only LAM-owned integration entries and stops only the LAM Chat process/link. Preserve unrelated hooks, SSH configuration, existing LAM configuration, and the Chat database/history. A binary rollback must not rewrite an incompatible Chat schema; migrations are versioned and checked before writes.

## Compatibility evidence, 2026-09-13

These are historical observations, not guarantees for newer versions or Mac clients:

| Tested client/path | Observed result |
| --- | --- |
| Claude Code 2.1.269 native socket | Idle wake and busy delivery succeeded; a 20-second tool completed without interruption |
| Pi 0.85.1 with existing control extension | Idle and steer modes succeeded, using its existing openai-codex provider login for the test |
| Ordinary Codex CLI 0.154.0 queue | Idle wake succeeded; a busy session processed the message in a later turn |
| Codex CLI attached to app-server 0.153.4 | `turn/steer` accepted and recorded input, but model receipt failed twice; cause not established |
| Ordinary Codex CLI 0.154.0 PostToolUse hook | Fresh token appeared in the same turn after tool completion; empty follow-up injected no duplicate token |

The hook's standalone empty check emitted zero stdout/stderr bytes and took about 18 ms once. It was not a performance benchmark or a provenance attack test. The successful Claude steering case explicitly authorized its peer to choose the test response token; delivery does not guarantee obedience.

Tests used owned interactive PTYs and throwaway Node scripts. Herdr was inspected but not used as a host or transport. No production LAM code changed. Cross-machine delivery, production inboxes/receipts, races, and safe arbitrary inline content were not proven. The initially suggested mandatory `lam inbox` fetch was superseded by the inline-first decision on 2026-09-14.

## Implementation plans

The approved design is split into [local messaging](../plans/2026-09-14-lam-chat-local.md), [PC/Mac synchronization](../plans/2026-09-14-lam-chat-peers.md), and [setup/rollout](../plans/2026-09-14-lam-chat-rollout.md), in that order. The 64 KiB ingestion bound and uncertainty/stale-roster rules are part of the approved baseline. Codex inline provenance and race handling remain release gates that must be resolved through implementation evidence, not assumptions.
