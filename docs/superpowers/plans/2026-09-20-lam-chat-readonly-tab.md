# Read-only Chat tab implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Carlos chose inline execution.

**Goal:** Watch the project Chat feed from the main `lam` TUI without adding a composer there.

**Architecture:** Add a fourth main-TUI tab backed by a read-only adapter over the existing Chat feed model and subscriber. Start the observer on a background thread, so Chat config or socket failure cannot block the existing TUI. Keep `lam chat` interactive.

**Tech Stack:** Rust, ratatui, crossterm, local Unix observer socket.

**Spec:** [Read-only Chat tab design](../specs/2026-09-20-lam-chat-readonly-tab-design.md)

## Global constraints

- No Chat send, reply, fetch or native binding from the main TUI tab.
- Do not mutate Chat config or database while selecting a project.
- A Chat failure must not block Requests, History or Articles.
- Do not replace installed binaries or restart running sessions until verification finishes.

---

### Task 1: Read-only Chat adapter and rendering

**Files:** `cli/src/chat/tui/mod.rs`, `cli/src/chat/tui/draw.rs`.

**Interfaces:** Expose `ReadonlyFeed::new()`, `start()`, `poll()`, `handle_key(KeyEvent)`, and `draw(&mut Frame, Rect)` to `cli::tui`. Reuse `App::add_page`, `spawn_subscription`, and the existing feed/detail renderer. `handle_key` returns no send or reply action.

- [ ] Add a failing unit test that passes `r`, `a`, Enter and printable text to `ReadonlyFeed::handle_key`, then asserts no composer, reply target or pending send changed. Add a separate fixture checking that a live feed event preserves an older selection and increments unseen count.
- [ ] Run `cargo test --manifest-path cli/Cargo.toml readonly` and confirm the tests fail because the adapter does not exist.
- [ ] Implement the adapter. Its background startup discovers existing paths/config, chooses the current-directory mapping or sole mapping, loads the tail with `HistoryTail`, subscribes from `subscribe_cursor`, and reports errors inline. Reuse the current `Network::Feed` and `Network::Status` path. Extract feed/detail drawing so both Chat screens use the same layout code.
- [ ] Run focused tests, then `cargo fmt --manifest-path cli/Cargo.toml --check`.

### Task 2: Main TUI integration and docs

**Files:** `cli/src/tui/mod.rs`, `cli/src/tui/draw.rs`, `README.md`, `docs/chat.md`.

**Interfaces:** `Tab::Chat` follows Articles in `l` order; `Ctrl+4` selects it; `App` owns `ReadonlyFeed`; `event_loop` starts it on first visit and polls it on each iteration.

- [ ] Add failing tests for four-tab `h/l` order, `Ctrl+4`, read-only keys, and the main header rendering `[chat]` on a test terminal. Confirm red with focused `cargo test`.
- [ ] Add `Tab::Chat` to tab navigation and rendering; route only navigation keys to `ReadonlyFeed`. The Chat tab draws under the existing main header and footer. A missing Chat service shows an inline status.
- [ ] Update the user-facing TUI key summary and Chat documentation; retain `lam chat` as the interactive command.
- [ ] Run focused tests, all Linux Rust tests, formatting, Clippy with warnings denied, and release build. Run Mac formatting, Clippy, tests and release build from the staged arm64 copy. Smoke-test a private local daemon and the real terminal observer. Do not disturb production sessions.
- [ ] Review the final diff and commit focused changes. Push immediately to `upstream`. Only then install the approved binaries and service/hooks, validate live status, and tell Carlos to refresh the TUI.
