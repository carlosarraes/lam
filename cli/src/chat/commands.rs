use super::{
    config::{Config, Paths},
    protocol,
};
#[cfg(not(target_os = "linux"))]
use anyhow::bail;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;

const MAX_BODY_BYTES: u64 = 65_536;

#[derive(Args)]
pub struct ChatArgs {
    #[arg(long)]
    pub project: Option<String>,
    #[command(subcommand)]
    pub command: Option<ChatCommand>,
}
#[derive(Subcommand)]
pub enum ChatCommand {
    /// Run an experimental native Chat hook. Empty checks are silent; Codex PostToolUse may supply peer context.
    Hook {
        #[arg(long, value_parser = ["codex", "claude", "pi"])]
        client: String,
        #[arg(long)]
        event: String,
        #[arg(long)]
        name: Option<String>,
    },
    #[command(hide = true)]
    Bridge {
        #[arg(long, value_parser = ["pi"])]
        client: String,
    },
    #[command(hide = true)]
    PeerStdio,
    /// Preview or install project-scoped native Chat integration without touching unrelated settings.
    Setup {
        #[arg(long, value_parser = ["codex", "claude", "pi"])]
        client: String,
        /// Project root for Codex/Claude; home directory for Pi. Defaults accordingly.
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        remove: bool,
    },
    /// Stage an owned user-service file. Installation does not start or enable it.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Run the private local Chat daemon. Does not install a service.
    Serve {
        #[arg(long, required = true)]
        foreground: bool,
        /// Enable the experimental, native-unverified validator for a scoped gate.
        #[arg(long)]
        native_bindings: bool,
    },
    /// Check the local daemon without contacting the cloud.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Send to pinned recipients in this project. Native delivery remains experimental.
    Send {
        #[arg(long, required = true)]
        to: Vec<String>,
        #[arg(long, conflicts_with_all = ["file", "stdin"])]
        message: Option<String>,
        #[arg(long, conflicts_with_all = ["message", "stdin"])]
        file: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["message", "file"])]
        stdin: bool,
        #[arg(long)]
        idempotency_key: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        allow_stale_roster: bool,
        #[arg(long)]
        json: bool,
    },
    /// Reply to the original sender, or explicitly include all original participants.
    Reply {
        message_id: String,
        #[arg(long, conflicts_with_all = ["file", "stdin"])]
        message: Option<String>,
        #[arg(long, conflicts_with_all = ["message", "stdin"])]
        file: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["message", "file"])]
        stdin: bool,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        idempotency_key: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the current project's registered agent sessions.
    Sessions {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read this project's observer feed without consuming agent inboxes.
    History {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: u16,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceAction {
    Install {
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        apply: bool,
    },
    Uninstall {
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        apply: bool,
    },
    Status {
        #[arg(long)]
        root: Option<PathBuf>,
    },
}
#[derive(Args)]
pub struct InboxArgs {
    #[arg(long)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<InboxCommand>,
}

#[derive(Subcommand)]
pub enum InboxCommand {
    /// Retry an unknown or refused handoff to this incarnation. May duplicate delivery.
    Retry { message_id: String },
    /// Fetch the full body of a message addressed to this native participant.
    Show { message_id: String },
}

pub fn run_chat(args: ChatArgs) -> Result<i32> {
    if let Some(ChatCommand::Hook {
        client,
        event,
        name,
    }) = &args.command
    {
        if std::env::var("LAM_CHAT_DISABLE").as_deref() == Ok("1") {
            return Ok(0);
        }
        return Ok(match client.as_str() {
            "codex" => super::adapters::codex::run_hook(event, name.clone()),
            "claude" => super::adapters::claude::run_hook(event, name.clone()),
            "pi" => super::adapters::pi::run_hook(event, name.clone()),
            _ => unreachable!("client is constrained by Clap"),
        });
    }
    if let Some(ChatCommand::Bridge { client }) = &args.command {
        if std::env::var("LAM_CHAT_DISABLE").as_deref() == Ok("1") {
            return Ok(0);
        }
        debug_assert_eq!(client, "pi");
        return super::adapters::pi::run_bridge();
    }
    if let Some(ChatCommand::Setup {
        client,
        root,
        apply,
        remove,
        ..
    }) = &args.command
    {
        #[cfg(unix)]
        {
            let root = match root {
                Some(root) => root.clone(),
                None if client == "pi" => {
                    dirs::home_dir().context("cannot find home for Pi extension")?
                }
                None => std::env::current_dir()?,
            };
            super::setup::run(client, &root, *apply, *remove)?;
        }
        #[cfg(not(unix))]
        bail!("Chat setup requires Unix platform support");
        return Ok(0);
    }
    if let Some(ChatCommand::Service { action }) = &args.command {
        #[cfg(unix)]
        {
            let (root, apply, remove, status) = match action {
                ServiceAction::Install { root, apply } => (root, *apply, false, false),
                ServiceAction::Uninstall { root, apply } => (root, *apply, true, false),
                ServiceAction::Status { root } => (root, false, false, true),
            };
            let root = match root {
                Some(root) => root.clone(),
                None => super::setup::default_service_root()?,
            };
            super::setup::run_service(&root, apply, remove, status)?;
        }
        #[cfg(not(unix))]
        bail!("Chat service setup requires Unix platform support");
        return Ok(0);
    }
    let paths = Paths::discover()?;
    if matches!(args.command.as_ref(), Some(ChatCommand::PeerStdio)) {
        #[cfg(unix)]
        super::peer::run_stdio(&paths)?;
        #[cfg(not(unix))]
        bail!("Chat peer bridge requires Unix sockets");
        return Ok(0);
    }
    if args.command.is_none() {
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            maybe_participant(&paths)?.is_none(),
            "open the Chat observer in a human terminal, outside a native agent session"
        );
        let project = selected_observer_project(&paths, args.project)?;
        super::tui::run(&paths, &project)?;
        return Ok(0);
    }
    anyhow::ensure!(
        args.project.is_none(),
        "for a Chat subcommand, place --project after the subcommand"
    );
    match args.command.expect("handled empty Chat command") {
        ChatCommand::Hook { .. } => unreachable!("hook is dispatched before fallible setup"),
        ChatCommand::Bridge { .. } => unreachable!("bridge is dispatched before fallible setup"),
        ChatCommand::PeerStdio => unreachable!("peer bridge is dispatched before command matching"),
        ChatCommand::Setup { .. } => unreachable!("setup is dispatched before command matching"),
        ChatCommand::Serve {
            native_bindings, ..
        } => {
            #[cfg(unix)]
            if native_bindings {
                #[cfg(target_os = "linux")]
                {
                    let directory = paths
                        .database
                        .parent()
                        .context("missing Chat data directory")?
                        .join("bindings");
                    let validator = super::adapters::NativeBindings::open(&directory)?;
                    super::daemon::run_with_validator(&paths, std::sync::Arc::new(validator))?;
                }
                #[cfg(not(target_os = "linux"))]
                bail!("native Chat binding validation is not implemented on this platform");
            } else {
                super::daemon::run(&paths)?;
            }
            #[cfg(not(unix))]
            bail!("Chat local service requires Unix sockets");
        }
        ChatCommand::Status { json } => {
            let status = status(&paths)?;
            if json {
                println!("{}", serde_json::to_string(&status)?);
            } else if status["running"] == true {
                println!(
                    "Chat daemon is running locally. Participant authentication: {}.",
                    status["participant_auth"].as_str().unwrap_or("unknown")
                );
            } else {
                println!("Chat daemon is not running. Start it with: lam chat serve --foreground");
            }
        }
        ChatCommand::Send {
            to,
            message,
            file,
            stdin,
            idempotency_key,
            project,
            allow_stale_roster,
            json,
        } => {
            #[cfg(target_os = "linux")]
            {
                use super::types::Draft;
                let body = read_body(message, file, stdin)?;
                let participant = maybe_participant(&paths)?;
                let project = if let Some(participant) = &participant {
                    selected_participant_project(participant, project)?
                } else {
                    selected_observer_project(&paths, project)?
                };
                let sessions = protocol::Operation::Sessions {
                    project: project.clone(),
                };
                let roster = if let Some(participant) = &participant {
                    participant.request(sessions)?
                } else {
                    observer_request(&paths, sessions)?
                };
                let recipients = exact_recipients(
                    roster,
                    &to,
                    allow_stale_roster,
                    participant.as_ref().map(|p| p.session()),
                )?;
                let operation = protocol::Operation::Send {
                    draft: Draft {
                        key: idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        project,
                        to: recipients,
                        body,
                        reply_to: None,
                    },
                };
                let result = submit_with_retry(&paths, participant.as_ref(), operation)?;
                print_send(&result, json)?;
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (
                    to,
                    message,
                    file,
                    stdin,
                    idempotency_key,
                    project,
                    allow_stale_roster,
                    json,
                );
                bail!("Chat requires a validated native participant binding; this platform integration is unavailable");
            }
        }
        ChatCommand::Reply {
            message_id,
            message,
            file,
            stdin,
            all,
            idempotency_key,
            project,
            json,
        } => {
            let body = read_body(message, file, stdin)?;
            #[cfg(target_os = "linux")]
            let result = {
                let participant = maybe_participant(&paths)?;
                let selected = if let Some(participant) = &participant {
                    selected_participant_project(participant, project)?
                } else {
                    selected_observer_project(&paths, project)?
                };
                let show = protocol::Operation::Show {
                    id: message_id.clone(),
                };
                let original = if let Some(participant) = &participant {
                    participant.request(show)?
                } else {
                    observer_request(&paths, show)?
                };
                anyhow::ensure!(
                    original["draft"]["project"] == selected,
                    "Chat reply belongs to another project"
                );
                let operation = protocol::Operation::Reply {
                    id: message_id,
                    key: idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    body,
                    all,
                };
                submit_with_retry(&paths, participant.as_ref(), operation)?
            };
            #[cfg(not(target_os = "linux"))]
            let result = {
                let _ = project;
                participant_request(
                    &paths,
                    protocol::Operation::Reply {
                        id: message_id,
                        key: idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        body,
                        all,
                    },
                )?
            };
            print_send(&result, json)?;
        }
        ChatCommand::Sessions { project, json } => {
            let result = read_project_operation(&paths, project, |project| {
                protocol::Operation::Sessions { project }
            })?;
            print_value(&result, json)?;
        }
        ChatCommand::History {
            project,
            cursor,
            limit,
            json,
        } => {
            let result =
                read_project_operation(&paths, project, |project| protocol::Operation::History {
                    project,
                    cursor,
                    limit,
                })?;
            print_value(&result, json)?;
        }
        ChatCommand::Service { .. } => {
            unreachable!("Chat service was handled before daemon discovery")
        }
    }
    Ok(0)
}

pub fn run_inbox(args: InboxArgs) -> Result<i32> {
    let operation = match args.command {
        Some(InboxCommand::Retry { message_id }) => protocol::Operation::Retry { id: message_id },
        Some(InboxCommand::Show { message_id }) => protocol::Operation::Show { id: message_id },
        None => protocol::Operation::Inbox {
            cursor: None,
            limit: 100,
        },
    };
    let result = participant_request(&Paths::discover()?, operation)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(0)
}

#[cfg(target_os = "linux")]
fn participant(paths: &Paths) -> Result<super::adapters::Participant> {
    super::adapters::Participant::current(
        paths,
        std::time::Instant::now() + std::time::Duration::from_secs(6),
    )
}

#[cfg(target_os = "linux")]
fn maybe_participant(paths: &Paths) -> Result<Option<super::adapters::Participant>> {
    super::adapters::Participant::maybe_current(
        paths,
        std::time::Instant::now() + std::time::Duration::from_secs(6),
    )
}

#[cfg(target_os = "linux")]
fn submit_with_retry(
    paths: &Paths,
    native: Option<&super::adapters::Participant>,
    operation: protocol::Operation,
) -> Result<Value> {
    let key = match &operation {
        protocol::Operation::Send { draft } => draft.key.clone(),
        protocol::Operation::Reply { key, .. } => key.clone(),
        _ => anyhow::bail!("only addressed Chat sends may use submission retry"),
    };
    let first = if let Some(participant) = native {
        participant.request(operation.clone())
    } else {
        observer_request(paths, operation.clone())
    };
    if let Ok(result) = first {
        return Ok(result);
    }
    let second = if native.is_some() {
        participant(paths).and_then(|client| client.request(operation))
    } else {
        observer_request(paths, operation)
    };
    second.map_err(|error| anyhow::anyhow!(
        "Chat send was not confirmed. Retry with --idempotency-key {key} to avoid duplicates: {error}"
    ))
}

fn read_project_operation(
    paths: &Paths,
    selected: Option<String>,
    build: impl FnOnce(String) -> protocol::Operation,
) -> Result<Value> {
    #[cfg(target_os = "linux")]
    {
        if let Some(participant) = maybe_participant(paths)? {
            let project = selected_participant_project(&participant, selected)?;
            return participant.request(build(project));
        }
    }
    let project = selected_observer_project(paths, selected)?;
    observer_request(paths, build(project))
}

fn participant_request(paths: &Paths, operation: protocol::Operation) -> Result<Value> {
    #[cfg(target_os = "linux")]
    {
        participant(paths)?.request(operation)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (paths, operation);
        bail!("Chat requires a validated native participant binding; this platform integration is unavailable")
    }
}

fn read_body(message: Option<String>, file: Option<PathBuf>, stdin: bool) -> Result<String> {
    let count = u8::from(message.is_some()) + u8::from(file.is_some()) + u8::from(stdin);
    anyhow::ensure!(
        count == 1,
        "choose exactly one of --message, --file, or --stdin"
    );
    let body = if let Some(message) = message {
        message
    } else {
        let mut bytes = Vec::new();
        if let Some(path) = file {
            std::fs::File::open(&path)
                .with_context(|| format!("cannot open Chat body file {}", path.display()))?
                .take(MAX_BODY_BYTES + 1)
                .read_to_end(&mut bytes)?;
        } else {
            std::io::stdin()
                .lock()
                .take(MAX_BODY_BYTES + 1)
                .read_to_end(&mut bytes)?;
        }
        String::from_utf8(bytes).context("Chat body must be UTF-8")?
    };
    anyhow::ensure!(
        body.len() <= MAX_BODY_BYTES as usize,
        "Chat body exceeds 64 KiB"
    );
    anyhow::ensure!(!body.trim().is_empty(), "Chat body cannot be empty");
    Ok(body)
}

fn print_send(result: &Value, json_output: bool) -> Result<()> {
    if json_output {
        let recipients = result["draft"]["to"]
            .as_array()
            .context("Chat send response has no recipient set")?
            .iter()
            .map(|target| json!({"target": target, "state": "queued_locally"}))
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string(
                &json!({"id": result["id"], "recipients": recipients, "message": result})
            )?
        );
    } else {
        let id = result["id"]
            .as_str()
            .context("Chat send response has no ID")?;
        let count = result["draft"]["to"]
            .as_array()
            .context("Chat send response has no recipient set")?
            .len();
        println!("Queued Chat message {id} for {count} recipient(s).");
    }
    Ok(())
}

fn print_value(result: &Value, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string(result)?);
    } else if let Some(sessions) = result["sessions"].as_array() {
        for session in sessions {
            println!(
                "{}  {}  {}/{}{}",
                session["name"].as_str().unwrap_or("?"),
                session["client"].as_str().unwrap_or("?"),
                session["session"]["machine"].as_str().unwrap_or("?"),
                session["session"]["incarnation"].as_str().unwrap_or("?"),
                if session["eligible"] == true {
                    ""
                } else {
                    "  unavailable"
                }
            );
        }
    } else {
        println!("{}", serde_json::to_string_pretty(result)?);
    }
    Ok(())
}

fn selected_observer_project(paths: &Paths, selected: Option<String>) -> Result<String> {
    if let Some(project) = selected {
        protocol::validate_project(&project)?;
        return Ok(project);
    }
    let config = Config::load_or_create(&paths.config)?;
    config.project_for_root(&std::env::current_dir()?)
}

#[cfg(target_os = "linux")]
fn selected_participant_project(
    participant: &super::adapters::Participant,
    selected: Option<String>,
) -> Result<String> {
    if let Some(project) = selected {
        protocol::validate_project(&project)?;
        anyhow::ensure!(
            project == participant.project(),
            "Chat project is outside this participant binding"
        );
        Ok(project)
    } else {
        Ok(participant.project().into())
    }
}

#[cfg(unix)]
pub(super) fn observer_request(paths: &Paths, operation: protocol::Operation) -> Result<Value> {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    operation.validate()?;
    super::daemon::validate_socket(&paths.observer_socket)?;
    let mut stream =
        UnixStream::connect(&paths.observer_socket).context("Chat daemon is not running")?;
    stream.set_read_timeout(Some(Duration::from_secs(6)))?;
    stream.set_write_timeout(Some(Duration::from_secs(6)))?;
    super::daemon::peer_identity(&stream)?;
    protocol::write_frame(
        &mut stream,
        &serde_json::to_value(protocol::Request {
            version: 1,
            operation,
        })?,
    )?;
    let response = protocol::read_frame(&mut stream)?;
    anyhow::ensure!(
        response["version"] == 1 && response["ok"] == true,
        "invalid Chat observer response"
    );
    Ok(response["data"].clone())
}

#[cfg(not(unix))]
fn observer_request(_paths: &Paths, _operation: protocol::Operation) -> Result<Value> {
    bail!("Chat observer requires Unix sockets")
}

#[cfg(target_os = "linux")]
fn exact_recipients(
    roster: Value,
    addresses: &[String],
    allow_stale: bool,
    sender: Option<&super::types::SessionRef>,
) -> Result<Vec<super::types::Target>> {
    #[derive(serde::Deserialize)]
    struct Entry {
        session: super::types::SessionRef,
        name: String,
        eligible: bool,
    }
    let entries: Vec<Entry> =
        serde_json::from_value(roster["sessions"].clone()).context("invalid Chat roster")?;
    let mut recipients = Vec::new();
    let stale = roster["stale"] == true;
    for address in addresses {
        if address == "@all" {
            anyhow::ensure!(
                !stale || allow_stale,
                "project roster is stale; use --allow-stale-roster to broadcast"
            );
            for entry in &entries {
                let target = super::types::Target::Agent(entry.session.clone());
                if entry.eligible && sender != Some(&entry.session) && !recipients.contains(&target)
                {
                    recipients.push(target);
                }
            }
            continue;
        }
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry.eligible
                    && (entry.name == *address
                        || format!("{}/{}", entry.session.machine, entry.session.incarnation)
                            == *address)
            })
            .collect();
        anyhow::ensure!(
            matches.len() == 1,
            "recipient {address:?} is missing or ambiguous; use its exact machine/incarnation"
        );
        protocol::validate_session(&matches[0].session)?;
        let target = super::types::Target::Agent(matches[0].session.clone());
        if !recipients.contains(&target) {
            recipients.push(target);
        }
    }
    anyhow::ensure!(!recipients.is_empty(), "no eligible recipients selected");
    Ok(recipients)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn recipient_selection_is_exact_eligible_and_unambiguous() {
        let machine = "11111111-1111-4111-8111-111111111111";
        let incarnation = "22222222-2222-4222-8222-222222222222";
        let entry = json!({"session":{"machine":machine,"incarnation":incarnation},"name":"owned","eligible":true,"client":"codex"});
        let roster = json!({"sessions":[entry.clone()]});
        assert_eq!(
            exact_recipients(roster.clone(), &["owned".into()], false, None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            exact_recipients(
                roster.clone(),
                &[format!("{machine}/{incarnation}")],
                false,
                None
            )
            .unwrap()
            .len(),
            1
        );
        assert!(exact_recipients(roster, &["own".into()], false, None).is_err());
        assert!(exact_recipients(
            json!({"sessions":[entry.clone(),entry.clone()]}),
            &["owned".into()],
            false,
            None
        )
        .is_err());
        let mut ineligible = entry;
        ineligible["eligible"] = json!(false);
        assert!(exact_recipients(
            json!({"sessions":[ineligible]}),
            &["owned".into()],
            false,
            None
        )
        .is_err());
    }

    #[test]
    fn broadcast_freezes_unique_eligible_recipients_and_excludes_sender() {
        let first = super::super::types::SessionRef {
            machine: "11111111-1111-4111-8111-111111111111".into(),
            incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        };
        let second = super::super::types::SessionRef {
            machine: first.machine.clone(),
            incarnation: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into(),
        };
        let roster = json!({"sessions": [
            {"session": first, "name": "self", "eligible": true},
            {"session": second, "name": "peer", "eligible": true}
        ], "stale": false});
        assert_eq!(
            exact_recipients(
                roster.clone(),
                &["@all".into(), "peer".into()],
                false,
                Some(&first)
            )
            .unwrap(),
            vec![super::super::types::Target::Agent(second.clone())]
        );
        let mut stale = roster;
        stale["stale"] = json!(true);
        assert!(exact_recipients(stale.clone(), &["@all".into()], false, Some(&first)).is_err());
        assert_eq!(
            exact_recipients(stale, &["@all".into()], true, Some(&first))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn body_file_is_bounded_and_utf8_validated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message.md");
        std::fs::write(&path, "a 🐏 message").unwrap();
        assert_eq!(
            read_body(None, Some(path.clone()), false).unwrap(),
            "a 🐏 message"
        );
        std::fs::write(&path, vec![b'x'; 65_537]).unwrap();
        assert!(read_body(None, Some(path.clone()), false)
            .unwrap_err()
            .to_string()
            .contains("64 KiB"));
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_body(None, Some(path), false)
            .unwrap_err()
            .to_string()
            .contains("UTF-8"));
    }

    #[test]
    fn lost_observer_response_retries_the_same_idempotency_key() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("observer.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let paths = Paths {
            config: dir.path().join("chat.toml"),
            database: dir.path().join("chat.sqlite3"),
            socket: dir.path().join("chat.sock"),
            observer_socket: socket,
            peer_socket: dir.path().join("peer.sock"),
            lock: dir.path().join("chat.lock"),
        };
        let owner = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                requests.push(protocol::read_frame(&mut stream).unwrap());
                if attempt == 1 {
                    protocol::write_frame(&mut stream, &json!({"version":1,"ok":true,"data":{"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}})).unwrap();
                }
            }
            requests
        });
        let operation = protocol::Operation::Send {
            draft: super::super::types::Draft {
                key: "one-stable-key".into(),
                project: "22222222-2222-4222-8222-222222222222".into(),
                to: vec![super::super::types::Target::Agent(
                    super::super::types::SessionRef {
                        machine: "11111111-1111-4111-8111-111111111111".into(),
                        incarnation: "33333333-3333-4333-8333-333333333333".into(),
                    },
                )],
                body: "test".into(),
                reply_to: None,
            },
        };
        assert_eq!(
            submit_with_retry(&paths, None, operation).unwrap()["id"],
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        );
        let requests = owner.join().unwrap();
        assert_eq!(requests[0], requests[1]);
        assert_eq!(requests[0]["operation"]["draft"]["key"], "one-stable-key");
    }
}

fn status(paths: &Paths) -> Result<Value> {
    let mut status = json!({"running": false, "observer_socket": paths.observer_socket, "participant_socket": paths.socket, "action": "lam chat serve --foreground"});
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        use std::time::Duration;
        match paths.observer_socket.symlink_metadata() {
            Ok(_) => super::daemon::validate_socket(&paths.observer_socket)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(status),
            Err(error) => return Err(error.into()),
        }
        let mut stream = match UnixStream::connect(&paths.observer_socket) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(status)
            }
            Err(error) => return Err(error.into()),
        };
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        super::daemon::peer_identity(&stream)?;
        protocol::write_frame(
            &mut stream,
            &json!({"version": 1, "operation": {"op": "status"}}),
        )?;
        let response = protocol::read_frame(&mut stream)?;
        anyhow::ensure!(
            response["version"] == 1 && response["ok"] == true,
            "Chat status request failed: {}",
            response["error"]
        );
        status.as_object_mut().unwrap().extend(
            response["data"]
                .as_object()
                .context("invalid Chat status response")?
                .clone(),
        );
        status.as_object_mut().unwrap().remove("action");
    }
    Ok(status)
}
