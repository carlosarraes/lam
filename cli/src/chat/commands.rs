use super::{config::Paths, protocol};
#[cfg(not(target_os = "linux"))]
use anyhow::bail;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[derive(Args)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub command: ChatCommand,
}
#[derive(Subcommand)]
pub enum ChatCommand {
    /// Run the experimental native-unverified Codex hook. Empty checks are silent; PostToolUse may supply peer context.
    Hook {
        #[arg(long, value_parser = ["codex"])]
        client: String,
        #[arg(long)]
        event: String,
        #[arg(long)]
        name: Option<String>,
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
    /// Queue for exact registered recipients. Native delivery remains experimental and unverified.
    Send {
        #[arg(long, required = true)]
        to: Vec<String>,
        #[arg(long)]
        message: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Reply to the original sender, or explicitly include all original participants.
    Reply {
        message_id: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        idempotency_key: Option<String>,
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
    if let ChatCommand::Hook { event, name, .. } = args.command {
        return Ok(super::adapters::codex::run_hook(&event, name));
    }
    let paths = Paths::discover()?;
    match args.command {
        ChatCommand::Hook { .. } => unreachable!("hook is dispatched before fallible setup"),
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
            idempotency_key,
        } => {
            #[cfg(target_os = "linux")]
            {
                use super::types::Draft;
                let participant = participant(&paths)?;
                let roster = participant.request(protocol::Operation::Sessions {
                    project: participant.project().into(),
                })?;
                let recipients = exact_recipients(roster, &to)?;
                let result = participant.request(protocol::Operation::Send {
                    draft: Draft {
                        key: idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        project: participant.project().into(),
                        to: recipients,
                        body: message,
                        reply_to: None,
                    },
                })?;
                println!("{}", serde_json::to_string(&result)?);
            }
            #[cfg(not(target_os = "linux"))]
            bail!("Chat requires a validated native participant binding; this platform integration is unavailable");
        }
        ChatCommand::Reply {
            message_id,
            message,
            all,
            idempotency_key,
        } => {
            let result = participant_request(
                &paths,
                protocol::Operation::Reply {
                    id: message_id,
                    key: idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    body: message,
                    all,
                },
            )?;
            println!("{}", serde_json::to_string(&result)?);
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

#[cfg(target_os = "linux")]
fn exact_recipients(roster: Value, addresses: &[String]) -> Result<Vec<super::types::Target>> {
    #[derive(serde::Deserialize)]
    struct Entry {
        session: super::types::SessionRef,
        name: String,
        eligible: bool,
    }
    let entries: Vec<Entry> =
        serde_json::from_value(roster["sessions"].clone()).context("invalid Chat roster")?;
    let mut recipients = Vec::new();
    for address in addresses {
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
        recipients.push(super::types::Target::Agent(matches[0].session.clone()));
    }
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
            exact_recipients(roster.clone(), &["owned".into()])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            exact_recipients(roster.clone(), &[format!("{machine}/{incarnation}")])
                .unwrap()
                .len(),
            1
        );
        assert!(exact_recipients(roster, &["own".into()]).is_err());
        assert!(exact_recipients(
            json!({"sessions":[entry.clone(),entry.clone()]}),
            &["owned".into()]
        )
        .is_err());
        let mut ineligible = entry;
        ineligible["eligible"] = json!(false);
        assert!(exact_recipients(json!({"sessions":[ineligible]}), &["owned".into()]).is_err());
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
