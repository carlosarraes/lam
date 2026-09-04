# lam Android Milestone 4: TUI Parity, Polish, and Rollout Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish recommendation parity in the TUI, install the approved app identity, close accessibility/diagnostic gaps, automate safe ADB builds and tests, and complete a recorded parallel ntfy rollout on the S24 Ultra.

**Architecture:** This milestone adds no new canonical state or delivery path. It aligns the terminal presentation with Android, hardens the private release workflow, and treats the acceptance matrix as the release gate. ntfy remains active throughout the trial and rollback remains uninstalling/downgrading only the Android client.

**Tech Stack:** Existing Rust/Ratatui CLI, Android Compose project from Milestones 2–3, Gradle/ADB, shell/Just tasks, Android accessibility and notification tooling.

**Spec:** [`docs/superpowers/specs/2026-09-03-lam-android-app-design.md`](../specs/2026-09-03-lam-android-app-design.md)

## Global Constraints

- Start only when Milestones 1–3 are green and the development Worker is reachable.
- Preserve every existing TUI key, selection-retention rule, history behavior, and ntfy/phone behavior unless this plan explicitly changes it.
- Do not put signing material, bearer tokens, device credentials, Firebase service-account material, phone screenshots, or live request bodies in Git.
- `just check` stays deterministic and phone-free. Connected-device checks live under explicit Android recipes.
- `just android-install` upgrades in place and refuses an unexpected target device by default; it never uninstalls first.
- Release is not complete until the manual mobile-data and accessibility cases have recorded results.

---

### Task 1: Give the TUI the Android request ordering

**Files:**
- Modify: `cli/src/tui/mod.rs`
- Modify: `cli/src/tui/draw.rs`

- [ ] Add unit tests first for `critical > normal > low`, newest-first within each priority, stable tie handling by ID, and selection retention when a refresh reorders other rows.
- [ ] Add a pure `sort_requests(&mut [Item])` helper and call it inside `App::set_items` before locating the previous selection. Do not apply priority sorting to History.
- [ ] Convert RFC3339 timestamps once per comparison test fixture; production sorting may compare normalized ISO timestamps directly because Worker timestamps are UTC ISO-8601.
- [ ] Verify filters and check cursor still follow the selected item ID after a reorder.
- [ ] Run `cd cli && cargo test tui` and commit: `feat(tui): align request priority ordering with Android`

### Task 2: Render recommendations consistently in CLI and TUI

**Files:**
- Modify: `cli/src/client.rs`
- Modify: `cli/src/commands.rs`
- Modify: `cli/src/tui/mod.rs`
- Modify: `cli/src/tui/draw.rs`
- Modify: `cli/tests/cli.rs`

- [ ] Add rendering tests for recommendation text, recommended choice, a legacy non-checklist warning, checklist exemption, narrow terminals, reader mode, and history outcomes.
- [ ] In Requests rows, show the recommended choice in restrained amber when present; show `! recommendation missing` only for open non-checklist legacy items. Do not mark checklists.
- [ ] In the detail pane and markdown reader, place a `Recommendation` section before the body. Mark the matching footer choice with the existing amber accent and a compact `recommended` label without changing its number key.
- [ ] Extend `/` matching to title, agent display name, and body so it matches Android Requests search. Keep the current filtering interaction and selection clamping.
- [ ] Include nullable recommendation fields in `lam list --json`/`lam show` naturally through the shared item model; keep compact text output scannable.
- [ ] Run Rust fmt/clippy/tests and commit: `feat(tui): surface agent recommendations`

### Task 3: Adopt the canonical history endpoint in the Rust client

**Files:**
- Modify: `cli/src/client.rs`
- Modify: `cli/src/tui/mod.rs`
- Modify: `cli/tests/cli.rs`

- [ ] Add Wiremock cases for `{items,next_cursor}`, opaque cursor forwarding, effective-close ordering, and a legacy Worker fallback.
- [ ] Add `HistoryPage { items, next_cursor }` and `Client::history(limit, cursor, q)` against `/history`. The TUI uses the opaque server cursor instead of deriving a cursor from `created_at`.
- [ ] If `/history` returns `404`, fall back for that process to the existing `/items?limit=&before=` behavior so the new CLI remains usable during staggered deployment.
- [ ] Preserve independent Requests/History selection and the `history_end` rule. Do not change the default `lam list --all` API.
- [ ] Run integration/unit tests and commit: `refactor(tui): page canonical item history`

### Task 4: Produce the approved lam app-icon family

**Files:**
- Create: `design/app-icon/lam-mark.svg`
- Create: `android/app/src/main/res/drawable/ic_launcher_foreground.xml`
- Create: `android/app/src/main/res/drawable/ic_notification_lam.xml`
- Create: `android/app/src/main/res/drawable/ic_launcher_monochrome.xml`
- Create: `android/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml`
- Create: `android/app/src/main/res/mipmap-anydpi-v26/ic_launcher_round.xml`
- Create: generated density assets under `android/app/src/main/res/mipmap-{mdpi,hdpi,xhdpi,xxhdpi,xxxhdpi}/`
- Modify: `android/app/src/main/AndroidManifest.xml`
- Create: `android/app/src/main/res/values/colors.xml`

- [ ] Reopen the approved visual-explorer artifact and reproduce only the selected lowercase `lam` mark: warm amber background, graphite wordmark, minimal flat geometry. Do not introduce the rejected cartoonish buttons or alternate Beacon/Gaze/Focus marks.
- [ ] Keep `design/app-icon/lam-mark.svg` as the canonical editable vector. Convert glyphs to paths so builds do not depend on a font license or installed font.
- [ ] Build adaptive foreground/background, round, legacy density, Android 13 monochrome, and a single-color transparent status-bar notification icon. Keep the adaptive foreground inside the safe zone.
- [ ] Add a resource/render test or screenshot harness that checks mask previews (circle, squircle, rounded square), monochrome themed icon, and notification small-icon legibility at 24 dp.
- [ ] Install on the S24 Ultra and inspect launcher, recent-apps, Settings, notification shade, and themed-icons mode at default and maximum display scaling.
- [ ] Commit: `feat(android): add canonical lam icon family`

### Task 5: Complete accessibility and reduced-motion verification

**Files:**
- Modify: Android Compose screens/components identified by failing checks
- Create: `android/app/src/androidTest/java/dev/carraes/lam/AccessibilityTest.kt`
- Create: `docs/android-accessibility.md`

- [ ] Use Compose semantics tests plus Android Accessibility Scanner/TalkBack to audit Requests, detail, checklist, quick response, History, pairing, Settings, and notification permission states.
- [ ] Prove all targets are at least 48 dp, text scales without clipping at 200%, contrast remains sufficient across each muted agent card, traversal order is logical, and state is never color-only.
- [ ] Prove swipe-left/right actions are exposed through custom accessibility actions and through long-press/detail overflow. TalkBack activation must never trigger a hidden swipe action.
- [ ] Respect system animator duration scale/reduce-motion: remove decorative transitions when disabled, while retaining necessary progress/state changes.
- [ ] Record device/Android version, checked font/display scales, scanner findings, and manual TalkBack result in `docs/android-accessibility.md`.
- [ ] Run lint and connected instrumentation; commit: `fix(android): close accessibility and motion gaps`

### Task 6: Harden redacted diagnostics

**Files:**
- Modify: `android/app/src/main/java/dev/carraes/lam/diagnostics/Diagnostics.kt`
- Modify: `android/app/src/main/java/dev/carraes/lam/ui/settings/SettingsScreen.kt`
- Create: `android/app/src/test/java/dev/carraes/lam/diagnostics/DiagnosticsTest.kt`
- Modify: `worker/src/services/FcmAuth.ts`
- Modify: `worker/src/services/Push.ts`
- Modify: `worker/src/services/Delivery.ts`
- Modify: `worker/test/delivery.test.ts`

- [ ] Seed tests with recognizable fake master/device/pairing/OAuth/FCM values and request bodies, exercise every error path, and assert none appears in local diagnostic text or captured Worker logs.
- [ ] Keep a bounded local ring of timestamp, subsystem, operation, result category, HTTP status, item ID, and version. Exclude headers, bodies, tokens, recommendations, and replies.
- [ ] `Copy local diagnostics` places one redacted plain-text report on clipboard and displays Android's clipboard privacy notice expectation; it never writes a shareable file automatically.
- [ ] Worker logs include delivery transport, outcome category, item ID/version, and device ID only. Use structured Effect log annotations rather than interpolated errors that may contain response bodies.
- [ ] Run Android/Worker tests and commit: `fix: redact Android and delivery diagnostics`

### Task 7: Add safe root Android commands

**Files:**
- Create: `scripts/android-device.sh`
- Modify: `justfile`
- Modify: `README.md`

- [ ] Add shell-level tests by factoring device parsing into functions and feeding recorded `adb devices -l`/`getprop` output. Cover zero devices, unauthorized, multiple devices, wrong model, explicit serial, and expected SM-S928B.
- [ ] Add these exact recipes:

```text
just android-check
just android-build
just android-install [serial]
just android-logs [serial]
just android-test-e2e [serial]
```

- [ ] `android-check` runs JVM tests, lint, and debug assemble only. Add it to root `just check`; it must not inspect ADB, Firebase availability, or release secrets.
- [ ] `android-build` requires release signing properties and produces a minified signed APK. Verify it with `apksigner verify --verbose --print-certs` and print the APK path/fingerprint, never passwords.
- [ ] `android-install` calls `android-build`, resolves exactly one authorized device (or the supplied serial), checks `ro.product.model == SM-S928B` unless `LAM_ANDROID_ALLOW_ANY_MODEL=1`, records installed version, runs `adb install -r`, and verifies package/version afterward. Never call uninstall or clear-data.
- [ ] `android-logs` runs a package/PID-scoped `adb logcat` with known lam tags. `android-test-e2e` starts the documented local Worker fixture, installs debug if needed, and runs only connected end-to-end tests.
- [ ] Update README with toolchain, signing-key backup/fingerprint, pairing, install/upgrade, log, and recovery instructions.
- [ ] Run `shellcheck scripts/android-device.sh`, every phone-free recipe, and dry-run device error paths; commit: `build: add safe Android release and ADB commands`

### Task 8: Run and record the full acceptance matrix

**Files:**
- Create: `docs/android-acceptance.md`
- Modify: only source/test files required to fix a reproduced acceptance failure

- [ ] Start the document with commit SHA, Worker deployment version, APK version/signing fingerprint, phone model/API, Play Services version, network, and timestamp. Use pass/fail/evidence columns without request content.
- [ ] Pair through `lam pair`; verify the QR expires in five minutes and cannot be reused. Upgrade once with `just android-install` and prove pairing/cache survive.
- [ ] Over mobile data with Wi-Fi off, test low/normal/critical notifications, grouping, badge, lock-screen privacy, deep link, permission denial, and notification-channel Settings.
- [ ] Test supported markdown, choices, free reply, checklist tick/untick/final resolve, link confirmation/Custom Tab, dismiss swipe+confirmation, quick response, and every non-gesture alternative.
- [ ] Keep TUI and app visible; test phone-to-TUI and TUI-to-phone changes, concurrent choice closure, incoming checklist updates without scroll jumps, and notification cancellation after TUI closure.
- [ ] Test airplane-mode cached reading/mutation disable, recovery after network return, force-stop/relaunch recovery, process death, phone reboot, expired items, FCM token refresh, owner revoke, and self-unpair.
- [ ] Run the TalkBack/font-scale checks from Task 5 and retain ntfy in parallel for every generated request. Confirm ntfy, phone page, `lam watch`, CLI, and TUI still work.
- [ ] Fix each reproducible failure test-first in a narrowly scoped commit, rerun its row, then rerun the entire matrix.
- [ ] Run final `just check`, `just android-build`, `just android-test-e2e`, and `just android-install`; attach command results and final pass counts to the acceptance document.
- [ ] Commit: `test: record lam Android v1 acceptance`

### Task 9: Perform reversible parallel rollout

**Files:**
- Modify: `README.md`
- Modify: `docs/android-acceptance.md`

- [ ] Apply D1 migrations, deploy Worker, deploy CLI/skill to every machine, and verify the exact versions before pairing the release app.
- [ ] Back up the release keystore and fingerprint in the user's password-manager/backup system; verify restoration without replacing the working key.
- [ ] Run Android and ntfy together for normal daily use. Track only delivery/state defects, not message content, in the acceptance document.
- [ ] Keep ntfy primary until every acceptance row passes and the agreed daily trial has no unresolved delivery defect. Then mark Android primary while leaving ntfy routes and subscriptions intact.
- [ ] Document rollback: stop using/uninstall Android, revoke its device record, and continue with TUI/phone/ntfy. Do not roll back additive migrations merely to remove the app.
- [ ] Commit: `docs: complete Android parallel rollout`

## Milestone Exit

- `just check` is deterministic and green; release/connected recipes are green on the S24 Ultra.
- TUI and Android show recommendations and the same active-queue ordering.
- The approved lam mark is installed and legible across launcher/theme/notification contexts.
- Accessibility, privacy, offline, concurrency, revocation, mobile-data, upgrade, and legacy-client acceptance rows are recorded as passing.
- Android is primary only after the parallel trial; ntfy remains a working fallback.
