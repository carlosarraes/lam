# lam — Look At Me

Agents queue blockers; Carlos answers from phone or PC; agents read the answer back.

```
agent ──lam push──▶ lam-api (CF Worker + D1) ──▶ ntfy.sh ──▶ phone (buttons) / `lam watch` (desktop)
  ▲                       ▲                                          │
  └──── lam wait ─────────┴──────── button / reply page / lam done ──┘
```

## Layout

- `worker/` — Cloudflare Worker (Hono, D1). `npm test`, `npm run deploy`.
- `cli/` — Rust CLI. `cargo test`, `cargo build --release`.
- `skill/lam/` — agent-facing skill; symlink into `~/.claude/skills/lam`.

## Setup

1. Worker: `cd worker && npx wrangler deploy && npx wrangler d1 migrations apply lam --remote`, then `wrangler secret put` for `LAM_TOKEN`, `LAM_HMAC_SECRET`, `NTFY_TOPIC`, `NTFY_TOKEN` (ntfy.sh account token — required, Workers' shared egress IPs exhaust the anonymous quota).
2. CLI: `lam init --server https://lam-api.<acct>.workers.dev --token <LAM_TOKEN> --topic <NTFY_TOPIC>` → `~/.config/lam/config.toml`.
3. Phone: ntfy app, subscribe to the topic.
4. Desktop: run `lam watch` (systemd user unit / launchd).

## CLI

| cmd | |
|---|---|
| `lam push <title> [-b body] [-p low\|normal\|critical] [-c choice]… [--wait]` | prints id |
| `lam wait <id> [--timeout 2h]` | prints JSON; exit 0 resolved / 2 dismissed / 3 timeout |
| `lam list [--all] [--json]`, `lam show <id>` | |
| `lam done <id> [choice] [-m text]`, `lam dismiss <id>` | Carlos's side |
| `lam watch` | mirror ntfy → desktop notifications |
