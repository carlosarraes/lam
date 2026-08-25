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

# or two-step
ID=$(lam push "Need prod DB password" -b "paste it in Claude Code")
lam wait "$ID" --timeout 1h
```

`wait` prints the item as JSON. Read `response_choice` (button pressed) and `response_text` (free text). Exit codes: `0` resolved, `2` dismissed (he doesn't want to deal with it — stop and report), `3` timeout (fall back to `lam list` later; do not re-push the same question).

- `-p critical` only for actual blockers; `normal` for "look when convenient".
- Title = the decision. Body = where to act ("Reply in Claude Code: …"). Host and project are attached automatically.
- `lam list` shows open items; `lam show <id>` shows one. Never resolve items yourself with `lam done` — that is Carlos's side.
