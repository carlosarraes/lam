//! A single, origin-authorized replay loop over an SSH child's stdio.
//! Only the daemon touches SQLite. The bridge uses a private same-user socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::{Config, Paths, PeerConfig};
use super::protocol::{self, Operation, Request};
use super::types::PeerEvent;

const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Hello {
        version: u32,
        machine: String,
        projects: Vec<String>,
        initiator: bool,
    },
    Pull {
        project: String,
        after: u64,
    },
    Events {
        events: Vec<PeerEvent>,
    },
    Push {
        event: Box<PeerEvent>,
    },
    Imported {
        through: u64,
    },
    Ack {
        project: String,
        through: u64,
    },
    Acked,
}

pub fn ssh_command(peer: &PeerConfig) -> Result<Command> {
    ensure!(
        !peer.ssh_host.is_empty()
            && peer.ssh_host.len() <= 255
            && peer
                .ssh_host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.@".contains(&byte))
            && !peer.ssh_host.starts_with('-'),
        "invalid Chat SSH host alias",
    );
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
    ]);
    command.arg(&peer.ssh_host).arg("lam chat peer-stdio");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

fn write_wire<W: Write>(writer: &mut W, value: &Wire) -> Result<()> {
    protocol::write_frame(writer, &serde_json::to_value(value)?)?;
    writer.flush()?;
    Ok(())
}

fn read_wire<R: Read>(reader: &mut R) -> Result<Wire> {
    Ok(serde_json::from_value(protocol::read_frame(reader)?)?)
}

fn peer_request(paths: &Paths, operation: Operation) -> Result<Value> {
    super::daemon::validate_socket(&paths.peer_socket)?;
    let mut stream =
        UnixStream::connect(&paths.peer_socket).context("Chat peer daemon is not running")?;
    stream.set_read_timeout(Some(Duration::from_secs(7)))?;
    stream.set_write_timeout(Some(Duration::from_secs(7)))?;
    protocol::write_frame(
        &mut stream,
        &serde_json::to_value(Request {
            version: 1,
            operation,
        })?,
    )?;
    let response = protocol::read_frame(&mut stream)?;
    ensure!(
        response["version"] == 1 && response["ok"] == true,
        "Chat peer daemon rejected operation: {}",
        response["error"].as_str().unwrap_or("invalid response")
    );
    Ok(response["data"].clone())
}

fn sorted(mut projects: Vec<String>) -> Vec<String> {
    projects.sort();
    projects
}

fn check_hello(wire: Wire, config: &Config, initiator: bool) -> Result<PeerConfig> {
    let Wire::Hello {
        version,
        machine,
        projects,
        initiator: remote_initiator,
    } = wire
    else {
        bail!("Chat peer handshake expected hello");
    };
    ensure!(version == VERSION, "incompatible Chat peer protocol");
    protocol::validate_uuid(&machine)?;
    ensure!(
        remote_initiator != initiator,
        "duplicate Chat peer initiator"
    );
    let peer = config
        .peer(&machine)
        .context("unknown Chat peer machine")?
        .clone();
    ensure!(
        peer.initiator == initiator,
        "Chat peer initiator configuration conflicts"
    );
    ensure!(
        sorted(peer.projects.clone()) == sorted(projects),
        "Chat peer project allowlists differ"
    );
    Ok(peer)
}

pub fn run_stdio(paths: &Paths) -> Result<()> {
    let config = Config::load_existing(&paths.config)?;
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let peer = check_hello(read_wire(&mut input)?, &config, false)?;
    write_wire(
        &mut output,
        &Wire::Hello {
            version: VERSION,
            machine: config.machine.clone(),
            projects: peer.projects.clone(),
            initiator: false,
        },
    )?;
    peer_request(
        paths,
        Operation::PeerHealth {
            peer: peer.machine.clone(),
            connected: true,
        },
    )?;
    let result = (|| -> Result<()> {
        loop {
            let request = match read_wire(&mut input) {
                Ok(request) => request,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::UnexpectedEof) =>
                {
                    return Ok(())
                }
                Err(error) => return Err(error),
            };
            let response = match request {
                Wire::Pull { project, after } => {
                    ensure!(
                        peer.projects.contains(&project),
                        "unauthorized Chat peer project"
                    );
                    let events: Vec<PeerEvent> = serde_json::from_value(peer_request(
                        paths,
                        Operation::PeerExport {
                            peer: peer.machine.clone(),
                            project,
                            after,
                            limit: 100,
                        },
                    )?)?;
                    Wire::Events { events }
                }
                Wire::Push { event } => {
                    ensure!(
                        event.origin == peer.machine && peer.projects.contains(&event.project),
                        "unauthorized Chat peer event"
                    );
                    let through: u64 = serde_json::from_value(peer_request(
                        paths,
                        Operation::PeerImport {
                            peer: peer.machine.clone(),
                            event,
                        },
                    )?)?;
                    Wire::Imported { through }
                }
                Wire::Ack { project, through } => {
                    ensure!(
                        peer.projects.contains(&project),
                        "unauthorized Chat peer project"
                    );
                    peer_request(
                        paths,
                        Operation::PeerAck {
                            peer: peer.machine.clone(),
                            project,
                            through,
                        },
                    )?;
                    Wire::Acked
                }
                _ => bail!("invalid Chat peer request"),
            };
            write_wire(&mut output, &response)?;
        }
    })();
    let _ = peer_request(
        paths,
        Operation::PeerHealth {
            peer: peer.machine,
            connected: false,
        },
    );
    result
}

pub type OwnedChild = Arc<Mutex<Option<Child>>>;

fn sync_once<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    paths: &Paths,
    peer: &PeerConfig,
) -> Result<bool> {
    let mut moved = false;
    for project in &peer.projects {
        let cursor = peer_request(
            paths,
            Operation::PeerCursor {
                peer: peer.machine.clone(),
                project: project.clone(),
            },
        )?;
        let received = cursor["received"]
            .as_u64()
            .context("invalid received cursor")?;
        write_wire(
            output,
            &Wire::Pull {
                project: project.clone(),
                after: received,
            },
        )?;
        let Wire::Events { events } = read_wire(input)? else {
            bail!("Chat peer pull response is invalid")
        };
        let mut through = received;
        for event in events {
            ensure!(
                event.origin == peer.machine && event.project == *project,
                "Chat peer exported an unauthorized event"
            );
            through = serde_json::from_value(peer_request(
                paths,
                Operation::PeerImport {
                    peer: peer.machine.clone(),
                    event: Box::new(event),
                },
            )?)?;
            moved = true;
        }
        if through > received {
            write_wire(
                output,
                &Wire::Ack {
                    project: project.clone(),
                    through,
                },
            )?;
            ensure!(
                matches!(read_wire(input)?, Wire::Acked),
                "Chat peer ack response is invalid"
            );
        }
        let cursor = peer_request(
            paths,
            Operation::PeerCursor {
                peer: peer.machine.clone(),
                project: project.clone(),
            },
        )?;
        let acknowledged = cursor["acknowledged"]
            .as_u64()
            .context("invalid acknowledged cursor")?;
        let events: Vec<PeerEvent> = serde_json::from_value(peer_request(
            paths,
            Operation::PeerExport {
                peer: peer.machine.clone(),
                project: project.clone(),
                after: acknowledged,
                limit: 100,
            },
        )?)?;
        for event in events {
            let sequence = event.seq;
            write_wire(
                output,
                &Wire::Push {
                    event: Box::new(event),
                },
            )?;
            let Wire::Imported { through } = read_wire(input)? else {
                bail!("Chat peer import response is invalid")
            };
            ensure!(
                through >= sequence,
                "Chat peer did not commit event before acknowledgement"
            );
            peer_request(
                paths,
                Operation::PeerAck {
                    peer: peer.machine.clone(),
                    project: project.clone(),
                    through: sequence,
                },
            )?;
            moved = true;
        }
    }
    Ok(moved)
}

fn run_connected(
    paths: &Paths,
    config: &Config,
    peer: &PeerConfig,
    owned: &OwnedChild,
    stop: &AtomicBool,
) -> Result<()> {
    let mut child = ssh_command(peer)?
        .spawn()
        .context("cannot start Chat SSH link")?;
    let mut input = child.stdout.take().context("missing SSH stdout")?;
    let mut output = child.stdin.take().context("missing SSH stdin")?;
    let stderr = child.stderr.take().context("missing SSH stderr")?;
    let diagnostics = std::thread::spawn(move || {
        let mut reader = stderr;
        let mut saved = Vec::new();
        let mut buffer = [0_u8; 1024];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let room = 4096_usize.saturating_sub(saved.len());
            saved.extend_from_slice(&buffer[..count.min(room)]);
        }
        String::from_utf8_lossy(&saved).into_owned()
    });
    *owned.lock().unwrap() = Some(child);
    let result = (|| -> Result<()> {
        write_wire(
            &mut output,
            &Wire::Hello {
                version: VERSION,
                machine: config.machine.clone(),
                projects: peer.projects.clone(),
                initiator: true,
            },
        )?;
        let remote = check_hello(read_wire(&mut input)?, config, true)?;
        ensure!(
            remote.machine == peer.machine,
            "SSH host returned another Chat machine"
        );
        peer_request(
            paths,
            Operation::PeerHealth {
                peer: peer.machine.clone(),
                connected: true,
            },
        )?;
        while !stop.load(Ordering::SeqCst) {
            if !sync_once(&mut input, &mut output, paths, peer)? {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        Ok(())
    })();
    if !stop.load(Ordering::SeqCst) {
        let _ = peer_request(
            paths,
            Operation::PeerHealth {
                peer: peer.machine.clone(),
                connected: false,
            },
        );
    }
    let mut child = owned
        .lock()
        .unwrap()
        .take()
        .context("Chat SSH child ownership was lost")?;
    let _ = child.kill();
    let status = child.wait()?;
    drop(input);
    drop(output);
    let stderr = diagnostics.join().unwrap_or_default();
    if let Err(error) = result {
        if !stderr.trim().is_empty() {
            eprintln!("Chat peer SSH diagnostic: {}", stderr.trim());
        }
        return Err(error);
    }
    ensure!(
        status.success() || stop.load(Ordering::SeqCst),
        "Chat SSH peer exited unsuccessfully"
    );
    Ok(())
}

pub fn run_link(
    paths: Paths,
    config: Config,
    peer: PeerConfig,
    owned: OwnedChild,
    stop: Arc<AtomicBool>,
) {
    let mut failures = 0_u32;
    while !stop.load(Ordering::SeqCst) {
        match run_connected(&paths, &config, &peer, &owned, &stop) {
            Ok(()) if stop.load(Ordering::SeqCst) => break,
            Ok(()) => failures = 0,
            Err(_) if stop.load(Ordering::SeqCst) => break,
            Err(error) => {
                eprintln!("Chat peer {} disconnected: {error:#}", peer.machine);
                failures = failures.saturating_add(1);
            }
        }
        let base = (1_u64 << failures.saturating_sub(1).min(5)).min(30);
        let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) * 500 / 255;
        let delay = Duration::from_millis(base * 1000 + jitter);
        let start = std::time::Instant::now();
        while !stop.load(Ordering::SeqCst) && start.elapsed() < delay {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn stop_owned_child(owned: &OwnedChild) {
    if let Some(child) = owned.lock().unwrap().as_mut() {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Config, PeerConfig) {
        let peer = PeerConfig {
            machine: "22222222-2222-4222-8222-222222222222".into(),
            ssh_host: "macbox".into(),
            initiator: true,
            projects: vec!["33333333-3333-4333-8333-333333333333".into()],
        };
        (
            Config {
                schema_version: 1,
                machine: "11111111-1111-4111-8111-111111111111".into(),
                inline_bytes: 4096,
                batch_bytes: 8192,
                projects: Vec::new(),
                peers: vec![peer.clone()],
            },
            peer,
        )
    }

    #[test]
    fn ssh_never_disables_host_verification() {
        let (_, peer) = pair();
        let command = ssh_command(&peer).unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"BatchMode=yes".into()));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("UserKnownHostsFile=/dev/null")));
        let mut bad = peer;
        bad.ssh_host = "-oProxyCommand=bad".into();
        assert!(ssh_command(&bad).is_err());
    }

    #[test]
    fn handshake_rejects_wrong_machine_protocol_projects_or_role() {
        let (config, peer) = pair();
        let hello = || Wire::Hello {
            version: VERSION,
            machine: peer.machine.clone(),
            projects: peer.projects.clone(),
            initiator: false,
        };
        assert!(check_hello(hello(), &config, true).is_ok());
        assert!(check_hello(
            Wire::Hello {
                version: 9,
                machine: peer.machine.clone(),
                projects: peer.projects.clone(),
                initiator: false
            },
            &config,
            true
        )
        .is_err());
        assert!(check_hello(
            Wire::Hello {
                version: VERSION,
                machine: config.machine.clone(),
                projects: peer.projects.clone(),
                initiator: false
            },
            &config,
            true
        )
        .is_err());
        assert!(check_hello(
            Wire::Hello {
                version: VERSION,
                machine: peer.machine.clone(),
                projects: Vec::new(),
                initiator: false
            },
            &config,
            true
        )
        .is_err());
        assert!(check_hello(
            Wire::Hello {
                version: VERSION,
                machine: peer.machine.clone(),
                projects: peer.projects.clone(),
                initiator: true
            },
            &config,
            true
        )
        .is_err());
    }
}
