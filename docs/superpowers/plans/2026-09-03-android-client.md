# lam Android Milestone 2: Usable Client Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a private, signed Android app that pairs by QR, securely retains its device credential, synchronizes on launch/resume/manual refresh, and supports the complete approved queue workflow with a Room-backed read-only offline mode.

**Architecture:** A single Compose app uses manual dependency injection through `LamApplication`/`AppContainer`. `ItemRepository` is the only item-state entry point: it reconciles the Worker into Room and exposes `Flow` to view models. UI never calls HTTP, Room, or Keystore directly. Final-answer mutations are never blindly retried; every ambiguous response triggers reconciliation.

**Tech Stack:** JDK 17, Gradle 9.6, Android Gradle Plugin 9.4.0 with built-in Kotlin 2.3.21, compile/target SDK 36, min SDK 29, Compose BOM 2026.08.00, Material 3, Lifecycle 2.10.0, Navigation Compose 2.10.0, Room 2.8.4/KSP 2.3.11, OkHttp 5.3.0, kotlinx.serialization 1.11.0, CameraX 1.6.2, ML Kit barcode scanning 17.3.0, markdown renderer 0.41.0.

**Spec:** [`docs/superpowers/specs/2026-09-03-lam-android-app-design.md`](../specs/2026-09-03-lam-android-app-design.md)

## Global Constraints

- Start only after Milestone 1 is deployed to the development Worker.
- Use package `dev.carraes.lam`; debug uses `dev.carraes.lam.debug` so it cannot replace the trusted release app.
- Keep the release keystore and passwords outside Git. Never print them in Gradle output.
- Persist canonical item data only through Room transactions. Never retain the master token, raw QR secret after claim, or plaintext device credential.
- Disable every mutation until a successful reconciliation establishes current connectivity.
- No mobile item creation/editing/retraction, chat, attachments, remote images, multiple accounts, light theme, analytics, or crash-report SDK.
- Every gesture has a visible long-press/detail-overflow equivalent and TalkBack semantics.
- Tests use fake transports and fixed clocks; unit tests must not require a phone or network.

---

### Task 1: Bootstrap a reproducible Android application and signed build boundary

**Files:**
- Create: `android/settings.gradle.kts`
- Create: `android/build.gradle.kts`
- Create: `android/gradle.properties`
- Create: `android/gradle/wrapper/gradle-wrapper.properties`
- Create: `android/gradlew`
- Create: `android/gradlew.bat`
- Create: `android/gradle/libs.versions.toml`
- Create: `android/app/build.gradle.kts`
- Create: `android/app/proguard-rules.pro`
- Create: `android/app/src/main/AndroidManifest.xml`
- Create: `android/app/src/main/java/dev/carraes/lam/LamApplication.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/MainActivity.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/AppContainer.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/theme/Color.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/theme/Theme.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/theme/Type.kt`
- Create: `android/app/src/main/res/values/strings.xml`
- Create: `android/app/src/main/res/values/themes.xml`
- Create: `android/app/src/main/res/xml/backup_rules.xml`
- Create: `android/app/src/main/res/xml/data_extraction_rules.xml`
- Create: `android/keystore.properties.example`
- Modify: `.gitignore`

- [ ] Confirm `java -version` is JDK 17 and `ANDROID_HOME` contains API 36/build-tools 36.0.0. If the SDK is absent, install Android command-line tools and those exact packages before generating the wrapper; do not vendor the SDK into the repository.
- [ ] Create a version catalog pinning the versions in this plan. Add Compose UI/Material3, lifecycle-runtime/viewmodel-compose, navigation-compose, Room runtime/ktx/compiler, OkHttp/MockWebServer, kotlinx serialization/coroutines, CameraX camera2/lifecycle/view, ML Kit bundled barcode scanner, Custom Tabs, and the core plus Material 3 markdown artifacts. Do not add Hilt.
- [ ] Configure namespace/application ID `dev.carraes.lam`, min 29, target/compile 36, Java/Kotlin 17, Compose, Room schema export, release minification/resource shrinking, and debug suffix `.debug`.
- [ ] Add an ignored `android/keystore.properties` and `android/keys/` rule. The release signing block must fail with a direct message if the local properties file is absent; it must never fall back to the debug key.
- [ ] Generate one stable PKCS12 key at `android/keys/lam-release.p12` only if it does not already exist. Store its generated passwords only in the ignored properties file and the user's password manager. Never regenerate a missing key during an upgrade build without stopping for user confirmation.
- [ ] Exclude `credentials.xml`, `room/`, diagnostic logs, and QR data from Android backup using both backup-rule formats.
- [ ] Render one smoke-test `Requests` surface in the approved graphite/amber dark theme. Force dark theme without querying system theme.
- [ ] Run `cd android && ./gradlew testDebugUnitTest lintDebug assembleDebug`; install the debug APK with `adb install -r` and prove it opens on the SM-S928B.
- [ ] Commit: `feat(android): bootstrap private Compose app`

### Task 2: Define API models and a testable Worker client

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/items/ApiModels.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/LamApi.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/OkHttpLamApi.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/ApiError.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/items/OkHttpLamApiTest.kt`

- [ ] Write MockWebServer tests first for bearer injection, open-item listing, get, history cursor/filter encoding, choice/free reply, dismiss, set-check, device self-read/update/revoke, and pairing claim.
- [ ] Model wire fields exactly, including nullable recommendations, nullable legacy agent name, version, response metadata, checks, and `{ items, next_cursor }` history pages. Keep API DTOs separate from UI/domain models.
- [ ] Implement one suspend `LamApi` interface. Give each mutation an idempotency classification: reads may retry once after connection failure; resolve/dismiss/check mutations never retry automatically.
- [ ] Decode non-2xx responses into `Unauthorized`, `Forbidden`, `AlreadyClosed`, `Validation`, `Transport`, or `Server`. Preserve enough status/body detail for local diagnostics but never include bearer headers.
- [ ] Configure OkHttp with 10-second connect and 20-second read/write timeouts, no body logger, and an authenticator-free bearer interceptor supplied by `CredentialStore`.
- [ ] Run `./gradlew testDebugUnitTest --tests '*OkHttpLamApiTest'`.
- [ ] Commit: `feat(android): add typed lam API client`

### Task 3: Store the device credential with Android Keystore

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/security/CredentialStore.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/security/KeystoreCredentialStore.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/security/PairedServer.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/security/CredentialStoreContractTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/security/KeystoreCredentialStoreTest.kt`

- [ ] Define `CredentialStore.observe(): Flow<PairedServer?>`, `save`, and `clear`; no getter may expose the credential to UI state or diagnostics.
- [ ] Store server URL/device metadata in app-private preferences. Encrypt the credential with an AES-256-GCM key generated in `AndroidKeyStore`, storing IV+ciphertext only. Disable user authentication requirements so background FCM can authenticate after boot.
- [ ] Bind associated data to application ID and server URL so ciphertext cannot be copied between debug/release or servers.
- [ ] On key invalidation/decryption failure, erase the unreadable record and return unpaired instead of crashing.
- [ ] Test save/load/clear, process recreation, corrupted ciphertext, and debug/release namespace isolation on-device. Use an in-memory fake for JVM repository tests.
- [ ] Commit: `feat(android): secure device credentials with Keystore`

### Task 4: Add Room storage and canonical reconciliation

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/items/Item.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/ItemEntity.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/ItemDao.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/LamDatabase.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/ItemMapper.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/ItemRepository.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/DefaultItemRepository.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/items/SyncState.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/items/ItemRepositoryTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/items/ItemDaoTest.kt`

- [ ] Write repository/DAO tests first for API mapping, atomic full-open reconciliation, missing-open removal, priority/newest ordering, closed-item upsert, history append/deduplication, and cache clear on unpair.
- [ ] Store arrays as JSON through Room type converters. Use stable primary key `id`; index `status`, `priority`, `createdAt`, and effective closure time. Preserve version and all canonical response fields.
- [ ] Make this transaction the only full reconciliation path:

```kotlin
database.withTransaction {
    itemDao.upsertAll(remoteOpen.map(mapper::toEntity))
    itemDao.deleteOpenNotIn(remoteOpen.map(ItemDto::id))
    syncMetadataDao.recordSuccess(clock.instant())
}
```

- [ ] Expose `Flow<List<Item>>` for open items sorted `critical/normal/low`, then `created_at DESC`, plus item detail and cached history flows. Agent display falls back to `source_host:source_project` when `name` is blank.
- [ ] Track `Idle`, `Refreshing`, `Current(lastSuccess)`, `Stale(lastSuccess,error)`, and `Revoked`. A successful full reconciliation is the only transition that enables mutations.
- [ ] For final-answer mutations: fetch item first, reject locally if no longer open, submit once, then persist returned canonical item. On ambiguous transport failure, mark stale and reconcile before controls return.
- [ ] For check mutation: apply a tagged optimistic Room value, submit once, replace with canonical response, or roll back to the pre-write snapshot and emit a one-shot error.
- [ ] Run JVM and Room instrumentation tests.
- [ ] Commit: `feat(android): cache and reconcile lam items`

### Task 5: Implement QR pairing

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/pairing/PairingPayload.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/pairing/PairingRepository.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/pairing/PairingViewModel.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/pairing/PairingScreen.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/pairing/QrScanner.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/pairing/PairingViewModelTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/pairing/PairingScreenTest.kt`
- Modify: `android/app/src/main/AndroidManifest.xml`

- [ ] Add tests for valid v1 payload, malformed JSON, unsupported version, non-HTTPS server, expired/consumed/offline claim messages, permission denial, duplicate camera frames, successful credential persistence, and transition to Requests.
- [ ] Request camera permission only when the user taps `Scan pairing code`. Offer a visible retry/settings path after denial; do not request notification permission here.
- [ ] Bind CameraX `ImageAnalysis` to lifecycle and feed only QR formats to bundled ML Kit. Close every `ImageProxy` and debounce after the first structurally valid payload.
- [ ] Normalize server URL by stripping a trailing slash, require HTTPS except for emulator/local debug builds, and compare the claim response server/device metadata before saving.
- [ ] Submit session/secret/device model/app version/Android version. Zero references to the one-time secret after the call; save the returned permanent credential through `CredentialStore`, then reconcile.
- [ ] Run pairing JVM/instrumentation tests and a manual scan of a Milestone 1 `lam pair` QR.
- [ ] Commit: `feat(android): pair devices by terminal QR`

### Task 6: Build Requests navigation, cards, search, and filters

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/ui/LamApp.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/LamNav.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/requests/RequestsViewModel.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/requests/RequestsScreen.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/requests/RequestCard.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/components/QueueTopBar.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/components/FilterSheet.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/AgentColor.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/sync/LifecycleReconciler.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/ui/requests/RequestsViewModelTest.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/sync/LifecycleReconcilerTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/ui/requests/RequestsScreenTest.kt`

- [ ] Test empty/loading/stale/error/populated states, priority ordering, title/agent/body search, type/priority filters, deterministic agent colors, critical burgundy override, and missing-recommendation warning.
- [ ] Make Requests the start destination after pairing. The title dropdown switches Requests/History; search sits beside it; overflow contains Settings. Add no bottom bar.
- [ ] Cards show title, agent, age, priority, choice count or check progress, and warning state. They never show a body preview. Use compact radii and the muted palette from the approved visual direction.
- [ ] Use a stable non-cryptographic hash of agent display name to select from a fixed accessible palette. Never rely on card color alone for priority or type.
- [ ] Pull-to-refresh calls full reconciliation. Stale mode shows last-success time and keeps navigation/search readable while clearly disabling actions.
- [ ] Observe process foreground transitions with `ProcessLifecycleOwner`: reconcile at paired launch and every return from background, coalesce simultaneous triggers, and never keep a periodic background poll. Test launch/resume success, failure, and trigger coalescing with a fake lifecycle.
- [ ] Add TalkBack descriptions that read title, agent, priority, type/progress, and available actions in that order.
- [ ] Commit: `feat(android): add mobile-first Requests queue`

### Task 7: Build decision detail, markdown, confirmations, and links

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/ui/detail/DecisionDetailScreen.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/detail/DecisionViewModel.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/detail/RecommendationCard.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/detail/ReplySheet.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/markdown/LamMarkdown.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/ui/detail/DecisionViewModelTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/ui/detail/DecisionDetailScreenTest.kt`

- [ ] Test recommendation-before-body order, recommended-choice marker, legacy warning, choice confirmation, multiline free reply, 8 KiB validation, double-submit suppression, ambiguous failure reconciliation, canonical concurrent closure, and disabled controls while stale.
- [ ] Render metadata, restrained amber recommendation callout, markdown body, then choices/free reply in that exact order.
- [ ] Configure markdown for headings/emphasis/lists/tables/fenced+inline code/links. Render raw HTML as text, provide no image transformer, and intercept links to show their destination before opening an Android Custom Tab.
- [ ] Every choice opens `Send "<choice>"?`; recommended and non-recommended choices use the same confirmation. Free reply always resolves and never appears as a chat message.
- [ ] If Room reports remote closure while detail is visible, retain the page, remove controls, and show canonical status/responder/answer.
- [ ] Add representative snapshot/semantics tests for long markdown, tables, code, large font scale, and RTL-safe layout.
- [ ] Commit: `feat(android): add decision detail and replies`

### Task 8: Add checklist interactions, quick response, and dismissal gestures

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/ui/detail/ChecklistDetailScreen.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/detail/ChecklistViewModel.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/requests/QuickResponseSheet.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/requests/DismissConfirmation.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/ui/detail/ChecklistViewModelTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/ui/requests/RequestGesturesTest.kt`

- [ ] Test optimistic tick/untick, rollback, final-check closure, scroll-position preservation, choice/plain/checklist quick-response variants, swipe thresholds, cancellation, dismissal confirmation, and non-gesture equivalents.
- [ ] Give each check at least a 48 dp target. Preserve canonical server order and never reorder after a state update.
- [ ] Swipe right reveals a dismiss affordance and then confirmation; it does not dismiss directly. Swipe left opens the context-aware quick-response sheet. Long press and detail overflow expose both actions.
- [ ] Quick response offers choices plus free reply, a focused plain reply, or remaining checks plus `Reply instead` based on item type. Choice confirmation remains mandatory.
- [ ] Ensure simultaneous remote closure closes the sheet, keeps detail state canonical, and never resubmits.
- [ ] Commit: `feat(android): add checklists and queue gestures`

### Task 9: Add History, Settings, offline state, and unpairing

**Files:**
- Create: `android/app/src/main/java/dev/carraes/lam/ui/history/HistoryViewModel.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/history/HistoryScreen.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/settings/SettingsViewModel.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/ui/settings/SettingsScreen.kt`
- Create: `android/app/src/main/java/dev/carraes/lam/diagnostics/Diagnostics.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/ui/history/HistoryViewModelTest.kt`
- Create: `android/app/src/androidTest/java/dev/carraes/lam/ui/settings/SettingsScreenTest.kt`

- [ ] Test cursor pagination with identical close timestamps, independent server-side history search, filters, inert history rows, cached offline reading, offline mutation suppression, diagnostic redaction, and unpair cleanup.
- [ ] History displays status/responder/answer/effective closure time newest-first. It never exposes action controls or reopen behavior.
- [ ] Keep Requests search local over the complete open cache. Send History query/priority/type to `/history`; replace results when the query changes and append by opaque cursor otherwise.
- [ ] Settings displays device name/ID, server, last successful sync, connection state, notification permission and application-notification Settings link, app version, unpair, and copy diagnostics. Milestone 3 adds channel-specific links after creating channels. It never displays credential or FCM token.
- [ ] `Unpair this device` confirms, calls self-revoke once, and then clears credentials, Room, in-memory API clients, and local state. If the call is unavailable, explain that local unpair cannot revoke the remote credential and require an explicit second confirmation before local erase.
- [ ] Diagnostics include app/Android/device versions, server origin, last sync, database item counts, and recent redacted error categories only.
- [ ] Run all JVM/instrumentation tests, lint, debug build, and signed release build. Manually enable airplane mode and prove cached navigation works while controls remain disabled.
- [ ] Commit: `feat(android): complete history settings and offline mode`

## Milestone Exit

- `cd android && ./gradlew testDebugUnitTest lintDebug assembleDebug assembleRelease` passes.
- QR pairing, launch/resume/manual synchronization, all decision actions, checklists, history, search, Settings, and offline reading work on the S24 Ultra.
- Release APK is signed by the stable private key and upgrades with `adb install -r` without losing Room or pairing.
- Push and foreground live events are intentionally absent until Milestone 3; opening/resuming/refreshing the app is already useful.
