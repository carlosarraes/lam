# Pending

Pending holds open items that should be tracked without a notification. It is a separate lane, stored as a nullable `pending_at` timestamp. Status stays `open`; moving an item preserves its kind, name, link and other origin fields. Closed items retain their normal status values.

## Commands

```bash
lam pending add "PR #42 awaiting review" --key github:org/repo:pr:42 -n reviewer -b "Waiting for review." -l https://github.com/org/repo/pull/42
lam pending list
lam pending list --json
lam pending resolve --key github:org/repo:pr:42 -m "Merged"
lam pending resolve ITEM_ID -m "Finished"
lam pending move ITEM_ID
lam pending unmove ITEM_ID
```

`pending add` creates a FYI with normal priority and requires a stable external key. Name resolution follows `lam push`. Repeating an open key updates its title, body, link and priority and returns the same id. Once that item closes, the key can identify a new item.

Use `lam push ... --pending [--key KEY]` for the full creation options. This preserves the selected kind and its validation. Requests still require `--recommendation` unless they have checks; FYIs still reject decision fields and `--wait`. The pending shortcut does not invent recommendation text.

Creation prints the item id, including when an upsert returns HTTP 200. List supports table or JSON output. Resolve accepts exactly one id or `--key KEY`, plus an optional `-m NOTE`. Resolve and lane changes print the updated item as JSON. A missing open external key returns an error.

Default item lists exclude pending items. `lam pending list` requests only open pending items. Pending creation and lane changes do not alert or ring the bell. Moving a critical item into Pending sends an FCM invalidation to cancel its existing phone notification while keeping the item open. Moving it back does not re-alert.

## TUI and Android

Pending follows Requests in the TUI. Press `p` on an open item to toggle its lane. `h`/`l` cycle the tabs; `Ctrl+1`, `Ctrl+2` and `Ctrl+3` select Requests, Pending and History. Android has a Pending screen next to Requests with the same item list controls.

## API

| Request | Behavior |
|---|---|
| `POST /v2/items` with `pending: true` and optional `external_key` | Create quietly in Pending. An existing open key updates title/body/link/priority and returns HTTP 200 with the same id; a new item returns HTTP 201. |
| `GET /items?pending=1&status=open` | List open pending items. Default item lists exclude pending items. |
| `POST /v2/items/:id/pending` with `{ "pending": true }` | Move an open item into Pending. Use `false` to return it to Requests. |
| `POST /v2/items/resolve` with `{ "external_key": "KEY", "text": "optional note" }` | Resolve the open item with that key. Return HTTP 404 when no open item matches. |

Legacy `POST /items` rejects `pending` and `external_key` with HTTP 400; use `POST /v2/items` for those fields.

Items expose nullable `pending_at` and `external_key` fields. CLI readers accept older responses without them. Ordinary pushes omit the new creation fields unless supplied.

Migration `0012_pending.sql` adds these nullable columns and a unique index on non-null external keys while status is open. Apply the additive migration before the new Worker. Older clients ignore the extra response fields; they do not gain Pending controls.
