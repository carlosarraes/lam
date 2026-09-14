use super::{config::Paths, protocol};
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[derive(Args)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub command: ChatCommand,
}
#[derive(Subcommand)]
pub enum ChatCommand {
    /// Run the private local Chat daemon. Does not install a service.
    Serve {
        #[arg(long, required = true)]
        foreground: bool,
    },
    /// Check the local daemon without contacting the cloud.
    Status {
        #[arg(long)]
        json: bool,
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
}

pub fn run_chat(args: ChatArgs) -> Result<i32> {
    let paths = Paths::discover()?;
    match args.command {
        ChatCommand::Serve { .. } => {
            #[cfg(unix)]
            super::daemon::run(&paths)?;
            #[cfg(not(unix))]
            bail!("Chat local service requires Unix sockets");
        }
        ChatCommand::Status { json } => {
            let status = status(&paths)?;
            if json {
                println!("{}", serde_json::to_string(&status)?);
            } else if status["running"] == true {
                println!("Chat daemon is running locally.");
            } else {
                println!("Chat daemon is not running. Start it with: lam chat serve --foreground");
            }
        }
    }
    Ok(0)
}

pub fn run_inbox(_args: InboxArgs) -> Result<i32> {
    bail!("Chat Inbox requires a validated native participant binding; configure the client integration before reading its inbox")
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
