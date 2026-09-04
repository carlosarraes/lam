# lam Android Milestone 3: Live Sync and Push Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add foreground WebSocket invalidations and reliable Android background notifications over FCM while preserving D1 as the only source of item truth.

**Architecture:** Every committed item mutation schedules three independent best-effort deliveries: the existing ntfy notification, a tiny structured event through a dedicated Durable Object, and one FCM data message per active registered device. Android reconciles before opening its foreground socket, treats events as invalidations, and uses stable tagged system notifications. Missed, duplicate, reordered, or failed signals are repaired by canonical reconciliation.

**Tech Stack:** Existing Effect Worker/D1 stack, Cloudflare Durable Objects, FCM HTTP v1 OAuth 2 service-account flow, Firebase Android BOM 34.18.0, Google Services plugin 4.5.0, WorkManager, OkHttp WebSocket, Android notification APIs.

**Spec:** [`docs/superpowers/specs/2026-09-03-lam-android-app-design.md`](../specs/2026-09-03-lam-android-app-design.md)

## Global Constraints

- Start after Milestones 1 and 2 pass independently.
- Events and FCM never carry authoritative bodies, choices, checks, recommendations, or response outcomes.
- Publish only after a successful D1 mutation. Publication failure never changes the HTTP mutation result.
- Reconcile at launch, foreground resume, socket reconnect, manual refresh, and before enabling any response from a deep link.
- Critical and normal FCM sends use high delivery priority because they produce visible notifications; low uses normal priority.
- No test contacts Google or depends on live push delivery. Use injectable event and FCM transports.
- Never log OAuth assertions/tokens, service-account JSON, device bearers, FCM registration tokens, or request content.

---

### Task 1: Add a dedicated structured-event Durable Object

**Files:**
- Create: `worker/src/domain/Event.ts`
- Create: `worker/src/events/stream.ts`
- Create: `worker/src/services/Events.ts`
- Create: `worker/src/http/events.ts`
- Modify: `worker/src/Env.ts`
- Modify: `worker/src/http/app.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/wrangler.jsonc`
- Modify: `worker/worker-configuration.d.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing tests for device-only socket upgrade, master/unknown rejection, initial connection, broadcast to two subscribers, closed-socket cleanup, and exact `item.created/item.changed/item.closed` schemas.
- [ ] Define the invalidation shape as `{ event, item_id, version, status }` and reject any payload containing item content. Keep event names a closed Effect schema.
- [ ] Implement `EventStream` as a separate Durable Object that accepts internal publish requests and WebSocket upgrades, sends JSON text frames, responds to ping/pong, and removes failed peers. It stores no item state and offers no replay.
- [ ] Add `EVENTS: DurableObjectNamespace<EventStream>` to bindings. Route every socket to `EVENTS.idFromName("global")`; authenticate the device bearer in the Worker before forwarding the upgrade.
- [ ] Add a new Wrangler Durable Object binding and migration tag without modifying `TOPIC`:

```json
{"tag":"v2","new_sqlite_classes":["EventStream"]}
```

- [ ] Implement `Events.publish(event)` against the global object. Register `Events.Default`, export `EventStream`, regenerate Worker types, and run TypeScript/Vitest.
- [ ] Commit: `feat(worker): add structured item event stream`

### Task 2: Publish structured invalidations after every committed mutation

**Files:**
- Create: `worker/src/services/Delivery.ts`
- Modify: `worker/src/http/api.ts`
- Modify: `worker/src/http/phone.ts`
- Modify: `worker/src/services/Notify.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Add failing tests proving create emits `item.created`, check/add-check emits `item.changed` unless it auto-closes, and resolve/dismiss/retract/final-check emits `item.closed`; failed/duplicate/no-op mutations emit nothing.
- [ ] Introduce a `Delivery` service that receives the committed `Item` and dispatches existing ntfy plus structured events through separate `Effect.ignoreLogged` branches. Preserve the existing phone URL behavior for ntfy.
- [ ] Replace route-local notification calls with one `background(delivery.created/changed/closed(...))` after the D1 call returns. Include phone-page mutations so TUI/phone/Android all converge.
- [ ] Confirm event publication failure is visible in redacted Worker logs but the mutation response remains successful.
- [ ] Run all Worker tests and commit: `feat(worker): publish canonical item invalidations`

### Task 3: Add foreground Android WebSocket synchronization

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/sync/ItemEvent.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/sync/EventSocket.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/sync/OkHttpEventSocket.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/sync/ForegroundSync.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/sync/ForegroundSyncTest.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/MainActivity.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/AppContainer.kt`

- [ ] Write deterministic tests with fake lifecycle/socket/clock for initial reconcile-before-connect, targeted refresh, duplicate/older versions ignored, out-of-order close, socket failure, background stop, full reconnect reconciliation, capped backoff+jitter, and manual-refresh delay bypass.
- [ ] Convert the paired HTTPS origin to WSS and attach the device bearer only in the WebSocket upgrade header. Decode only the closed event schema; malformed frames are logged by category and ignored.
- [ ] While the process is foregrounded: full reconcile, then connect. On an event newer than Room, fetch that item and persist canonical state. On a socket disconnect, mark live state `Reconnecting` but do not disable actions unless canonical HTTP connectivity is also stale.
- [ ] Use exponential delays of 1, 2, 4, 8, 16, then 30 seconds, each with ±20% seeded jitter in tests. Cancel socket and retry job when the process backgrounds; repeat full reconciliation on return.
- [ ] Expose `Connected/Reconnecting/Offline` to Settings and a subtle queue status, without showing transient toast spam.
- [ ] Commit: `feat(android): synchronize foreground queue over WebSocket`

### Task 4: Provision Firebase without committing server authority

**Files:**
- Modify: `.gitignore`
- Modify: `android/build.gradle.kts`
- Modify: `android/app/build.gradle.kts`
- Add after provisioning: `android/app/google-services.json`
- Create: `docs/android-firebase.md`

- [ ] At execution time, invoke the `wizard` skill for the human-only Firebase/Google Cloud steps. Create/select one Firebase project, register release app `dev.carraes.lam` and debug app `dev.carraes.lam.debug`, enable Cloud Messaging API v1, and create a least-purpose service account allowed to send FCM messages.
- [ ] Verify both app IDs are present in the downloaded `google-services.json`. This client configuration is not server authority and may be committed; inspect it first to ensure it contains no private key.
- [ ] Store the service-account JSON only with `npx wrangler secret put FCM_SERVICE_ACCOUNT_JSON`; store the Firebase project ID as non-secret `FCM_PROJECT_ID` configuration. Do not save the service-account download under the repository.
- [ ] Add the Google Services plugin and Firebase BOM/messaging dependency. Build debug/release and confirm generated Firebase resources use the intended application IDs.
- [ ] Document rotation/removal steps and exact Wrangler secret names. Do not paste secret values into the document or shell history.
- [ ] Commit: `build(android): configure Firebase messaging clients`

### Task 5: Implement injectable FCM HTTP v1 delivery in the Worker

**Files:**
- Create: `worker/src/domain/PushMessage.ts`
- Create: `worker/src/services/FcmAuth.ts`
- Create: `worker/src/services/Push.ts`
- Modify: `worker/src/services/Devices.ts`
- Modify: `worker/src/services/Delivery.ts`
- Modify: `worker/src/Env.ts`
- Modify: `worker/src/index.ts`
- Modify: `worker/test/api.test.ts`

- [ ] Write fake-transport tests first for create/change/close payloads, safe fields only, open count, high/normal priority mapping, one send per active registered device, revoked/unregistered exclusion, OAuth cache/refresh, transient failure logging, and permanent invalid-token clearing.
- [ ] Parse the service-account JSON once per isolate. Import its PKCS#8 RSA key, create a signed JWT assertion for `https://www.googleapis.com/auth/firebase.messaging`, exchange it at Google's token endpoint, and cache the access token until 60 seconds before expiry.
- [ ] Define the data-only payload with string values: event, item ID, version, status, agent display name, priority, notification-safe title, and canonical open count. Do not include body, recommendation, checks, choices, or final answer.
- [ ] POST to `https://fcm.googleapis.com/v1/projects/{project}/messages:send`. Set Android priority `HIGH` for critical/normal and `NORMAL` for low. Let Android build the visible notification.
- [ ] Classify `UNREGISTERED` and permanent invalid-registration responses; atomically clear only the matching stale token so a concurrent token rotation is not erased. Retry no push inside the mutation request.
- [ ] Extend `Delivery` so ntfy, event, and FCM are independently isolated. One failure must not suppress the other transports.
- [ ] Run Worker tests and commit: `feat(worker): deliver item invalidations through FCM`

### Task 6: Register FCM tokens and interpret incoming data messages

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/sync/LamFirebaseMessagingService.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/sync/PushEventParser.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/sync/PushRefreshWorker.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/sync/PushEventParserTest.kt`
- Modify: `android/app/src/main/AndroidManifest.xml`
- Modify: `android/app/src/main/java/dev/carraes/lam/AppContainer.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/pairing/PairingRepository.kt`

- [ ] Test malformed/missing fields, older version suppression, open/change fetch, close cancellation, token rotation upload, unpaired delivery ignore, and Worker `401` cleanup.
- [ ] Include the current FCM token in pairing claim when available. In `onNewToken`, persist only a pending-update marker locally; upload through the authenticated device route as soon as paired/connectable.
- [ ] Parse FCM data without trusting its content as canonical. For close, cancel promptly and schedule reconciliation. For create/change, enqueue one uniquely named `PushRefreshWorker` per item with replacement/coalescing so duplicate events do not fan out network work.
- [ ] The worker fetches the canonical item before changing Room or enabling a deep link. If execution time is constrained, leave the signal pending for launch reconciliation rather than posting stale controls.
- [ ] On `401`, use the single revocation handler from Task 8: clear credential/cache/notifications and return to pairing.
- [ ] Commit: `feat(android): receive and reconcile FCM events`

### Task 7: Post private, stable Android notifications

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/notifications/NotificationChannels.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/notifications/LamNotifications.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/notifications/NotificationDeepLink.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/notifications/NotificationPermission.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/notifications/NotificationDeepLinkTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/notifications/LamNotificationsTest.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/sync/PushRefreshWorker.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/ui/settings/SettingsScreen.kt`
- Modify: `android/app/src/main/AndroidManifest.xml`

- [ ] Add tests for channel importance, item tag stability, group summary/open count, lock-screen public version, deep link, update-in-place, close cancellation, all-notification reconciliation, and denied permission behavior.
- [ ] Create immutable channel IDs `lam.low`, `lam.normal`, `lam.critical` with low/default/high importance respectively. Names explain that Android Settings owns sound/vibration after creation.
- [ ] Notify with `(tag = "item:${item.id}", id = 1)` so each item is collision-free and updates/cancels stably across processes. Group under `dev.carraes.lam.REQUESTS`; use the canonical open count for summary/badge.
- [ ] Set the private notification visibility/public version to generic `New lam request` plus agent only. Use the canonical title in unlocked content, never the body. Add no inline resolve/dismiss actions.
- [ ] Deep links contain item ID only. On tap, launch paired navigation, reconcile/fetch the item, then show open controls or the canonical closed outcome.
- [ ] Ask `POST_NOTIFICATIONS` once in context after successful pairing, with a pre-prompt explanation. If denied, continue normally and expose the application/channel Settings intents.
- [ ] Reconcile all tagged lam notifications against Room after every full sync; cancel tags for items no longer open.
- [ ] Commit: `feat(android): add private grouped request notifications`

### Task 8: Centralize revocation and concurrent-state recovery

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/security/RevocationHandler.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/security/RevocationHandlerTest.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/items/DefaultItemRepository.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/sync/ForegroundSync.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/sync/PushRefreshWorker.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/ui/LamApp.kt`

- [ ] Write a concurrency test proving simultaneous HTTP/socket/FCM `401` signals perform one cleanup and never recreate a stale API client.
- [ ] Under a process-wide mutex: stop foreground sync, cancel push work, cancel all lam notifications, close Room, delete Room files, clear credentials, reset in-memory container state, and route to pairing with `This device was revoked`.
- [ ] Keep local manual unpair distinct: it attempts server self-revoke first; an authoritative `401` always performs immediate local erasure.
- [ ] Add repository tests for another client closing the current detail between fetch and mutation. Treat `409` as canonical-state recovery, not a retryable error.
- [ ] Commit: `fix(android): converge revoked and concurrent client state`

### Task 9: Exercise the complete delivery loop

**Files:**
- Create: `worker/test/delivery.test.ts`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/DeliveryFlowTest.kt`
- Modify: `docs/android-firebase.md`

- [ ] Run a fake-transport Worker integration matrix for every mutation source (CLI/master, device, phone) and destination (ntfy, socket, FCM), asserting delivery happens after persistence.
- [ ] Run Android instrumentation with fake API/socket/FCM adapters for create, update, close, duplicate, reorder, offline, token rotation, and revoke.
- [ ] Against local Wrangler plus the connected S24 Ultra, prove foreground propagation both directions in under two seconds and record the commands/results in the document.
- [ ] Against deployed FCM, create one low, normal, and critical request while the phone is on mobile data with the app backgrounded. Record notification behavior and correction on launch.
- [ ] Force-stop the app, push an item, verify no false promise is made, relaunch, and prove reconciliation restores it.
- [ ] Run `just check` plus all Android unit/lint/build/instrumentation tests.
- [ ] Commit: `test: verify Android live sync and push delivery`

## Milestone Exit

- Foreground Android/TUI state converges bidirectionally in under two seconds in the acceptance run.
- Background notifications arrive over mobile data with the approved priority/privacy behavior.
- Duplicate, reordered, or missed delivery does not corrupt Room or D1 state.
- Revocation clears local sensitive state at next contact.
- Existing ntfy delivery remains active and independently tested.
