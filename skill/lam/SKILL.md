---
name: lam
description: Use when blocked on a decision only Carlos can make (approval, choice between options, destructive action, credentials) and you need to reach him wherever he is — pushes to his phone and desktop via `lam` and waits for his answer.
---

# lam — Look At Me

`lam` is a queue Carlos reads from his phone (ntfy) and PC. You push an item, block on it, and get his answer back as JSON. Replaces `ssh carraes notify-send` and adb pings.

## When

Genuine blockers only: a decision, an approval, a secret, a "wave finished while he's away". Never for FYI progress. One item per blocking event, not per agent.

## How

```bash
# ask a question with buttons (max 3 choices) and block until answered
lam push "PR #2529: waive artifact check?" -b "Reply in Claude Code session mp-2529" -p critical -c waive -c require --wait

# two-step, with an "Open" button to the thing to look at and a deadline after which the ask expires
ID=$(lam push "PR #2529 needs your click" --link https://github.com/org/repo/pull/2529 --ttl 2h -c done)
lam wait "$ID" --timeout 1h

# several asks in flight: block until whichever closes first (prints that item)
lam wait "$ID1" "$ID2"        # explicit ids
lam wait --any                # every open item you pushed from this host+project

# the world already resolved it (he did the thing without tapping): withdraw your own ask
lam retract "$ID"
```

`wait` prints the item as JSON. Read `response_choice` (button pressed) and `response_text` (free text). Exit codes: `0` resolved, `2` dismissed (he doesn't want to deal with it — stop and report), `3` timeout (fall back to `lam list` later; do not re-push the same question), `4` expired (TTL passed — decide whether to re-push), `5` retracted.

- `-p critical` only for actual blockers; `normal` for "look when convenient".
- Title = the decision. Body = where to act ("Reply in Claude Code: …"). Host and project are attached automatically.
- Always pass `--link` when there is a URL to act on, and `--ttl` when the ask stops mattering after a while — stale items make the queue untrustworthy.
- The phone notification shows at most 3 buttons; with 3 choices the Open/Reply buttons are still available inside the ntfy app.
- `lam list` shows open items; `lam show <id>` shows one. Never resolve items yourself with `lam done` — that is Carlos's side. `lam retract` is the only closing action that is yours.
