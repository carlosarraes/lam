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

1. Worker: `cd worker && npx wrangler deploy && npx wrangler d1 migrations apply lam --remote`, then `printf %s '<value>' | npx wrangler secret put <NAME>` for `LAM_TOKEN`, `LAM_HMAC_SECRET`, `NTFY_TOPIC` (an unguessable topic name — it is the only access control on the topic).
2. CLI: `lam init --server https://lam-api.<acct>.workers.dev --token <LAM_TOKEN> --topic <NTFY_TOPIC>` → `~/.config/lam/config.toml` (`~/Library/Application Support/lam/` on macOS).
3. Phone: install the ntfy app (Play/F-Droid), *Add subscription* → topic `<NTFY_TOPIC>` → *Use another server* → `https://lam-api.<acct>.workers.dev`. No account. The app keeps one streaming connection ("instant delivery").
4. Desktop: `lam` opens the TUI to answer items; run `lam watch` (systemd user unit / launchd) for notifications.

Pushing the same name+title+body while the original is still open returns the existing item (HTTP 200, same id) instead of queueing a second one, so a retry after a lost response never double-notifies.

## Agent names

Every item records **who asked**. `lam push` resolves the name in this order: `--name` → `$LAM_NAME` → the multiplexer (`tmux display-message -p -t "$TMUX_PANE" '#S:#W'`, zellij session, or screen `$STY`) → error. Pane-targeting matters: an agent working in a background tmux window would otherwise report whichever window you are looking at.

## CLI

| cmd | |
|---|---|
| `lam push <title> [-n name] [-b body] [-p low\|normal\|critical] [-c choice]… \| [--check part]… [--link URL] [--ttl 2h] [--wait]` | prints id; `--check` makes a checklist that resolves when all parts are ticked |
| `lam wait <id>… \| --any [--timeout 2h]` | first item to change (a check ticked) or close, as JSON; `--any` covers every open item under this agent's name; exit 0 resolved/changed / 2 dismissed / 3 timeout / 4 expired / 5 retracted |
| `lam retract <id>`, `lam check add <id> <label>` | agent withdraws its ask / appends a check |
| `lam check tick\|untick <id> <n>` | Carlos's side, from the terminal |
| `lam list [--all] [--json]`, `lam show <id>` | |
| `lam done <id> [choice] [-m text]`, `lam dismiss <id>` | Carlos's side |
| `lam` / `lam tui` | interactive queue in the terminal: `1-3` choose, `Enter` done, `Tab`/`Space` tick checks, `r` reply text, `d` dismiss, `o` open link, `/` filter by agent, `a` show all, live updates |
| `lam watch` | mirror ntfy → desktop notifications |
| `lam --llm` | print the agent guide (the `lam` skill) — for agents that don't have the skill installed |
