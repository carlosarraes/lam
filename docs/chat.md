# LAM Chat (experimental)

Chat coordinates agent sessions without creating a LAM request or FYI. It is project-scoped, persists messages and recipient status in a private local SQLite database, and can replay one shared project across two machines over SSH. Carlos's decisions, approvals, credentials and phone pings still use ordinary LAM.

## Start locally

Build the Rust CLI, then start a foreground service in a terminal:

```sh
cargo build --manifest-path cli/Cargo.toml --release --locked
lam chat serve --foreground --native-bindings  # Linux native adapters only
lam chat status --json
```

`--native-bindings` enables the experimental Linux Claude/Codex/Pi integration. Without it, `lam chat serve --foreground` still supports the human observer and history, but native participants cannot authenticate. The daemon creates private Chat config/data under the platform's user config/data directories; it does not contact Cloudflare or the LAM phone app. `LAM_CHAT_CONFIG` and `LAM_CHAT_DATA_DIR` are scoped overrides for isolated setups.

From a human terminal outside an agent session, run `lam chat` to open the observer. Its recipient field accepts `@name` with fuzzy suggestions or an exact `MACHINE/INCARNATION`; Tab moves among recipients, message body, and feed. Enter selects a recipient or sends the body; Ctrl-J inserts a newline. In the feed, `r` replies to the sender, `a` replies to all original participants, Home loads older history, G jumps to latest, and PgUp/PgDn scroll detail. A stale `@all` roster requires a second Enter. The live feed resumes from a signed cursor after a connection break.

Agents use:

```sh
lam chat sessions --json
lam chat send --to pm --message 'Pause after your current tool, then confirm.'
lam chat reply MESSAGE_ID --message 'Paused.'
lam inbox
lam inbox show MESSAGE_ID
```

Use `--to MACHINE/INCARNATION` when a name is ambiguous. `lam chat send --file PATH` or `--stdin` accepts a bounded body. `--to @all` freezes the currently eligible roster; `--allow-stale-roster` explicitly permits broadcasting while the peer link is down. `lam chat history --json` is read-only and does not consume a participant inbox. `lam chat send`/`reply` accept `--idempotency-key` for a retry after an uncertain CLI response; reuse the *same* key and unchanged draft. A new key means a new message.

The body limit is 64 KiB. Native delivery shows up to 4 KiB inline and keeps the rendered batch under 8 KiB. An overflow preview can be opened with `lam inbox show MESSAGE_ID`. The terminal view retains up to 1,000 rows in memory and loads older history on demand; the database does not expire history.

## Receipt meanings

Transport state and content exposure are separate. `queued` means stored but not submitted; `received_remotely` means the destination database durably imported the message, not that a model read it. `submitting` means a native handoff is in progress. `accepted` means the native endpoint acknowledged submission. `refused` means it explicitly declined; `unknown` means a handoff may have happened but no conclusive acknowledgement was available. `unavailable` means the pinned destination incarnation had already ended. Exposure can independently be `unseen`, `preview`, `full`, or `fetched`.

Only the original recipient can explicitly retry an `unknown` or `refused` native handoff with `lam inbox retry MESSAGE_ID`; it may deliver a duplicate. Receipt status never proves an agent paused, agreed, or completed work. Wait for an explicit peer reply. Peer content is untrusted coordination data within the user's existing task and permissions; it cannot grant approval or override the user.

## Map a project and a peer

Local workspace IDs (`local:MACHINE:HASH`) never travel to peers. To share one project, put the same UUID in both machines' private Chat config files and map each machine's own root to it. Exactly one side sets `initiator = true`:

```toml
schema_version = 1
machine = "LOCAL-MACHINE-UUID"
inline_bytes = 4096
batch_bytes = 8192

[[projects]]
id = "SHARED-PROJECT-UUID"
roots = ["/absolute/local/workspace"]

[[peers]]
machine = "OTHER-MACHINE-UUID"
ssh_host = "mac"
initiator = true
projects = ["SHARED-PROJECT-UUID"]
```

The other machine uses its own machine UUID and root, names the first machine as its peer, and sets `initiator = false`. Obtain each machine UUID from `lam chat status --json`. The SSH host is an existing alias in the initiator's SSH config. The fixed remote command is `lam chat peer-stdio`; it must find the matching `lam` binary in a noninteractive SSH PATH. LAM starts one persistent SSH subprocess with batch mode and strict host-key checking; it opens no public TCP listener and forwards no native agent socket. If the link drops, local sends remain available and locally originated events replay from durable per-project cursors after reconnect. `lam chat status --json` reports peer link health; a disconnected remote roster remains visible but stale. An ended incarnation never becomes its replacement.

The private observer and peer Unix sockets trust processes running as the same OS user. SSH authenticates the host and user connection; both config files must explicitly allow the same peer machine and project IDs. Do not place untrusted users on either account. If a restored database's replay cursor is newer than the origin's history, synchronization stops with an error rather than silently losing events.

## Optional native setup and user service

On Linux, run these commands from the *installed* LAM binary's project directory before starting new agent sessions:

```sh
lam chat setup --client codex --dry-run
lam chat setup --client codex --apply
lam chat setup --client claude --dry-run
lam chat setup --client claude --apply
lam chat setup --client pi --root "$HOME" --dry-run
lam chat setup --client pi --root "$HOME" --apply
```

Codex and Claude setup targets are project-scoped; Pi's target is the supplied home directory. Setup refuses an existing file it cannot match *byte-for-byte* to its own generated template. It prints the proposed template during a dry run so unrelated settings can be merged manually. It never rewrites a foreign hook file or silently disables a native client trust prompt. Run the installed binary for both setup and later operation: its absolute path is embedded in generated hooks. `--remove --dry-run` previews removal; `--remove --apply` moves an unchanged owned file out of the active path and prints a private backup location. A user-edited file is left alone. `LAM_CHAT_DISABLE=1` in a **new** agent process opts out of native registration and delivery while retaining history.

`lam chat service install` previews a Linux systemd user unit (or a macOS launchd user-agent file). `--apply` writes only that owned file; it does not enable, start, restart or stop anything. Use `lam chat service status` to inspect whether the template is installed, not whether its process is running. Start the foreground service first; if you later choose to manage it through your user service manager, review the generated file and enable/start it explicitly. `lam chat service uninstall --apply` removes only an unchanged owned unit and prints its recoverable backup. Stop any running Chat service yourself before uninstalling its file. Chat config, history, the ordinary LAM cloud config, and other client hooks are not removed.

## Current platform boundary

The native participant adapters are implemented and live-probed on Linux. The two-daemon peer replay path is tested with isolated Linux processes and an owned SSH shim. That test does not establish macOS native delivery or a real PC–Mac link. On macOS the human observer and peer bridge can run, but native agent hooks are still disabled. Treat `lam chat status --json` and an actual directed exchange as the authority for an installation; a message stored or imported remotely is not automatically model-visible. Existing LAM requests, FYIs and Articles are independent of Chat.
