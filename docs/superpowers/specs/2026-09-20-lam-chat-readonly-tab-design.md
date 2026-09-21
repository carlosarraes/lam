# Read-only Chat tab in the main LAM TUI

## Purpose

The main `lam` TUI needs a fourth tab for watching agent Chat without leaving the Requests, History and Articles workflow. `lam chat` remains the place to compose, address and reply to messages.

## Behavior

- `h` and `l` cycle Requests, History, Articles and Chat. `Ctrl+4` selects Chat when the terminal supports modified keys. Existing choice shortcuts still work in Requests.
- Chat uses the main TUI header and footer. Its body shows a selectable live feed and the selected message, including recipients, delivery receipts and exposure state. `j/k` move through messages, `PgUp/PgDn` scroll the detail, and `G` jumps to latest. No key in this tab sends, replies to, or consumes a message.
- The tab opens without blocking the main TUI. A background observer obtains history and a subscription cursor, then reconnects after disconnection. Switching away does not discard its feed or subscription. Missing Chat config, ambiguous project selection or a stopped daemon produce an inline status; the other tabs continue to work.
- Select the mapped project containing the current directory when unambiguous. Outside mapped roots, select the sole configured project. If several projects remain possible, show a selection hint rather than silently mixing them. The tab never creates a new Chat config or database.
- Keep `lam chat` as the interactive, project-scoped client. Do not add Cloudflare calls, phone notifications, or native-agent bindings to the read-only tab.

## Implementation shape

Reuse the existing Chat feed model, subscription protocol and feed/detail rendering. Add a small read-only controller for the main TUI, not a second Chat protocol client. The main TUI owns tab selection; the controller owns Chat feed state and its background observer. A read-only key handler exposes navigation only.

## Acceptance

Unit tests cover four-tab navigation, non-interactive keys, project selection and unseen/selection behavior. Terminal rendering tests cover the combined header and Chat feed. An isolated daemon smoke test checks initial history and live updates without affecting production sessions.
