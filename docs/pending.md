# Pending

Pending holds open items Carlos parks for later from the TUI (`p`) or the Android Pending screen. It is a separate lane, stored as a nullable `pending_at` timestamp. Status stays `open`; moving an item preserves its kind, name, link and other origin fields. Closed items retain their normal status values.

Agents send a request (default) or `--kind fyi` when Carlos should see an item now. Creating items as pending and changing lanes from the CLI are rejected with:

```text
pending is Carlos-only: send a request (default) or --kind fyi instead
```

## Agent commands

```bash
lam pending list
lam pending list --json
lam pending resolve --key github:org/repo:pr:42 -m "Merged"
lam pending resolve ITEM_ID -m "Finished"
```

List is read-only and requests only open pending items. Resolve lets an agent close its own old tracker when the world has already resolved it. It accepts exactly one id or `--key KEY`, plus an optional `-m NOTE`, and prints the updated item as JSON. A missing open external key returns an error.

`--key` remains a stable dedupe key on ordinary pushes. Repeating an open key updates title, body, link and priority and returns the same id, preserving the item's lane and origin. Once that item closes, the key can identify a new item.

Default item lists exclude pending items. Pending updates and closing do not alert or ring the bell. Lane changes do not alert. Moving a critical item into Pending sends an FCM invalidation to cancel its existing phone notification while keeping the item open. Moving it back does not re-alert.

## TUI and Android

Pending follows Requests in the TUI. Press `p` on an open item to toggle its lane. `h`/`l` cycle the tabs; `Ctrl+1`, `Ctrl+2` and `Ctrl+3` select Requests, Pending and History. Android has a Pending screen next to Requests with the same item list controls.

## API

| Request | Behavior |
|---|---|
| `POST /v2/items` with `pending: true` | Reject with HTTP 400 and the Carlos-only message above, including when an external key already exists. |
| `POST /v2/items` without `pending` or with `pending: false` | Create a request or FYI outside Pending. An existing open external key updates title/body/link/priority and returns HTTP 200 with the same id and lane; a new item returns HTTP 201. |
| `GET /items?pending=1&status=open` | List open pending items. Default item lists exclude pending items. |
| `POST /v2/items/:id/pending` with `{ "pending": true }` | Move an open item into Pending. Use `false` to return it to Requests. Both master and paired-device bearers are accepted for the TUI and Android. |
| `POST /v2/items/resolve` with `{ "external_key": "KEY", "text": "optional note" }` | Resolve the open item with that key. Return HTTP 404 when no open item matches. |

Legacy `POST /items` rejects `pending` and `external_key` with HTTP 400. Use `POST /v2/items` for external keys; creation as pending is rejected on both routes.

Items expose nullable `pending_at` and `external_key` fields. CLI readers accept older responses without them. Ordinary pushes omit these creation fields unless supplied.

The existing `0012_pending.sql` migration provides these nullable columns and a unique index on non-null external keys while status is open. This guard needs no schema or migration change and leaves existing pending items intact.
