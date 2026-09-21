# macOS Codex delivery through cx

**Status:** Approved in chat on 2026-09-21

## Goal

Give a Codex session launched through `cx` on the Mac the same experimental LAM Chat behavior already gated on Linux: project-scoped enrollment, authenticated agent commands, inline delivery after a tool call, and an exact-thread idle wake. Keep `cx` responsible for its Codex app-server connection and LAM responsible for messages, recipients, delivery state, and peer synchronization.

## Current constraint

On Linux, LAM connects to Codex's private `app-server-control.sock` after proving that the registered Codex backend owns it. A Mac session launched by `cx` has no such listener. `cx` starts `codex app-server --listen stdio://`, owns the app server's stdin and stdout, and accepts one Codex TUI on its private Unix socket. A second client cannot attach without either bypassing `cx` or adding a relay to it.

The production Mac LAM service therefore remains observer-only until this design passes its native gates. Existing agents continue using herdr and are not restarted during development.

## Boundaries

LAM remains the sole owner of:

- project mappings, participants, messages, receipts, retries, and peer replication;
- per-session enrollment, participant, and integration secrets;
- claim-before-delivery ordering and `Accepted`, `Unknown`, and `NotSubmitted` outcomes;
- inline rendering and the rule that peer text carries no human authority.

`cx` owns only:

- the live stdio connection to its Codex app server;
- proof that an enrollment caller descends from the exact Codex app-server it launched;
- three relay operations for one runtime: bind, inspect, and queue;
- correlation and validation of Codex app-server responses.

The relay is not a general Codex proxy and does not store Chat history. It never contacts another machine.

## Runtime layout

Each `cx` process creates `lam.sock` beside its existing TUI socket in the private temporary runtime directory. The directory is mode `0700`; the relay socket is mode `0600`. `cx` passes its absolute path only to the Codex app-server as `CX_LAM_RELAY`, because that process owns hook and tool descendants. The path is a locator, not a credential.

The LAM SessionStart hook discovers the nearest supported Codex process and its parent `cx` process. It verifies both exact process lifetimes, the parent-child relationship, the relay path and permissions, and the relay's kernel peer PID. It then creates the ordinary private LAM binding, enrolls with the LAM daemon, and binds that thread at the `cx` relay using the binding's integration secret.

The private binding retains:

- the Codex app-server process evidence;
- the exact native thread ID and LAM session incarnation;
- the `cx` process evidence and relay path;
- the existing three distinct LAM secrets.

Codex and `cx` process evidence must still match at every later delivery. A replacement process, reparented app-server, missing relay, changed socket owner, or unsupported version makes the recipient unavailable. Messages stay attached to the original incarnation and never transfer to a replacement session.

Enrollment is resumable for the same exact lifetime. If SessionStart persists the LAM incarnation but reaches its native timeout before attaching the relay, a later trusted hook repeats the idempotent Bind and completes that same private record. It cannot rotate credentials, move the incarnation, or adopt another relay.

## Relay protocol

`lam.sock` uses versioned, length-prefixed JSON frames. A frame is at most 32 KiB. `cx` accepts a bounded number of concurrent connections and applies one absolute two-second deadline to connection, request, app-server response, and reply. Logs contain fixed diagnostics only, never message bodies or credentials.

### Bind

The SessionStart hook sends:

```json
{
  "version": 1,
  "operation": "bind",
  "thread_id": "UUID",
  "binding": "64 lowercase hexadecimal characters"
}
```

`cx` accepts Bind only when the kernel peer belongs to the current user and descends from the exact live Codex app-server process. It sends `thread/read` to that server and requires the response to name the same thread before retaining the binding in memory. Repeating an identical Bind is idempotent. A different secret cannot replace a live binding for the same thread.

The response is:

```json
{"version":1,"ok":true}
```

### Inspect

The LAM queue worker sends the thread ID and integration secret. `cx` first reports `active` from its own turn tracker when a native turn is in progress. Otherwise it calls `thread/read` and requires an exact thread ID, a status of `idle` or `active`, and `canAcceptDirectInput` other than `false`.

The response contains only the validated state:

```json
{"version":1,"ok":true,"state":"idle"}
```

LAM reserves a new observation epoch and claims from its database only after this response. No body crosses into `cx` before the durable claim.

### Queue

After claiming, LAM sends the Queue operation below. `cx` refuses it with `submission=not_started` if a native turn became active after Inspect, closing the Inspect-to-Queue race without injecting a second turn.

```json
{
  "version": 1,
  "operation": "queue",
  "thread_id": "UUID",
  "binding": "64 lowercase hexadecimal characters",
  "attempt_id": "UUID",
  "text": "bounded rendered peer message"
}
```

`cx` calls `thread/queue/add` with the exact thread ID, attempt ID as `clientUserMessageId`, and one text input. It accepts the result only when Codex returns a UUID receipt with the same client message ID and input. The response returns that receipt without provider payloads.

Every relay error uses a fixed code and a `submission` field of `not_started` or `uncertain`. `not_started` is permitted only before `cx` writes any part of `thread/queue/add` to the app server. Once that write begins, disconnects, timeouts, Codex errors, and validation failures report `uncertain`. LAM treats a missing relay response after sending Queue as `uncertain`.

The secret stays in LAM's private binding and `cx` memory. It is absent from process arguments, the Codex environment, hook output, Chat history, and logs.

## Busy and idle delivery

PostToolUse on macOS uses the existing LAM hook claim and renderer. The hook writes at most 8192 bytes to stdout under the existing absolute hook deadline. An owned Mac characterization test must prove that the selected nonblocking output method leaves the parent descriptor flags unchanged. If that invariant cannot be met, the hook emits nothing and the rollout stops rather than enabling an unbounded writer.

An idle Stop observation arms the existing LAM queue coordinator. When a message arrives, its worker validates the retained Codex and `cx` evidence, performs Inspect, claims the pending batch through the sole SQLite owner, and performs Queue. A message arriving while Codex reports `active` can be queued but is not injected into or used to interrupt a running tool.

Empty PostToolUse checks emit zero stdout and stderr bytes. Old messages do not replay on later hooks.

## Outcomes and failure handling

LAM records `Accepted` only after `cx` verifies Codex's matching queue receipt.

Failures before `cx` writes `thread/queue/add` are `NotSubmitted`. The pending message remains eligible for a later explicit delivery attempt under the existing coordinator rules.

A timeout, disconnect, malformed response, or error after the queue write begins is `Unknown`. LAM does not replay it automatically. The existing bounded completion repair persists the outcome without repeating native delivery.

Bind and Inspect never consume a message. Relay shutdown ends only the affected native connection. `cx` account switching and TUI traffic continue unless the Codex app server itself fails.

## Compatibility

The initial gate supports this exact production combination:

- macOS arm64;
- `cx` relay protocol 1;
- Codex CLI 0.155.1;
- the project-scoped LAM Codex hook template in this branch.

Linux keeps its direct backend socket path and current Codex 0.153.4 and 0.154.0 gates. The Mac version allowance does not broaden Linux support. Claude and Pi adapters are unchanged. Zapsign gains no Chat service or project mapping from this work.

## Installation and rollback

The dirty `/home/carraes/projs/cx` checkout is preserved. `cx` work happens on a separate `feat/lam-relay` worktree based on its current committed HEAD. LAM work remains on `feat/lam-chat`.

Rollout order:

1. Pass unit and integration suites in both repositories on Linux.
2. Build both committed branches on the Mac and pass their full suites.
3. Run isolated Mac `cx`, LAM daemon, hook, and peer probes with private config and data directories.
4. Back up and atomically replace the Mac binaries.
5. Change the Mac LAM launch agent to `chat serve --foreground --native-bindings` and restart only that service.
6. Install the reviewed Codex hook in `mondrio-platform`.
7. Start new owned Codex sessions for the live gate. Existing user sessions remain untouched.

Rollback restores both named binary backups, the previous observer-only launch agent, and the previous project hook file or its absence. LAM's ordinary cloud configuration and `cx` account state remain byte-for-byte unchanged.

## Acceptance gates

Automated tests must cover:

- exact descendant Bind success and unrelated-process rejection;
- wrong secret, wrong thread, replaced process, and ambiguous response rejection;
- bounded frames, deadlines, connections, and fixed diagnostics;
- Inspect before claim and Queue only after claim;
- matching receipt acceptance and post-write uncertainty;
- identical Bind idempotency and conflicting Bind refusal;
- account switching and current `cx` commands without protocol regressions;
- Mac hook output without inherited descriptor mutation;
- Linux direct Codex delivery without behavior changes.

The owned Mac gate must then prove:

- a busy message appears once at the next PostToolUse boundary;
- an idle message wakes the exact Codex thread and produces a matching receipt;
- Mac-to-PC and PC-to-Mac messages preserve sender, recipients, and reply linkage;
- restarting the LAM daemon and reconnecting the peer produces no duplicate delivery;
- 100 empty hook checks emit no output, with measured latency recorded;
- disabling the hook leaves a message pending rather than claiming delivery.

Only those owned sessions may be started or stopped by the gate. Shared Codex sessions, account selection, credentials, phone data, and herdr traffic remain unchanged.

## Non-goals

- General access to Codex app-server methods through `cx`.
- Retrofitting already-running sessions.
- Mac Pi support or changes to Claude delivery.
- Cloud relay, phone Chat notifications, attachments, or human approvals through Chat.
- Treating transport acceptance as proof that an agent understood or acted on a message.
