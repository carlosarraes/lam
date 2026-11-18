# lam — Look At Me

Agents queue blockers; Carlos answers from phone or PC; agents read the answer back.

```
agent ──lam push──▶ lam-api (CF Worker: Effect + D1 + Topic DO) ──ntfy protocol──▶ ntfy app on phone / `lam watch` on desktop
  ▲                       ▲                                                                   │
  └──── lam wait ─────────┴─────────────── button / reply page / lam done ────────────────────┘
```

## Layout

- `worker/` — Cloudflare Worker written with Effect (`@effect/platform` HttpRouter, `Effect.Service`s, Schema). D1 holds items; a `Topic` Durable Object serves an ntfy-compatible topic (publish, `/json` stream, `/ws`, `?poll=1&since=`) so no ntfy.sh account is needed. `npm test`, `npm run deploy`.
- `cli/` — Rust CLI. `cargo test`, `cargo build --release`.
- `skill/lam/` — agent-facing skill; symlink into `~/.claude/skills/lam`.

## Setup

1. Worker: `cd worker && npx wrangler d1 migrations apply lam --remote && npx wrangler deploy`, then `printf %s '<value>' | npx wrangler secret put <NAME>` for `LAM_TOKEN`, `LAM_HMAC_SECRET`, `NTFY_TOPIC` (an unguessable topic name — it is the only access control on the topic).
2. CLI: `lam init --server https://lam-api.<acct>.workers.dev --token <LAM_TOKEN> --topic <NTFY_TOPIC>` → `~/.config/lam/config.toml` (`~/Library/Application Support/lam/` on macOS).
3. Phone: install the ntfy app (Play/F-Droid), *Add subscription* → topic `<NTFY_TOPIC>` → *Use another server* → `https://lam-api.<acct>.workers.dev`. No account. The app keeps one streaming connection ("instant delivery").
4. Desktop: `lam` opens the TUI to answer items; run `lam watch` (systemd user unit / launchd) for notifications.

## Rollout and rollback

Deploy in this order:

1. Apply D1 migrations 0006 through 0008.
2. Deploy the Worker.
3. Deploy the CLI and agent skill.

The schema must land first because the new Worker reads the recommendation columns and the device and pairing tables. For a rollback, restore the previous Worker, CLI, and skill but leave the additive D1 migrations in place. The old Worker ignores additive tables and columns; the old CLI remains accepted by the new Worker.

Pushing the same agent name, title, body, priority, ordered choices, ordered checks, link, exact TTL seconds, recommendation, and recommended choice while the original is still open returns the existing item (HTTP 200, same id) instead of queueing a second one. Source host and project are metadata, not identity, so a retry from another machine still cannot double-notify.

That de-dup is what makes `lam push` retry safely. Connect errors, timeouts, HTTP 408/429 and 5xx are retried up to 4 attempts with exponential backoff plus jitter, within a 30s total budget, and each retry prints one stderr line. Other 4xx fail immediately. On final failure `lam push` exits 1 and writes `$XDG_CACHE_HOME/lam/last-error.json` (default `~/.cache/lam/last-error.json`) with `command`, `status` (HTTP code or null), `error`, `retryable`, `attempts`, `elapsed_ms` and `timestamp`, and never the token. Tests shrink the timing with `LAM_PUSH_RETRY_BASE_MS` (first backoff, default 1000) and `LAM_PUSH_RETRY_BUDGET_MS` (total budget, default 30000).

Item bodies are markdown. In the TUI, `m` opens a side-by-side reader (list left, rendered markdown right) with `J`/`K`, `PgUp`/`PgDn` and `g`/`G` to scroll — so an agent can send a whole plan, not a teaser.

## Agent names

Every item records **who asked**. `lam push` resolves the name in this order: `--name` → `$LAM_NAME` → the multiplexer (`tmux display-message -p -t "$TMUX_PANE" '#S:#W'`, zellij session, or screen `$STY`) → error. Pane-targeting matters: an agent working in a background tmux window would otherwise report whichever window you are looking at.

## CLI

FYIs accept one optional text reply from Android's **Reply** action, `r` in the TUI, or `lam done ID -m "reply"`. Seen FYIs can still be replied to in History. They stay out of Requests; Enter still only marks seen. Agents retrieve the saved reply through `lam show ID` → `response_text`. Replies do not automatically wake the sending agent. Retracted, expired, explicitly dismissed, and already-replied FYIs cannot receive another reply.

| cmd | |
|---|---|
| `lam push <title> [-n name] [-b body] [-p low\|normal\|critical] [--recommendation TEXT] [-c choice]… [--recommended-choice CHOICE] \| [--check part]… [--link URL] [--ttl 2h] [--key KEY] [--wait]` | prints id; every decision requires `--recommendation` (the action and rationale); choices also require an exact `--recommended-choice`; `--check` makes a checklist that resolves when all parts are ticked and is the only exception |
| `lam pending list [--json]` | list open pending items |
| `lam pending resolve <id> \| --key KEY [-m NOTE]` | close a tracked item by id or stable external key |
| `lam wait <id>… \| --any [--timeout 2h]` | first item to change (a check ticked) or close, as JSON; `--any` covers every open item under this agent's name; exit 0 resolved/changed / 2 dismissed / 3 timeout / 4 expired / 5 retracted |
| `lam retract <id>`, `lam check add <id> <label>` | agent withdraws its ask / appends a check |
| `lam check tick\|untick <id> <n>` | Carlos's side, from the terminal |
| `lam list [--open\|--all] [-n NAME] [--json\|--ids\|--count]`, `lam show <id>` | open by default; `--name` keeps items whose agent name equals NAME ignoring case; `--ids` prints one id per line, `--count` only the number |
| `lam done <id> [choice] [-m text]`, `lam dismiss <id>` | Carlos's side |
| `lam` / `lam tui` | interactive queue in the terminal: `1-3` choose, `Enter` done or mark FYI seen, `Space` tick the next check, `Tab` + `j/k` pick a check, `r` reply text, `d` dismiss, `o` open link, `m` markdown reader, `/` filter by agent; `p` moves an item to or from Pending; `h`/`l` cycle Requests, Pending and History, or `Ctrl+1` through `Ctrl+3` select a tab; live updates |
| `lam watch` | mirror ntfy → desktop notifications |
| `lam --llm` | print the agent guide (the `lam` skill) — for agents that don't have the skill installed |

Pending is a quiet lane Carlos manages from the TUI (`p`) or the Android Pending screen. Agents send a request (default) or `--kind fyi` when Carlos should see an item now. Creating items as pending and changing lanes from the CLI are rejected. Agents may inspect Pending with `lam pending list` and close their own old trackers with `lam pending resolve` when the world has already resolved them. Default lists exclude Pending. Lane changes do not alert; moving a critical item to Pending cancels its phone notification. `--key KEY` still updates an existing open item with that key and returns the same id, while preserving its lane. See [Pending](docs/pending.md) for commands and API details.
