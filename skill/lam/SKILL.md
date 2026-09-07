---
name: lam
description: Use when Carlos needs to make a decision, grant approval, provide credentials, or receive a progress or completion update about already-authorized work while away.
---

# lam: Look At Me

`lam` is a queue Carlos reads from his phone and PC. Requests collect decisions; FYIs deliver information and close when read.

## When

Choose the kind from what Carlos needs to do:

- If an action needs his decision, approval, credentials, or other input, send a request with your recommendation and wait for his answer. Omitted `--kind` defaults to `request`.
- If already-authorized work has progressed or finished and only an update is needed, send `--kind fyi` and continue. FYIs never grant approval. Any action still lacking authorization remains a request.

Send one item per useful update or blocking event. For an informational update rejected for a missing recommendation, switch to `--kind fyi` and remove wait/decision flags. Never invent a filler recommendation such as "No action needed" to satisfy request validation.

These commands require the FYI-capable CLI and Worker. Check `lam push --help` for `--kind`. An older CLI or a server-upgrade error means rollout is needed; it does not mean FYI is already live. `lam --llm` prints the guide embedded in the installed CLI.

## Who you are

Every item carries a name so Carlos can tell concurrent agents apart. Inside tmux, zellij or screen it is inferred as `session:window`. When `lam push` errors with "who is asking?", pass `--name <session:window-ish label>` or export `LAM_NAME` for the run.

## How

```bash
# informational completion of work Carlos already authorized; publish and continue
lam push "Local verification finished" --kind fyi -b "All checks passed. The change is ready for review." -p low

# ask a question with buttons (max 3 choices) and block until answered
# Every decision says what you recommend and why; the recommended choice exactly matches a --choice.
lam push "PR #2529: waive artifact check?" -b "Reply in Claude Code session mp-2529" -p critical -c waive -c require --recommendation "Waive it: the published artifact's checksum and smoke test are green." --recommended-choice waive --wait

# two-step decision, with an "Open" button and a deadline after which the ask expires
ID=$(lam push "PR #2529 needs your click" --link https://github.com/org/repo/pull/2529 --ttl 2h --recommendation "Approve it: the review and CI are complete.")
lam wait "$ID" --timeout 1h

# several asks in flight: block until whichever closes first (prints that item)
lam wait "$ID1" "$ID2"        # explicit ids
lam wait --any                # every open item under your name

# the world already resolved it (he did the thing without tapping): withdraw your own ask
lam retract "$ID"

# multi-part ask: one check per thing he has to do; he ticks them one by one, item resolves on the last tick
ID=$(lam push "Trigger CodeRabbit on tonight's PRs" --check "PR #2597" --check "PR #2598" --link https://github.com/org/repo/pulls)
lam wait "$ID"                 # returns on EVERY change: read .checks[].done, act on what is ticked, then wait again
lam check add "$ID" "PR #2601" # a new part became ready: append instead of pushing a second item
```

Checklist loop: `lam wait` exits 0 both on progress and on resolution. Check `status`: `open` means "some check flipped, act on it and call `lam wait` again"; anything else is final.

Every non-checklist request requires `--recommendation <action and rationale>`. With `--choice`, also pass `--recommended-choice <CHOICE>` exactly matching a choice. Checklist requests omit both recommendation flags. `--check` and `--choice` are exclusive.

FYIs reject `--wait`, `--choice`, `--check`, `--recommendation`, and `--recommended-choice`. They have no reply or decision fields. Never call `lam wait ID` on an FYI or include one in an explicit multi-ID wait. `lam wait --any` selects requests only and returns immediately if only FYIs exist.

`lam list` and `lam show ID` inspect without marking an FYI seen. In the TUI, `m` or Enter opens the FYI reader and marks it seen; selecting its row does not. A visible phone page also marks it seen after rendering, with a Mark seen button when scripting is unavailable or the automatic request fails. Fetching or prefetching the page alone changes nothing. Seen FYIs leave the open queue and remain readable in history. Explicit Dismiss also closes them, but leaves `seen_at` empty and history says Dismissed, not Seen.

`wait` prints the item as JSON. Read `response_choice` for buttons and `response_text` for free text. Exit codes: `0` resolved or checklist progress; `2` dismissed, stop and report; `3` timeout, inspect later without repeating the question; `4` expired, reconsider whether the request still matters; `5` retracted.

- Requests accept `-p warning`, `normal`, or `critical`; `warning` is an alias for wire `normal`. New requests reject `low`. FYIs accept `low`, `normal`, or `critical`, plus the same warning alias. Use critical only when urgency warrants it. Request normal displays as Warning; FYI normal displays as Normal. Old low-priority requests remain readable.
- The body may be markdown. Carlos reads it rendered in the terminal reader, so include the context needed to decide. The phone shows the same text unrendered; keep the first line meaningful.
- Retrying an identical open push returns that item's ID without notifying again. Kind, name, title, body, priority, ordered choices/checks, link, TTL seconds, and recommendation fields determine identity. Source host and project do not.
- Never invent a name that hides who you are: the inferred `session:window` is what Carlos looks for when several agents are running.
- Request title = the decision; recommendation = what you recommend and why; body = context and where to act. FYI title/body = the update and its result. Host and project are attached automatically.
- Pass `--link` when there is a relevant URL, and `--ttl` when the item stops mattering after a while.
- The phone notification shows at most 3 buttons; with 3 choices the Open/Reply buttons are still available inside the ntfy app.
- Leave answering, marking seen, dismissing, and ticking checks to Carlos. As the sending agent, use `lam retract` to withdraw an obsolete item and `lam check add` to extend a checklist. Do not use `lam done` or `lam check tick` to answer your own requests.
