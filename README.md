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
4. Desktop: run `lam watch` (systemd user unit / launchd).

## CLI

| cmd | |
|---|---|
| `lam push <title> [-b body] [-p low\|normal\|critical] [-c choice]… [--link URL] [--ttl 2h] [--wait]` | prints id |
| `lam wait <id>… \| --any [--timeout 2h]` | first item to close, as JSON; exit 0 resolved / 2 dismissed / 3 timeout / 4 expired / 5 retracted |
| `lam retract <id>` | agent withdraws its own ask |
| `lam list [--all] [--json]`, `lam show <id>` | |
| `lam done <id> [choice] [-m text]`, `lam dismiss <id>` | Carlos's side |
| `lam watch` | mirror ntfy → desktop notifications |
| `lam --llm` | print the agent guide (the `lam` skill) — for agents that don't have the skill installed |
