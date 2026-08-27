---
name: lam
description: Use when blocked on a decision only Carlos can make (approval, choice between options, destructive action, credentials) and you need to reach him wherever he is — pushes to his phone and desktop via `lam` and waits for his answer.
---

# lam — Look At Me

`lam` is a queue Carlos reads from his phone (ntfy) and PC. You push an item, block on it, and get his answer back as JSON. Replaces `ssh carraes notify-send` and adb pings.

## When

Genuine blockers only: a decision, an approval, a secret, a "wave finished while he's away". Never for FYI progress. One item per blocking event, not per agent.

## Who you are

Every item carries a **name** so Carlos can tell concurrent agents apart. Inside tmux, zellij or screen it is inferred as `session:window` — you do not have to think about it. Only when `lam push` errors with "who is asking?" do you pass `--name <session:window-ish label>` (or export `LAM_NAME` once at the start of your run).

## How

```bash
# ask a question with buttons (max 3 choices) and block until answered
lam push "PR #2529: waive artifact check?" -b "Reply in Claude Code session mp-2529" -p critical -c waive -c require --wait

# two-step, with an "Open" button to the thing to look at and a deadline after which the ask expires
ID=$(lam push "PR #2529 needs your click" --link https://github.com/org/repo/pull/2529 --ttl 2h -c done)
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

Checklist loop: `lam wait` exits 0 both on progress and on resolution — check `status`: `open` means "some check flipped, act on it and call `lam wait` again"; anything else is final. `--check` and `--choice` are exclusive.

`wait` prints the item as JSON. Read `response_choice` (button pressed) and `response_text` (free text). Exit codes: `0` resolved, `2` dismissed (he doesn't want to deal with it — stop and report), `3` timeout (fall back to `lam list` later; do not re-push the same question), `4` expired (TTL passed — decide whether to re-push), `5` retracted.

- `-p critical` only for actual blockers; `normal` for "look when convenient".
- The body may be **markdown** — headings, bullets, tables, fenced code. Carlos reads it rendered in the terminal (`m` opens a reader pane), so send the whole plan or diff summary when the decision needs it rather than a one-line teaser. The phone shows the same text unrendered, so keep the first line meaningful.
- A push with the same name, title and body as an item that is still open returns **that item's id** and does not notify again — so a retry after a failed-looking push is safe, and re-asking an open question is a no-op.
- Never invent a name that hides who you are: the inferred `session:window` is what Carlos looks for when several agents are running.
- Title = the decision. Body = where to act ("Reply in Claude Code: …"). Host and project are attached automatically.
- Always pass `--link` when there is a URL to act on, and `--ttl` when the ask stops mattering after a while — stale items make the queue untrustworthy.
- The phone notification shows at most 3 buttons; with 3 choices the Open/Reply buttons are still available inside the ntfy app.
- `lam list` shows open items; `lam show <id>` shows one. Never resolve items yourself with `lam done` or tick checks with `lam check tick` — that is Carlos's side. `lam retract` and `lam check add` are the only mutations that are yours.
