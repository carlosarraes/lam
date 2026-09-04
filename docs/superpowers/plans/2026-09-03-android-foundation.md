# lam Android Milestone 1: Contract and Device Foundation Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the existing Worker and Rust CLI with recommendation-aware items, complete duplicate identity, scoped Android-device credentials, and one-use QR pairing without breaking deployed clients.

**Architecture:** D1 remains canonical. Existing master-bearer routes continue to behave as they do today. Authentication becomes an explicit `master | device` authority, with route-level capability checks. Pairing exchanges a five-minute secret for a device-specific credential; only HMAC digests are stored. Item schemas tolerate legacy null recommendations, while the new CLI rejects new non-checklist pushes that omit them.

**Tech Stack:** TypeScript 7, Effect 3, Cloudflare Workers/D1, Vitest Workers pool, Rust 2021, Clap 4, Reqwest 0.12, `qrcode` 0.14.

**Spec:** [`docs/superpowers/specs/2026-09-03-lam-android-app-design.md`](../specs/2026-09-03-lam-android-app-design.md)

## Global Constraints

- Preserve existing `/items`, phone, ntfy, `lam watch`, and master-token behavior.
- Apply additive migrations before deploying code that reads new columns.
- The API accepts nullable recommendation fields during rollout; only the upgraded CLI enforces them.
- Never return or log credential digests, raw device credentials after claim, FCM tokens, request bodies, or the master token.
- Use HMAC-SHA-256 under `LAM_HMAC_SECRET`; compare fixed-length byte arrays without early exit.
- A device bearer can read and respond, but cannot push, retract, pair, or administer other devices.
- Keep commits scoped to the task that produced them. Run `just check` after every Worker/Rust boundary change.

---

### Task 1: Add recommendation fields, content limits, and complete duplicate identity

**Files:**
- Create: `worker/migrations/0006_item_recommendations.sql`
- Modify: `worker/src/domain/Item.ts`
- Modify: `worker/src/services/Items.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing API tests for: legacy push with absent recommendation; recommendation round-trip; choices/check/title/body/recommendation/reply limits; mismatched recommendation creating a distinct item; fully identical retries returning the first item; precise TTL seconds participating in identity.
- [ ] Add this additive migration:

```sql
ALTER TABLE items ADD COLUMN recommendation TEXT;
ALTER TABLE items ADD COLUMN recommended_choice TEXT;
ALTER TABLE items ADD COLUMN dedupe_key TEXT;
CREATE INDEX items_open_dedupe ON items(dedupe_key, created_at DESC)
  WHERE status = 'open' AND dedupe_key IS NOT NULL;
```

- [ ] In `domain/Item.ts`, add `recommendation` and `recommended_choice` as `Schema.NullOr(Schema.String)` on `Item`, optional nullable counterparts on `NewItem`, and exported constants for every approved limit. Use a code-point-count predicate for character limits, a UTF-8 byte predicate for the 64 KiB body and 8 KiB reply limits, and `Schema.maxItems` for arrays. Keep choices and checks mutually exclusive and reject `recommended_choice` unless it exactly matches a choice.
- [ ] Introduce an internal row schema that accepts nullable `dedupe_key` without exposing it in `Item`. Map the row field explicitly instead of passing storage-only data to `new Item(...)`.
- [ ] Add `canonicalDedupeInput(input)` with fixed object-key order and exact array order. Normalize optional API fields to their stored values (`""`, `[]`, or `null`) and include the submitted TTL as an exact integer number of seconds.
- [ ] Hash the UTF-8 canonical JSON with SHA-256 and encode it as lowercase hex. `Items.create` computes one timestamp, derives `expires_at`, and inserts both recommendation fields plus `dedupe_key` in the same statement.
- [ ] Change `Items.findDuplicate` to compute/query `dedupe_key` and independently require the stored item to remain unexpired. Only when no keyed row matches, query an unexpired open legacy row whose `dedupe_key IS NULL` by the old `name/title/body` identity.
- [ ] Run `cd worker && npx tsc -p . && npx vitest run`; confirm the new tests fail before implementation and all tests pass after it.
- [ ] Commit: `feat(worker): add item recommendations and complete deduplication`

### Task 2: Enforce recommendations in the upgraded CLI and agent guide

**Files:**
- Modify: `cli/src/client.rs`
- Modify: `cli/src/main.rs`
- Modify: `cli/src/commands.rs`
- Modify: `cli/tests/cli.rs`
- Modify: `skill/lam/SKILL.md`
- Modify: `README.md`

- [ ] Add failing Rust unit/integration cases covering: plain request without recommendation rejected; choice request without either recommendation flag rejected; exact recommended choice accepted; non-member recommended choice rejected; checklist accepted without either field; field-length errors are local and readable.
- [ ] Add nullable `recommendation` and `recommended_choice` to `client::Item` with `#[serde(default)]`, and optional serialized fields to `client::NewItem`.
- [ ] Add `--recommendation <TEXT>` and `--recommended-choice <CHOICE>` to `Cmd::Push`, thread them through `PushArgs`, and validate before loading config or touching the network:

```rust
if a.checks.is_empty() && a.recommendation.as_deref().is_none_or(str::is_empty) {
    bail!("--recommendation is required for every non-checklist request");
}
if !a.choices.is_empty() {
    let recommended = a.recommended_choice.as_deref()
        .context("--recommended-choice is required when --choice is used")?;
    if !a.choices.iter().any(|choice| choice == recommended) {
        bail!("--recommended-choice must exactly match one --choice");
    }
}
```

- [ ] Reject recommendation flags on checklists, and reject `--recommended-choice` when no choices exist. Apply the approved character/byte/count limits in one validation helper so CLI errors match Worker errors.
- [ ] Update the skill examples and `lam --llm` prose: every decision request states the recommended action and rationale; checklist pushes are the only exception. Include a choice example with both flags.
- [ ] Update README command examples without removing legacy compatibility notes.
- [ ] Run `cd cli && cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`.
- [ ] Commit: `feat(cli): require recommendations for decision requests`

### Task 3: Introduce typed master and device authentication

**Files:**
- Create: `worker/migrations/0007_devices.sql`
- Create: `worker/src/domain/Device.ts`
- Create: `worker/src/services/Devices.ts`
- Modify: `worker/src/services/Auth.ts`
- Modify: `worker/src/http/api.ts`
- Modify: `worker/src/http/app.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing tests proving a master bearer still reaches every existing API route, an unknown device bearer gets `401`, a valid device can list/get/resolve/dismiss/toggle checks, and a device gets `403` from push/retract/add-check.
- [ ] Add `devices` with columns from the spec, a unique `credential_hash`, and indexes on `revoked_at` and `fcm_token`. Do not use deletion for revocation.
- [ ] Define public `Device`, owner-visible `DeviceSummary`, self-visible `DeviceRegistration`, and storage row schemas. Public shapes expose `push_registered: boolean`, never `credential_hash` or `fcm_token`.
- [ ] Implement `Devices.authenticate(digest)`, `get`, `list`, `rename`, `updateSelf`, `revoke`, and `revokeSelf`. Authentication reads the small active-device digest set and uses `constantTimeEqual` for every candidate rather than relying on an early-exit string comparison. Every successful device-authenticated request updates `last_seen_at` at most once per five-minute bucket to avoid a D1 write per read.
- [ ] Export reusable `hmacDigest(secret)` and byte-wise `constantTimeEqual` from `Auth.ts`. Define:

```ts
export type Authority =
  | { readonly kind: "master" }
  | { readonly kind: "device"; readonly device: Device };
```

`authenticateBearer` returns that authority. `requireMaster` and `requireDevice` narrow it and raise `Forbidden` for a valid but insufficient bearer.
- [ ] Split API middleware so authentication attaches `Authority` to request context and handlers perform their own capability checks. Mark item creation, retraction, and check addition master-only; reads and human response routes accept both. Map device responses to `response_by: "phone"` to preserve the existing wire enum.
- [ ] Register `Devices.Default` in `index.ts`, run Worker type/tests, and confirm existing phone/ntfy tests are unchanged.
- [ ] Commit: `feat(worker): add scoped device authentication`

### Task 4: Implement atomic one-use pairing

**Files:**
- Create: `worker/migrations/0008_pairing_sessions.sql`
- Create: `worker/src/domain/Pairing.ts`
- Create: `worker/src/services/Pairings.ts`
- Create: `worker/src/http/pairings.ts`
- Modify: `worker/src/http/app.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing tests for five-minute expiry, malformed secret, one-use consumption, concurrent double claim with exactly one winner, wrong session, cancellation, and a wait response for claimed/expired/pending.
- [ ] Add `pairing_sessions(id, secret_hash, created_at, expires_at, consumed_at, device_id)` with indexes for unconsumed expiry cleanup and `device_id`.
- [ ] Generate 32 random bytes for both pairing secret and permanent device credential. Encode base64url without padding. Persist only HMAC digests.
- [ ] Define QR payload version 1 as compact JSON and test its exact keys:

```json
{"v":1,"server":"https://lam.example","session":"...","secret":"..."}
```

- [ ] Implement `Pairings.create(origin)`, `status(id)`, `cancel(id)`, and `claim(id, secret, registration)`. Claim runs one atomic D1 batch: `INSERT INTO devices ... SELECT ... WHERE EXISTS` an unconsumed/unexpired matching session, then a guarded `UPDATE pairing_sessions SET consumed_at = ?, device_id = ?`. Inspect both batch results and return the generated raw credential only when each changed one row. Concurrent losing claims create no device and return `409`.
- [ ] Mount master-only `POST /pairings`, `GET /pairings/:id/wait`, and `DELETE /pairings/:id`. Mount unauthenticated `POST /pairings/:id/claim`; authenticate the body secret inside the service before accepting device metadata.
- [ ] Bound wait at 25 seconds so the CLI can poll until the five-minute deadline. Return typed statuses `pending`, `claimed`, `expired`, `cancelled`; never echo the raw secret.
- [ ] Register the service/router and run `cd worker && npx tsc -p . && npx vitest run`.
- [ ] Commit: `feat(worker): add one-use device pairing`

### Task 5: Add device administration and self-service routes

**Files:**
- Create: `worker/src/http/devices.ts`
- Modify: `worker/src/http/app.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing capability tests for every route in the approved API table, including cross-device access attempts and revoked-device `401` behavior.
- [ ] Implement master-only `GET /devices`, `PATCH /devices/:id`, and `DELETE /devices/:id`.
- [ ] Implement device-only `GET /device`, `PATCH /device`, and `DELETE /device`. Permit only `name`, `fcm_token`, `app_version`, and `android_version` updates; trim names and enforce a 100-character device-name limit.
- [ ] Make revocation idempotent for the master and self endpoints. Clear `fcm_token` when revoking so delivery queries cannot include the device.
- [ ] Verify response bodies contain neither sensitive storage fields nor raw FCM tokens.
- [ ] Run Worker checks and commit: `feat(worker): add device management routes`

### Task 6: Add canonical paginated History

**Files:**
- Modify: `worker/src/services/Items.ts`
- Modify: `worker/src/http/api.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing tests for newest-first effective closure time, expired rows using `expires_at`, explicitly closed rows using `resolved_at`, identical-time ID tie-breaking, opaque cursor continuation without skips/duplicates, title/name/body/legacy-source search, priority/type filters, and master/device access.
- [ ] Add `HistoryQuery` and `HistoryPage` types. Determine request type as `checklist` when checks are non-empty, `choice` when choices are non-empty, otherwise `plain`.
- [ ] Query only closed or effectively expired rows. Use `COALESCE(resolved_at, expires_at)` as `closed_at`, order by `closed_at DESC, id DESC`, and page with the strict tuple condition `closed_at < ? OR (closed_at = ? AND id < ?)`.
- [ ] Encode/decode the cursor as base64url JSON `{ "closed_at": "...", "id": "..." }`; malformed cursors return `400`. Fetch `limit + 1`, return at most `limit`, and derive `next_cursor` only when the extra row exists.
- [ ] Apply `q`, `priority`, and `type` filters in SQL before pagination. Escape `%`, `_`, and the escape character for literal case-insensitive `LIKE`; search title, name, body, and the concatenated legacy `source_host:source_project` fallback.
- [ ] Mount `GET /history` for master and device authority without changing `/items` response shape. Return `{ items, next_cursor }` and decode derived expiry status before serialization.
- [ ] Run Worker type/tests and commit: `feat(worker): add canonical paginated history`

### Task 7: Add QR pairing and device commands to the Rust CLI

**Files:**
- Modify: `cli/Cargo.toml`
- Modify: `cli/Cargo.lock`
- Modify: `cli/src/client.rs`
- Modify: `cli/src/main.rs`
- Modify: `cli/src/commands.rs`
- Modify: `cli/tests/cli.rs`

- [ ] Add `qrcode = { version = "0.14", default-features = false }` plus `ctrlc = "3"`, and failing tests for the serialized QR payload, terminal Unicode rendering, pending/claimed/expired/cancelled wait outcomes, device list output, rename request, and revoke confirmation behavior.
- [ ] Add typed client methods for pairing create/wait/cancel and device list/rename/revoke. Add `patch` and `delete` request builders rather than constructing raw Reqwest calls in commands.
- [ ] Add commands exactly as approved:

```text
lam pair
lam devices
lam device rename <id> <name>
lam device revoke <id>
```

- [ ] `lam pair` renders a quiet-zone QR with Unicode half blocks, prints the server and five-minute expiry in text, polls the wait endpoint, handles Ctrl-C by cancelling best-effort, and reports the claimed device name/ID. It must not print the QR JSON or secret as plain text.
- [ ] `lam devices` prints ID, name, Android/app version, creation, last contact, push registration, and revocation state. Rename/revoke print the returned safe device summary.
- [ ] Run Rust fmt, clippy, unit tests, and CLI integration tests.
- [ ] Commit: `feat(cli): pair and manage Android devices`

### Task 8: Prove compatibility and migration safety

**Files:**
- Modify: `worker/test/api.test.ts`
- Modify: `cli/tests/cli.rs`
- Modify: `README.md`

- [ ] Add one compatibility test that sends the pre-milestone `NewItem` JSON and decodes the response with the pre-milestone Rust shape fixture.
- [ ] Add a migration test that inserts a `0005`-shape row before applying migrations `0006`–`0008`, then proves it decodes with null recommendations and participates in legacy deduplication.
- [ ] Document deploy order: migrations, Worker, CLI/skill. Include rollback behavior: old Worker ignores additive tables/columns; old CLI remains accepted by the new Worker.
- [ ] Run `just check` and record the passing Worker/Rust test counts in the commit message body.
- [ ] Commit: `test: prove Android foundation rollout compatibility`

## Milestone Exit

- `just check` passes.
- Existing ntfy, phone, CLI, TUI, and wait behavior remains green.
- The upgraded CLI enforces recommendations and prints a one-use QR.
- A claimed device bearer can read/respond but cannot perform master operations.
- No Android source exists yet; this milestone is independently deployable and reversible.
