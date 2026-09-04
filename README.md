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

Item bodies are markdown. In the TUI, `m` opens a side-by-side reader (list left, rendered markdown right) with `J`/`K`, `PgUp`/`PgDn` and `g`/`G` to scroll — so an agent can send a whole plan, not a teaser.

## Agent names

Every item records **who asked**. `lam push` resolves the name in this order: `--name` → `$LAM_NAME` → the multiplexer (`tmux display-message -p -t "$TMUX_PANE" '#S:#W'`, zellij session, or screen `$STY`) → error. Pane-targeting matters: an agent working in a background tmux window would otherwise report whichever window you are looking at.

## CLI

| cmd | |
|---|---|
| `lam push <title> [-n name] [-b body] [-p low\|normal\|critical] [--recommendation TEXT] [-c choice]… [--recommended-choice CHOICE] \| [--check part]… [--link URL] [--ttl 2h] [--wait]` | prints id; every decision requires `--recommendation` (the action and rationale); choices also require an exact `--recommended-choice`; `--check` makes a checklist that resolves when all parts are ticked and is the only exception |
| `lam wait <id>… \| --any [--timeout 2h]` | first item to change (a check ticked) or close, as JSON; `--any` covers every open item under this agent's name; exit 0 resolved/changed / 2 dismissed / 3 timeout / 4 expired / 5 retracted |
| `lam retract <id>`, `lam check add <id> <label>` | agent withdraws its ask / appends a check |
| `lam check tick\|untick <id> <n>` | Carlos's side, from the terminal |
| `lam list [--all] [--json]`, `lam show <id>` | |
| `lam done <id> [choice] [-m text]`, `lam dismiss <id>` | Carlos's side |
| `lam` / `lam tui` | interactive queue in the terminal: `1-3` choose, `Enter` done, `Space` tick the next check, `Tab` + `j/k` pick a check, `r` reply text, `d` dismiss, `o` open link, `m` markdown reader, `/` filter by agent, `h`/`l` (or `Ctrl+1`/`Ctrl+2`) switch between the **requests** and **history** tabs, live updates |
| `lam watch` | mirror ntfy → desktop notifications |
| `lam --llm` | print the agent guide (the `lam` skill) — for agents that don't have the skill installed |
