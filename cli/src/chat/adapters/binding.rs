#[cfg(target_os = "macos")]
pub(super) use super::macos_process::ProcessEvidence;

/// Authenticate only on the private participant endpoint. Connection and both
/// frames share the caller's absolute deadline, including a congested listener.
fn request(
    socket: &std::path::Path,
    credential: &str,
    operation: crate::chat::protocol::Operation,
    deadline: std::time::Instant,
) -> anyhow::Result<serde_json::Value> {
    use crate::chat::{daemon, protocol};
    use std::time::Instant;
    operation.validate()?;
    let mut stream = connect_authenticated(socket, credential, deadline)?;
    let mut stream = daemon::DeadlineStream::until(&mut stream, deadline);
    protocol::write_frame(
        &mut stream,
        &serde_json::to_value(protocol::Request {
            version: 1,
            operation,
        })?,
    )?;
    let response = protocol::read_frame(&mut stream)?;
    anyhow::ensure!(
        Instant::now() <= deadline,
        "native request deadline expired"
    );
    anyhow::ensure!(
        response["version"] == 1 && response["ok"] == true,
        "Chat participant request failed: {}",
        response["error"]
    );
    Ok(response["data"].clone())
}

#[cfg(target_os = "linux")]
fn connect_authenticated(
    socket: &std::path::Path,
    credential: &str,
    deadline: std::time::Instant,
) -> anyhow::Result<std::os::unix::net::UnixStream> {
    use crate::chat::{daemon, protocol};
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    };
    use std::time::Instant;
    daemon::validate_socket(socket)?;
    let path = socket.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    anyhow::ensure!(
        path.len() < address.sun_path.len() && !path.contains(&0),
        "invalid private socket path"
    );
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(path) {
        *target = *source as libc::c_char;
    }
    anyhow::ensure!(Instant::now() < deadline, "native request deadline expired");
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    anyhow::ensure!(fd >= 0, "cannot create private Chat connection");
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    let result = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            (std::mem::size_of::<libc::sa_family_t>() + path.len() + 1) as libc::socklen_t,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error.into());
        }
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow::anyhow!("native request deadline expired"))?;
            let mut poll = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let ready = unsafe {
                libc::poll(
                    &mut poll,
                    1,
                    remaining.as_millis().min(i32::MAX as u128) as i32,
                )
            };
            if ready < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            anyhow::ensure!(ready > 0, "private Chat connection deadline expired");
            if let Some(error) = stream.take_error()? {
                return Err(error.into());
            }
            break;
        }
    }
    stream.set_nonblocking(false)?;
    daemon::peer_identity(&stream)?;
    let mut framed = daemon::DeadlineStream::until(&mut stream, deadline);
    protocol::write_frame(
        &mut framed,
        &serde_json::json!({"version":1,"binding":credential}),
    )?;
    Ok(stream)
}

#[cfg(target_os = "macos")]
fn connect_authenticated(
    socket: &std::path::Path,
    credential: &str,
    deadline: std::time::Instant,
) -> anyhow::Result<std::os::unix::net::UnixStream> {
    use crate::chat::{daemon, protocol};
    let mut stream = connect_native_socket(socket, deadline)?;
    daemon::peer_identity(&stream)?;
    protocol::write_frame(
        &mut daemon::DeadlineStream::until(&mut stream, deadline),
        &serde_json::json!({"version":1,"binding":credential}),
    )?;
    Ok(stream)
}

fn enroll(
    files: &BindingFiles,
    socket: &std::path::Path,
    candidate: NativeBinding,
    deadline: std::time::Instant,
) -> anyhow::Result<NativeBinding> {
    candidate.process.validate()?;
    let key = candidate.locator()?;
    if let Err(error) = files.create(&candidate, deadline) {
        if error
            .downcast_ref::<std::io::Error>()
            .is_none_or(|error| error.kind() != std::io::ErrorKind::AlreadyExists)
        {
            return Err(error);
        }
    }
    files.enroll(
        &candidate,
        deadline,
        |binding| {
            let credential = format!("{key}.{}", binding.integration_secret);
            let state = request(
                socket,
                &credential,
                crate::chat::protocol::Operation::Lifecycle {},
                deadline,
            )?;
            anyhow::ensure!(
                state["session"] == serde_json::to_value(&binding.session)?,
                "native lifecycle response mismatch"
            );
            state["ended"]
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("invalid native lifecycle response"))
        },
        |binding| {
            let credential = format!("{key}.{}", binding.enrollment_secret);
            let result = request(
                socket,
                &credential,
                crate::chat::protocol::Operation::Register {},
                deadline,
            )?;
            let session = serde_json::from_value(result["session"].clone())
                .map_err(|_| anyhow::anyhow!("invalid enrollment session response"))?;
            let eligible = result["eligible"]
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("invalid enrollment eligibility response"))?;
            anyhow::ensure!(
                result["project"] == binding.project
                    && result["client"] == binding.client
                    && result["name"] == binding.name,
                "enrollment identity response mismatch"
            );
            Ok((session, eligible))
        },
    )
}

pub(in crate::chat) struct Participant {
    binding: NativeBinding,
    socket: std::path::PathBuf,
    deadline: std::time::Instant,
}

fn deliver_hook(
    files: &BindingFiles,
    binding: &NativeBinding,
    socket: &std::path::Path,
    deadline: std::time::Instant,
    output: impl FnOnce(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    use crate::chat::{
        protocol::Operation,
        types::{Attempt, ClientEvent, Handoff},
    };
    let epoch = files.next_epoch(binding, deadline)?;
    let credential = format!("{}.{}", binding.locator()?, binding.integration_secret);
    let claimed = request(
        socket,
        &credential,
        Operation::Delivery {
            event: ClientEvent::Hook,
            epoch,
        },
        deadline,
    )?;
    let attempt: Option<Attempt> = serde_json::from_value(claimed)
        .map_err(|_| anyhow::anyhow!("invalid native hook claim response"))?;
    let Some(attempt) = attempt else {
        return Ok(());
    };
    anyhow::ensure!(
        Some(&attempt.recipient) == binding.session.as_ref(),
        "native hook recipient mismatch"
    );
    binding.process.validate()?;
    let encoded = crate::chat::render::hook_output(&attempt.batch.text)?;
    anyhow::ensure!(
        encoded.len() <= 8192,
        "native hook output exceeds its bound"
    );
    let written = output(&encoded);
    let finished = request(
        socket,
        &credential,
        Operation::Finish {
            attempt: attempt.id,
            outcome: Handoff::Unknown {
                reason: crate::chat::delivery::HOOK_UNCONFIRMED.into(),
            },
        },
        deadline,
    );
    // Even a partial/failed stdout attempt is uncertain. Lost Finish is recovered
    // by the owner after the native hook deadline, never by replaying the batch.
    written?;
    finished?;
    Ok(())
}

pub(super) fn codex_check(
    paths: &crate::chat::config::Paths,
    input: &super::codex::HookInput,
    explicit_name: Option<String>,
    deadline: std::time::Instant,
    output: impl FnOnce(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let runtime = NativeRuntime::discover(std::process::id(), deadline)?;
    anyhow::ensure!(
        runtime.client == "codex",
        "native Codex hook ancestry changed"
    );
    #[cfg(target_os = "macos")]
    let relay = discover_codex_relay(&runtime.process)?;
    let directory = paths
        .database
        .parent()
        .context("missing Chat data directory")?
        .join("bindings");
    let files = BindingFiles::open(&directory)?;
    let binding = if matches!(
        input.hook_event_name.as_str(),
        "SessionStart" | "SubagentStart"
    ) {
        let config = crate::chat::config::Config::load_or_create(&paths.config)?;
        let project = config.project_for_root(&input.cwd)?;
        let name = crate::name::pick(crate::name::Sources {
            explicit: explicit_name,
            lam_name: std::env::var("LAM_NAME").ok(),
            multiplexer: None,
        })?;
        let candidate = NativeBinding::new(
            "codex",
            &runtime.version,
            input.native_id(),
            runtime.process.clone(),
            &project,
            &name,
        )?;
        let binding = enroll(&files, &paths.socket, candidate, deadline)?;
        #[cfg(target_os = "macos")]
        let binding = {
            super::codex::bind_relay(
                &relay,
                &runtime.process,
                input.native_id(),
                &binding.integration_secret,
                deadline,
            )?;
            files.attach_codex_relay(&binding, relay.clone(), deadline)?
        };
        binding
    } else {
        let key = binding_locator("codex", input.native_id(), &runtime.process)?;
        let binding = files.load(&key)?;
        anyhow::ensure!(
            binding.session.is_some()
                && binding.version == runtime.version
                && binding.process == runtime.process,
            "native hook binding is missing or changed"
        );
        #[cfg(target_os = "macos")]
        {
            anyhow::ensure!(
                binding.codex_relay.as_ref() == Some(&relay),
                "native cx relay binding is missing or changed"
            );
            relay.validate_for(&runtime.process)?;
        }
        binding
    };
    if input.hook_event_name == "PostToolUse" {
        return deliver_hook(&files, &binding, &paths.socket, deadline, output);
    }
    let credential = format!("{}.{}", binding.locator()?, binding.integration_secret);
    if matches!(input.hook_event_name.as_str(), "Stop" | "UserPromptSubmit") {
        use crate::chat::{protocol::Operation, types::ClientEvent};
        let (event, epoch) = if input.hook_event_name == "Stop" {
            (
                ClientEvent::Idle,
                files.reserve_epochs(&binding, 2, deadline)?,
            )
        } else {
            (ClientEvent::Busy, files.next_epoch(&binding, deadline)?)
        };
        request(
            &paths.socket,
            &credential,
            Operation::Observe { event, epoch },
            deadline,
        )?;
        return Ok(());
    }
    let operation = if matches!(
        input.hook_event_name.as_str(),
        "SessionEnd" | "SubagentStop"
    ) {
        crate::chat::protocol::Operation::End {}
    } else {
        crate::chat::protocol::Operation::Register {}
    };
    request(&paths.socket, &credential, operation, deadline)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn discover_codex_relay(process: &ProcessEvidence) -> anyhow::Result<CodexRelay> {
    let path = std::env::var_os("CX_LAM_RELAY")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("missing cx relay locator"))?;
    let cx = ProcessEvidence::read(process.parent)?;
    anyhow::ensure!(
        is_cx_executable(&cx.executable),
        "Codex parent is not the cx runtime"
    );
    let relay = CodexRelay { path, process: cx };
    relay.validate_for(process)?;
    Ok(relay)
}

#[cfg(target_os = "macos")]
fn is_cx_executable(path: &std::path::Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("cx")
}

pub(super) fn claude_check(
    paths: &crate::chat::config::Paths,
    input: &super::claude::HookInput,
    explicit_name: Option<String>,
    deadline: std::time::Instant,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let runtime = NativeRuntime::discover(std::process::id(), deadline)?;
    anyhow::ensure!(
        runtime.client == "claude",
        "native Claude hook ancestry changed"
    );
    let directory = paths
        .database
        .parent()
        .context("missing Chat data directory")?
        .join("bindings");
    let files = BindingFiles::open(&directory)?;
    let binding = if input.hook_event_name == "SessionStart" {
        let config = crate::chat::config::Config::load_or_create(&paths.config)?;
        let project = config.project_for_root(&input.cwd)?;
        let name = crate::name::pick(crate::name::Sources {
            explicit: explicit_name,
            lam_name: std::env::var("LAM_NAME").ok(),
            multiplexer: None,
        })?;
        let mut candidate = NativeBinding::new(
            "claude",
            &runtime.version,
            &input.session_id,
            runtime.process,
            &project,
            &name,
        )?;
        candidate.native_socket =
            super::claude::socket_locator(std::env::var_os("CLAUDE_CODE_MESSAGING_SOCKET"))?;
        enroll(&files, &paths.socket, candidate, deadline)?
    } else {
        let key = binding_locator("claude", &input.session_id, &runtime.process)?;
        let binding = files.load(&key)?;
        anyhow::ensure!(
            binding.session.is_some()
                && binding.version == runtime.version
                && binding.process == runtime.process,
            "native Claude hook binding is missing or changed"
        );
        binding
    };
    let credential = format!("{}.{}", binding.locator()?, binding.integration_secret);
    let operation = if input.hook_event_name == "SessionEnd" {
        crate::chat::protocol::Operation::End {}
    } else {
        crate::chat::protocol::Operation::Register {}
    };
    request(&paths.socket, &credential, operation, deadline)?;
    Ok(())
}

pub(super) fn pi_check(
    paths: &crate::chat::config::Paths,
    input: &super::pi::HookInput,
    explicit_name: Option<String>,
    deadline: std::time::Instant,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let runtime = NativeRuntime::discover(std::process::id(), deadline)?;
    anyhow::ensure!(
        runtime.client == "pi",
        "native Pi extension ancestry changed"
    );
    let directory = paths
        .database
        .parent()
        .context("missing Chat data directory")?
        .join("bindings");
    let files = BindingFiles::open(&directory)?;
    let binding = if input.hook_event_name == "SessionStart" {
        let config = crate::chat::config::Config::load_or_create(&paths.config)?;
        let project = config.project_for_root(&input.cwd)?;
        let name = crate::name::pick(crate::name::Sources {
            explicit: explicit_name,
            lam_name: std::env::var("LAM_NAME").ok(),
            multiplexer: None,
        })?;
        let candidate = NativeBinding::new(
            "pi",
            &runtime.version,
            &input.session_id,
            runtime.process,
            &project,
            &name,
        )?;
        enroll(&files, &paths.socket, candidate, deadline)?
    } else {
        let key = binding_locator("pi", &input.session_id, &runtime.process)?;
        let binding = files.load(&key)?;
        anyhow::ensure!(
            binding.session.is_some()
                && binding.version == runtime.version
                && binding.process == runtime.process,
            "native Pi binding is missing or changed"
        );
        binding
    };
    let credential = format!("{}.{}", binding.locator()?, binding.integration_secret);
    let operation = if input.hook_event_name == "SessionEnd" {
        crate::chat::protocol::Operation::End {}
    } else {
        crate::chat::protocol::Operation::Register {}
    };
    request(&paths.socket, &credential, operation, deadline)?;
    Ok(())
}

impl Participant {
    pub fn current(
        paths: &crate::chat::config::Paths,
        deadline: std::time::Instant,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        (|| -> anyhow::Result<Self> {
            let runtime = NativeRuntime::discover(std::process::id(), deadline)?;
            Self::for_runtime(paths, deadline, runtime)
        })().context("Chat requires a validated native participant binding; configure the client integration")
    }

    pub(in crate::chat) fn maybe_current(
        paths: &crate::chat::config::Paths,
        deadline: std::time::Instant,
    ) -> anyhow::Result<Option<Self>> {
        let runtime = NativeRuntime::find(std::process::id(), deadline)?;
        runtime
            .map(|runtime| Self::for_runtime(paths, deadline, runtime))
            .transpose()
            .map_err(|error| error.context("Chat requires a validated native participant binding; configure the client integration"))
    }

    fn for_runtime(
        paths: &crate::chat::config::Paths,
        deadline: std::time::Instant,
        runtime: NativeRuntime,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        let native_id = native_id_from(&runtime.client, |key| std::env::var(key).ok())?;
        let directory = paths
            .database
            .parent()
            .context("missing Chat data directory")?
            .join("bindings");
        anyhow::ensure!(
            directory.exists(),
            "native integration has not issued a binding"
        );
        let files = BindingFiles::open(&directory)?;
        let locator = binding_locator(&runtime.client, &native_id, &runtime.process)?;
        let binding = files.load(&locator)?;
        anyhow::ensure!(
            binding.session.is_some()
                && binding.version == runtime.version
                && binding.process == runtime.process,
            "native enrollment missing or changed"
        );
        Ok(Self {
            binding,
            socket: paths.socket.clone(),
            deadline,
        })
    }

    pub fn project(&self) -> &str {
        &self.binding.project
    }

    pub(in crate::chat) fn session(&self) -> &crate::chat::types::SessionRef {
        self.binding
            .session
            .as_ref()
            .expect("validated participant has a session")
    }

    pub fn request(
        &self,
        operation: crate::chat::protocol::Operation,
    ) -> anyhow::Result<serde_json::Value> {
        let credential = format!(
            "{}.{}",
            self.binding.locator()?,
            self.binding.participant_secret
        );
        request(&self.socket, &credential, operation, self.deadline)
    }

    pub(super) fn subscribe_pi(&self) -> anyhow::Result<std::os::unix::net::UnixStream> {
        use crate::chat::protocol::{self, Operation, Request};
        anyhow::ensure!(
            self.binding.client == "pi",
            "Pi bridge requires a Pi binding"
        );
        let credential = format!(
            "{}.{}",
            self.binding.locator()?,
            self.binding.participant_secret
        );
        let mut stream = connect_authenticated(&self.socket, &credential, self.deadline)?;
        protocol::write_frame(
            &mut crate::chat::daemon::DeadlineStream::until(&mut stream, self.deadline),
            &serde_json::to_value(Request {
                version: 1,
                operation: Operation::Subscribe {
                    project: self.binding.project.clone(),
                    cursor: None,
                    limit: 100,
                },
            })?,
        )?;
        stream.set_read_timeout(None)?;
        Ok(stream)
    }

    pub(super) fn claim_pi(&self) -> anyhow::Result<Option<crate::chat::types::Attempt>> {
        use crate::chat::{protocol::Operation, types::ClientEvent};
        anyhow::ensure!(
            self.binding.client == "pi",
            "Pi bridge requires a Pi binding"
        );
        let paths = crate::chat::config::Paths::discover()?;
        let files = BindingFiles::open(
            &paths
                .database
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing Chat data directory"))?
                .join("bindings"),
        )?;
        let epoch = files.next_epoch(
            &self.binding,
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )?;
        let credential = format!(
            "{}.{}",
            self.binding.locator()?,
            self.binding.integration_secret
        );
        let value = request(
            &self.socket,
            &credential,
            Operation::Delivery {
                event: ClientEvent::Hook,
                epoch,
            },
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )?;
        Ok(serde_json::from_value(value)?)
    }

    pub(super) fn finish_pi(
        &self,
        attempt: &str,
        outcome: crate::chat::types::Handoff,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.binding.client == "pi",
            "Pi bridge requires a Pi binding"
        );
        let credential = format!(
            "{}.{}",
            self.binding.locator()?,
            self.binding.integration_secret
        );
        request(
            &self.socket,
            &credential,
            crate::chat::protocol::Operation::Finish {
                attempt: attempt.into(),
                outcome,
            },
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )?;
        Ok(())
    }
}

fn binding_locator(
    client: &str,
    native_id: &str,
    process: &ProcessEvidence,
) -> anyhow::Result<String> {
    use sha2::Digest;
    let lifetime = format!(
        "{}:{}:{}",
        process.boot_id,
        process.pid,
        process.start_marker()
    );
    let input = serde_json::to_vec(&(client, native_id, lifetime))?;
    Ok(format!("{:x}", sha2::Sha256::digest(input)))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct NativeRuntime {
    client: String,
    version: String,
    process: ProcessEvidence,
}

#[cfg(target_os = "linux")]
impl NativeRuntime {
    fn discover(pid: u32, deadline: std::time::Instant) -> anyhow::Result<Self> {
        use anyhow::Context;
        Self::find(pid, deadline)?.context("no supported native execution ancestor")
    }

    fn find(mut pid: u32, deadline: std::time::Instant) -> anyhow::Result<Option<Self>> {
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;
        for _ in 0..64 {
            if pid <= 1 {
                return Ok(None);
            }
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let (parent, _) = parse_process_stat(&stat)?;
            anyhow::ensure!(parent != pid, "invalid native process ancestry");
            let mut cmdline = Vec::new();
            std::fs::File::open(format!("/proc/{pid}/cmdline"))?
                .take(65_537)
                .read_to_end(&mut cmdline)?;
            anyhow::ensure!(
                cmdline.len() <= 65_536,
                "native command line exceeds validation bound"
            );
            let args: Vec<_> = cmdline
                .split(|byte| *byte == 0)
                .map(|arg| arg.to_vec())
                .collect();
            // A human terminal launched by the per-user manager reaches a
            // non-dumpable `systemd --user` process. Its /proc/exe is often
            // unreadable even to the same UID. This is an ancestry boundary,
            // not a candidate agent. Everything below it was inspected first.
            if parent == 1
                && std::fs::metadata(format!("/proc/{pid}"))?.uid() == unsafe { libc::geteuid() }
                && std::fs::read_to_string(format!("/proc/{pid}/comm"))?.trim() == "systemd"
                && user_manager_command(&args)
            {
                return Ok(None);
            }
            let process = ProcessEvidence::read(pid)?;
            let client = native_client(&process.executable, &args).or_else(|| {
                pi_process_title(&process.executable, &args)
                    .then(|| std::env::var_os("LAM_CHAT_PI_ENTRYPOINT"))
                    .flatten()
                    .map(|_| "pi")
            });
            if let Some(client) = client {
                // The nearest recognized client owns the command, even if an
                // outer client exported a different session locator.
                let executable = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
                let version_args = version_args(client, &args)?;
                let borrowed: Vec<_> = version_args.iter().map(String::as_str).collect();
                let output = bounded_command(&executable, &borrowed, deadline, 256)?;
                anyhow::ensure!(output.success, "native version check failed");
                let version = parse_version(client, &output.stdout)?;
                process.validate()?;
                return Ok(Some(Self {
                    client: client.into(),
                    version,
                    process,
                }));
            }
            pid = parent;
        }
        anyhow::bail!("native process ancestry exceeds validation bound")
    }
}

#[cfg(target_os = "macos")]
impl NativeRuntime {
    fn discover(pid: u32, deadline: std::time::Instant) -> anyhow::Result<Self> {
        use anyhow::Context;
        Self::find(pid, deadline)?.context("no supported native execution ancestor")
    }

    fn find(mut pid: u32, deadline: std::time::Instant) -> anyhow::Result<Option<Self>> {
        use anyhow::Context;
        for _ in 0..64 {
            if pid <= 1 {
                return Ok(None);
            }
            if ProcessEvidence::owner(pid).with_context(|| format!("native process owner {pid}"))?
                != unsafe { libc::geteuid() }
            {
                return Ok(None);
            }
            let process = ProcessEvidence::read(pid)
                .with_context(|| format!("native process evidence {pid}"))?;
            let args = ProcessEvidence::args(pid)
                .with_context(|| format!("native process arguments {pid}"))?;
            let client = native_client(&process.executable, &args).or_else(|| {
                pi_process_title(&process.executable, &args)
                    .then(|| std::env::var_os("LAM_CHAT_PI_ENTRYPOINT"))
                    .flatten()
                    .map(|_| "pi")
            });
            if let Some(client) = client {
                let version_args = version_args(client, &args)?;
                let borrowed: Vec<_> = version_args.iter().map(String::as_str).collect();
                let output = bounded_command(&process.executable, &borrowed, deadline, 256)?;
                anyhow::ensure!(output.success, "native version check failed");
                let version = parse_version(client, &output.stdout)?;
                process.validate()?;
                return Ok(Some(Self {
                    client: client.into(),
                    version,
                    process,
                }));
            }
            anyhow::ensure!(process.parent != pid, "invalid native process ancestry");
            pid = process.parent;
        }
        anyhow::bail!("native process ancestry exceeds validation bound")
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_runtime_tests {
    use super::*;
    use crate::chat::types::{Attempt, Handoff, Registration, RenderedBatch, SessionRef};
    use std::{
        io::Read,
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::{Duration, Instant},
    };

    #[test]
    fn human_ssh_command_has_no_native_participant() {
        let found = NativeRuntime::find(
            std::process::id(),
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        )
        .unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn private_mac_binding_authenticates_exact_lifetime_and_claude_socket() {
        let dir = tempfile::Builder::new()
            .prefix("lam-chat-mac-binding-")
            .tempdir_in("/private/tmp")
            .unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let native_id = "11111111-1111-4111-8111-111111111111";
        let project = "22222222-2222-4222-8222-222222222222";
        let session = SessionRef {
            machine: "33333333-3333-4333-8333-333333333333".into(),
            incarnation: "44444444-4444-4444-8444-444444444444".into(),
        };
        let process = ProcessEvidence::read(std::process::id()).unwrap();
        let socket_dir = dir.path().join("claude-sockets");
        std::fs::create_dir(&socket_dir).unwrap();
        std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = socket_dir.join("native.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut binding = NativeBinding::new(
            "claude",
            "2.1.278",
            native_id,
            process.clone(),
            project,
            "dev",
        )
        .unwrap();
        binding.native_socket = Some(socket.clone());
        binding.bind(session.clone()).unwrap();
        files
            .create(&binding, Instant::now() + Duration::from_secs(2))
            .unwrap();

        let credential = format!(
            "{}.{}",
            binding.locator().unwrap(),
            binding.participant_secret
        );
        let validator = NativeBindings::open(&files.directory).unwrap();
        let context = validator
            .validate_with(
                crate::chat::daemon::PeerIdentity {
                    uid: process.uid,
                    pid: Some(process.pid),
                },
                &credential,
                || {
                    Ok(NativeRuntime {
                        client: "claude".into(),
                        version: "2.1.278".into(),
                        process: process.clone(),
                    })
                },
            )
            .unwrap();
        assert!(matches!(
            context,
            crate::chat::daemon::VerifiedContext::Participant(_)
        ));
        assert!(validator
            .validate_with(
                crate::chat::daemon::PeerIdentity {
                    uid: process.uid,
                    pid: Some(1)
                },
                &credential,
                || Ok(NativeRuntime {
                    client: "claude".into(),
                    version: "2.1.278".into(),
                    process: process.clone()
                }),
            )
            .is_err());

        let registration = Registration {
            session: session.clone(),
            project: project.into(),
            name: "dev".into(),
            client: "claude".into(),
            native_id: native_id.into(),
            process_start: binding.process_start(),
            eligible: true,
        };
        let attempt = Attempt {
            id: "55555555-5555-4555-8555-555555555555".into(),
            recipient: session,
            batch: RenderedBatch {
                text: "native Mac frame".into(),
                ..Default::default()
            },
        };
        let result = super::super::claude::submit_owned(
            &files.directory,
            &registration,
            Instant::now() + Duration::from_secs(2),
            || Ok(Some(attempt.clone())),
        )
        .unwrap();
        assert_eq!(result.0.id, attempt.id);
        assert!(matches!(result.1, Handoff::Unknown { .. }));
        let (mut receiver, _) = listener.accept().unwrap();
        let mut bytes = String::new();
        receiver.read_to_string(&mut bytes).unwrap();
        let frame: serde_json::Value = serde_json::from_str(bytes.trim()).unwrap();
        assert_eq!(frame["msg_id"], attempt.id);
        assert_eq!(frame["message"]["content"], "native Mac frame");
    }

    #[test]
    fn mac_codex_relay_pins_parent_socket_and_binding_protocol() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let dir = tempfile::Builder::new()
            .prefix("lam-chat-mac-cx-relay-")
            .tempdir_in("/private/tmp")
            .unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("lam.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let child = ChildGuard(
            std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let cx = ProcessEvidence::read(std::process::id()).unwrap();
        let codex = ProcessEvidence::read(child.0.id()).unwrap();
        let relay = CodexRelay {
            path: socket.clone(),
            process: cx.clone(),
        };
        relay.validate_for(&codex).unwrap();

        let secret = "a".repeat(64);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = crate::chat::protocol::read_frame(&mut stream).unwrap();
            assert_eq!(request["version"], 1);
            assert_eq!(request["operation"], "bind");
            assert_eq!(request["thread_id"], "11111111-1111-4111-8111-111111111111");
            assert_eq!(request["binding"], secret);
            crate::chat::protocol::write_frame(
                &mut stream,
                &serde_json::json!({"version":1,"ok":true}),
            )
            .unwrap();
        });
        super::super::codex::bind_relay(
            &relay,
            &codex,
            "11111111-1111-4111-8111-111111111111",
            &"a".repeat(64),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        server.join().unwrap();

        let mut changed = relay.clone();
        changed.process.start_usec += 1;
        assert!(changed.validate_for(&codex).is_err());
        let mut reparented = codex.clone();
        reparented.parent = 1;
        assert!(relay.validate_for(&reparented).is_err());
        let alias = dir.path().join("relay-link.sock");
        std::os::unix::fs::symlink(&socket, &alias).unwrap();
        let linked = CodexRelay {
            path: alias,
            process: cx,
        };
        assert!(linked.validate_for(&codex).is_err());
        assert!(is_cx_executable(std::path::Path::new(
            "/Users/owned/.local/bin/cx"
        )));
        assert!(!is_cx_executable(std::path::Path::new(
            "/Users/owned/.local/bin/codex"
        )));
    }
}

#[cfg(target_os = "linux")]
fn user_manager_command(args: &[Vec<u8>]) -> bool {
    use std::os::unix::ffi::OsStrExt;
    args.first().is_some_and(|program| {
        std::path::Path::new(std::ffi::OsStr::from_bytes(program))
            .file_name()
            .is_some_and(|name| name == "systemd")
    }) && args.get(1).is_some_and(|flag| flag == b"--user")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn version_args(client: &str, process_args: &[Vec<u8>]) -> anyhow::Result<Vec<String>> {
    match client {
        "codex" | "claude" => Ok(vec!["--version".into()]),
        "pi" => {
            let script = process_args.iter().skip(1).take(4).find_map(|arg| {
                std::str::from_utf8(arg)
                    .ok()
                    .filter(|arg| arg.contains("/pi-coding-agent/") && arg.ends_with("/cli.js"))
            });
            let script = if let Some(script) = script {
                script.to_owned()
            } else {
                let inherited = std::env::var("LAM_CHAT_PI_ENTRYPOINT")
                    .map_err(|_| anyhow::anyhow!("missing native Pi entrypoint"))?;
                let canonical = std::fs::canonicalize(inherited)?;
                let value = canonical.to_string_lossy();
                anyhow::ensure!(
                    value.contains("/pi-coding-agent/dist/") && value.ends_with("/cli.js"),
                    "Pi entrypoint does not identify the installed client"
                );
                value.into_owned()
            };
            anyhow::ensure!(
                std::path::Path::new(&script).is_absolute(),
                "Pi entrypoint is not absolute"
            );
            Ok(vec![script, "--version".into()])
        }
        _ => anyhow::bail!("unsupported native client"),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn parse_version(client: &str, output: &str) -> anyhow::Result<String> {
    #[cfg(target_os = "linux")]
    let platform = NativePlatform::Linux;
    #[cfg(target_os = "macos")]
    let platform = NativePlatform::MacOs;
    parse_version_for_platform(client, output, platform)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, Copy)]
enum NativePlatform {
    Linux,
    MacOs,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn parse_version_for_platform(
    client: &str,
    output: &str,
    platform: NativePlatform,
) -> anyhow::Result<String> {
    let version = match client {
        "codex" => output.trim().strip_prefix("codex-cli "),
        "claude" => output.trim().strip_suffix(" (Claude Code)"),
        "pi" => Some(output.trim()),
        _ => None,
    }
    .ok_or_else(|| anyhow::anyhow!("unrecognized native execution version"))?;
    let supported = match client {
        "codex" => match platform {
            NativePlatform::Linux => matches!(version, "0.153.4" | "0.154.0"),
            NativePlatform::MacOs => version == "0.155.1",
        },
        "claude" => matches!(version, "2.1.270" | "2.1.273" | "2.1.278"),
        "pi" => matches!(version, "0.85.1" | "0.86.1"),
        _ => false,
    };
    anyhow::ensure!(supported, "unsupported native execution version");
    Ok(version.into())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native_id_from(client: &str, lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    let key = match client {
        "codex" => "CODEX_THREAD_ID",
        "claude" => "LAM_CHAT_CLAUDE_SESSION_ID",
        "pi" => "PI_SESSION_ID",
        _ => anyhow::bail!("unsupported native client"),
    };
    let id = lookup(key).ok_or_else(|| anyhow::anyhow!("missing native session locator"))?;
    crate::chat::protocol::validate_uuid(&id)?;
    Ok(id)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native_client(executable: &std::path::Path, args: &[Vec<u8>]) -> Option<&'static str> {
    let path = executable.to_str()?;
    let name = executable.file_name()?.to_str()?;
    if name == "codex" {
        return Some("codex");
    }
    if name == "claude" || path.contains("/claude/versions/") {
        return Some("claude");
    }
    if matches!(name, "node" | "nodejs" | "bun")
        && args.iter().skip(1).take(4).any(|arg| {
            std::str::from_utf8(arg)
                .is_ok_and(|arg| arg.contains("/pi-coding-agent/") && arg.ends_with("/cli.js"))
        })
    {
        return Some("pi");
    }
    None
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn pi_process_title(executable: &std::path::Path, args: &[Vec<u8>]) -> bool {
    executable
        .file_name()
        .is_some_and(|name| matches!(name.to_str(), Some("node" | "nodejs" | "bun")))
        && args.first().is_some_and(|arg| arg == b"pi")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::chat) struct NativeBindings {
    files: BindingFiles,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl NativeBindings {
    pub fn open(directory: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            files: BindingFiles::open(directory)?,
        })
    }

    fn validate_with(
        &self,
        peer: crate::chat::daemon::PeerIdentity,
        credential: &str,
        observe: impl FnOnce() -> anyhow::Result<NativeRuntime>,
    ) -> anyhow::Result<crate::chat::daemon::VerifiedContext> {
        anyhow::ensure!(
            peer.uid == unsafe { libc::geteuid() },
            "native binding peer belongs to another user"
        );
        let pid = peer
            .pid
            .ok_or_else(|| anyhow::anyhow!("native binding requires kernel peer PID"))?;
        let (key, secret) = credential
            .split_once('.')
            .ok_or_else(|| anyhow::anyhow!("invalid native binding credential"))?;
        validate_binding_key(key)?;
        validate_binding_key(secret)?;
        let binding = self.files.load(key)?;
        let context = binding.context(secret)?;
        let runtime = if binding.client == "pi" {
            // Pi overwrites /proc/cmdline after startup. The trusted extension
            // checked its entrypoint/version when issuing this exact-process
            // binding; the daemon can recheck lifetime and child ancestry.
            binding.process.validate()?;
            NativeRuntime {
                client: "pi".into(),
                version: binding.version.clone(),
                process: binding.process.clone(),
            }
        } else {
            observe()?
        };
        anyhow::ensure!(
            runtime.client == binding.client
                && runtime.version == binding.version
                && runtime.process == binding.process,
            "native binding execution identity changed"
        );
        binding.process.validate_descendant(pid)?;
        Ok(context)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl crate::chat::daemon::BindingValidator for NativeBindings {
    fn status(&self) -> &'static str {
        "experimental native validator; native gates incomplete"
    }
    fn reaps_local_processes(&self) -> bool {
        true
    }
    fn validate(
        &self,
        peer: crate::chat::daemon::PeerIdentity,
        credential: &str,
    ) -> anyhow::Result<crate::chat::daemon::VerifiedContext> {
        self.validate_with(peer, credential, || {
            NativeRuntime::discover(
                peer.pid
                    .ok_or_else(|| anyhow::anyhow!("native binding requires kernel peer PID"))?,
                std::time::Instant::now() + std::time::Duration::from_millis(500),
            )
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::chat) fn registration_live(registration: &crate::chat::types::Registration) -> bool {
    registration_process(registration).is_ok()
}

#[cfg(unix)]
struct NativeOutput {
    success: bool,
    stdout: String,
}

#[cfg(unix)]
struct NativeChild(std::process::Child);

#[cfg(unix)]
impl Drop for NativeChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// Bounds the owned helper's output and total wait; never interrupts its target
/// native agent or retains arbitrary native stderr in model-facing errors.
#[cfg(unix)]
fn bounded_command(
    program: &std::path::Path,
    args: &[&str],
    deadline: std::time::Instant,
    max_output: usize,
) -> anyhow::Result<NativeOutput> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    anyhow::ensure!(Instant::now() < deadline, "native command deadline expired");
    let mut child = NativeChild(
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("missing native helper output"))?;
    let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
    anyhow::ensure!(
        flags >= 0
            && unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                >= 0,
        "cannot bound native helper output"
    );
    let mut output = Vec::new();
    let mut ended = false;
    loop {
        let mut bytes = [0; 512];
        loop {
            match stdout.read(&mut bytes) {
                Ok(0) => {
                    ended = true;
                    break;
                }
                Ok(count) => {
                    output.extend_from_slice(&bytes[..count]);
                    anyhow::ensure!(
                        output.len() <= max_output,
                        "native helper output exceeds its bound"
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if let Some(status) = child.0.try_wait()? {
            if ended {
                return Ok(NativeOutput {
                    success: status.success(),
                    stdout: String::from_utf8(output)?,
                });
            }
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow::anyhow!("native command deadline expired"))?;
        std::thread::sleep(remaining.min(Duration::from_millis(2)));
    }
}

/// Kernel process evidence is one part of a binding, not a thread credential.
/// Attached terminal clients may reconnect to the same live execution backend.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessEvidence {
    pub pid: u32,
    pub boot_id: String,
    pub start_ticks: u64,
    pub executable: std::path::PathBuf,
    pub uid: u32,
}

#[cfg(target_os = "linux")]
impl ProcessEvidence {
    pub fn start_marker(&self) -> u64 {
        self.start_ticks
    }

    pub fn read(pid: u32) -> anyhow::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(pid > 0, "missing native process identity");
        let path = std::path::PathBuf::from(format!("/proc/{pid}"));
        let stat = std::fs::read_to_string(path.join("stat"))?;
        let (_, start_ticks) = parse_process_stat(&stat)?;
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_owned();
        crate::chat::protocol::validate_uuid(&boot_id)?;
        let evidence = Self {
            pid,
            boot_id,
            start_ticks,
            executable: std::fs::read_link(path.join("exe"))?,
            uid: std::fs::metadata(&path)?.uid(),
        };
        anyhow::ensure!(
            evidence.uid == unsafe { libc::geteuid() },
            "native process belongs to another user"
        );
        let (_, after) = parse_process_stat(&std::fs::read_to_string(path.join("stat"))?)?;
        anyhow::ensure!(
            after == start_ticks,
            "native process changed during validation"
        );
        Ok(evidence)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            Self::read(self.pid)? == *self,
            "native process lifetime changed"
        );
        Ok(())
    }

    pub fn validate_descendant(&self, mut pid: u32) -> anyhow::Result<()> {
        self.validate()?;
        for _ in 0..64 {
            if pid == self.pid {
                return self.validate();
            }
            anyhow::ensure!(pid > 1, "command is outside its native process");
            Self::read(pid)?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let (parent, _) = parse_process_stat(&stat)?;
            anyhow::ensure!(parent != pid, "invalid native process ancestry");
            pid = parent;
        }
        anyhow::bail!("native process ancestry exceeds the validation bound")
    }
}

#[cfg(target_os = "linux")]
fn backend_socket_path(process: &ProcessEvidence) -> anyhow::Result<std::path::PathBuf> {
    use std::collections::HashSet;
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    process.validate()?;
    let mut owned_inodes = HashSet::new();
    for entry in std::fs::read_dir(format!("/proc/{}/fd", process.pid))?.take(2049) {
        anyhow::ensure!(
            owned_inodes.len() < 2048,
            "native descriptor search exceeds bound"
        );
        let entry = entry?;
        if let Ok(link) = std::fs::read_link(entry.path()) {
            if let Some(inode) = link
                .to_str()
                .and_then(|value| value.strip_prefix("socket:["))
                .and_then(|value| value.strip_suffix(']'))
            {
                owned_inodes.insert(inode.to_owned());
            }
        }
    }
    let mut listing = String::new();
    std::fs::File::open(format!("/proc/{}/net/unix", process.pid))?
        .take(4_194_305)
        .read_to_string(&mut listing)?;
    anyhow::ensure!(
        listing.len() <= 4_194_304,
        "native socket table exceeds bound"
    );
    let mut matches = Vec::new();
    for line in listing.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 8
            || fields[3] != "00010000"
            || fields[4] != "0001"
            || fields[5] != "01"
            || !owned_inodes.contains(fields[6])
        {
            continue;
        }
        let path = std::path::PathBuf::from(fields[7]);
        if path
            .file_name()
            .is_some_and(|name| name == "app-server-control.sock")
        {
            crate::chat::daemon::validate_socket(&path)?;
            let parent = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing native socket directory"))?;
            let metadata = parent.symlink_metadata()?;
            anyhow::ensure!(
                metadata.is_dir()
                    && metadata.uid() == process.uid
                    && metadata.permissions().mode() & 0o077 == 0,
                "native socket directory must be private"
            );
            matches.push(path);
        }
    }
    anyhow::ensure!(
        matches.len() == 1,
        "native backend has no unique private control socket"
    );
    process.validate()?;
    Ok(matches.remove(0))
}

#[cfg(all(test, target_os = "linux"))]
pub(in crate::chat) static BACKEND_SOCKET_TEST_LOCK: std::sync::Mutex<()> =
    std::sync::Mutex::new(());

#[cfg(target_os = "linux")]
pub(in crate::chat) fn codex_queue_target(
    directory: &std::path::Path,
    registration: &crate::chat::types::Registration,
) -> anyhow::Result<(ProcessEvidence, std::path::PathBuf)> {
    use crate::chat::protocol;
    anyhow::ensure!(registration.client == "codex", "queue target is not Codex");
    protocol::validate_session(&registration.session)?;
    protocol::validate_uuid(&registration.native_id)?;
    let process = registration_process(registration)?;
    let files = BindingFiles::open(directory)?;
    let key = binding_locator("codex", &registration.native_id, &process)?;
    let binding = files.load(&key)?;
    anyhow::ensure!(
        binding.process == process
            && binding.client == registration.client
            && binding.native_id == registration.native_id
            && binding.session.as_ref() == Some(&registration.session)
            && binding.project == registration.project
            && binding.process_start() == registration.process_start
            && matches!(binding.version.as_str(), "0.153.4" | "0.154.0"),
        "native queue binding changed"
    );
    let path = backend_socket_path(&process)?;
    Ok((process, path))
}

#[cfg(target_os = "linux")]
fn registration_process(
    registration: &crate::chat::types::Registration,
) -> anyhow::Result<ProcessEvidence> {
    use crate::chat::protocol;
    let mut lifetime = registration.process_start.split(':');
    let boot = lifetime.next().unwrap_or_default();
    let pid = lifetime.next().unwrap_or_default().parse::<u32>()?;
    let ticks = lifetime.next().unwrap_or_default().parse::<u64>()?;
    anyhow::ensure!(lifetime.next().is_none(), "invalid native process lifetime");
    protocol::validate_uuid(boot)?;
    let process = ProcessEvidence::read(pid)?;
    anyhow::ensure!(
        process.boot_id == boot && process.start_ticks == ticks,
        "native process lifetime changed"
    );
    Ok(process)
}

#[cfg(target_os = "macos")]
fn registration_process(
    registration: &crate::chat::types::Registration,
) -> anyhow::Result<ProcessEvidence> {
    let mut lifetime = registration.process_start.split(':');
    let boot = lifetime.next().unwrap_or_default();
    let pid = lifetime.next().unwrap_or_default().parse::<u32>()?;
    let start = lifetime.next().unwrap_or_default().parse::<u64>()?;
    anyhow::ensure!(lifetime.next().is_none(), "invalid native process lifetime");
    crate::chat::protocol::validate_uuid(boot)?;
    let process = ProcessEvidence::read(pid)?;
    anyhow::ensure!(
        process.boot_id == boot && process.start_marker() == start,
        "native process lifetime changed"
    );
    Ok(process)
}

#[cfg(target_os = "linux")]
pub(in crate::chat) fn claude_target(
    directory: &std::path::Path,
    registration: &crate::chat::types::Registration,
) -> anyhow::Result<(ProcessEvidence, std::path::PathBuf)> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, PermissionsExt},
    };
    anyhow::ensure!(registration.client == "claude", "peer target is not Claude");
    crate::chat::protocol::validate_session(&registration.session)?;
    crate::chat::protocol::validate_uuid(&registration.native_id)?;
    let process = registration_process(registration)?;
    let files = BindingFiles::open(directory)?;
    let key = binding_locator("claude", &registration.native_id, &process)?;
    let binding = files.load(&key)?;
    anyhow::ensure!(
        binding.process == process
            && binding.client == registration.client
            && binding.native_id == registration.native_id
            && binding.session.as_ref() == Some(&registration.session)
            && binding.project == registration.project
            && binding.process_start() == registration.process_start
            && matches!(binding.version.as_str(), "2.1.270" | "2.1.273" | "2.1.278"),
        "native Claude binding changed"
    );
    let path = binding
        .native_socket
        .ok_or_else(|| anyhow::anyhow!("native Claude peer socket is unavailable"))?;
    crate::chat::config::validate_path_components(&path)?;
    crate::chat::daemon::validate_socket(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing native socket directory"))?;
    let metadata = parent.symlink_metadata()?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == process.uid
            && metadata.permissions().mode() & 0o077 == 0,
        "native Claude socket directory must be private"
    );
    let mut listing = String::new();
    std::fs::File::open(format!("/proc/{}/net/unix", process.pid))?
        .take(4_194_305)
        .read_to_string(&mut listing)?;
    anyhow::ensure!(
        listing.len() <= 4_194_304,
        "native socket table exceeds bound"
    );
    let mut inodes = listing.lines().skip(1).filter_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        (fields.len() == 8
            && fields[3] == "00010000"
            && fields[4] == "0001"
            && fields[5] == "01"
            && fields[7] == path.to_str().unwrap_or(""))
        .then_some(fields[6].to_owned())
    });
    let inode = inodes
        .next()
        .ok_or_else(|| anyhow::anyhow!("native Claude socket is not listening"))?;
    anyhow::ensure!(inodes.next().is_none(), "native Claude socket is ambiguous");
    let expected = format!("socket:[{inode}]");
    let mut owned = false;
    for (index, entry) in std::fs::read_dir(format!("/proc/{}/fd", process.pid))?.enumerate() {
        anyhow::ensure!(index < 2048, "native descriptor search exceeds bound");
        if std::fs::read_link(entry?.path())
            .is_ok_and(|link| link == std::path::Path::new(&expected))
        {
            owned = true;
            break;
        }
    }
    anyhow::ensure!(owned, "native Claude process does not own peer listener");
    process.validate()?;
    Ok((process, path))
}

#[cfg(target_os = "macos")]
pub(in crate::chat) fn claude_target(
    directory: &std::path::Path,
    registration: &crate::chat::types::Registration,
) -> anyhow::Result<(ProcessEvidence, std::path::PathBuf)> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    anyhow::ensure!(registration.client == "claude", "peer target is not Claude");
    crate::chat::protocol::validate_session(&registration.session)?;
    crate::chat::protocol::validate_uuid(&registration.native_id)?;
    let process = registration_process(registration)?;
    let files = BindingFiles::open(directory)?;
    let key = binding_locator("claude", &registration.native_id, &process)?;
    let binding = files.load(&key)?;
    anyhow::ensure!(
        binding.process == process
            && binding.client == registration.client
            && binding.native_id == registration.native_id
            && binding.session.as_ref() == Some(&registration.session)
            && binding.project == registration.project
            && binding.process_start() == registration.process_start
            && binding.version == "2.1.278",
        "native Claude binding changed"
    );
    let path = binding
        .native_socket
        .ok_or_else(|| anyhow::anyhow!("native Claude peer socket is unavailable"))?;
    crate::chat::config::validate_path_components(&path)?;
    crate::chat::daemon::validate_socket(&path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing native socket directory"))?;
    let metadata = parent.symlink_metadata()?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == process.uid
            && metadata.permissions().mode() & 0o077 == 0,
        "native Claude socket directory must be private"
    );
    process.validate()?;
    Ok((process, path))
}

#[cfg(target_os = "linux")]
pub(super) fn reserve_queue_epoch(
    directory: &std::path::Path,
    registration: &crate::chat::types::Registration,
    deadline: std::time::Instant,
) -> anyhow::Result<u64> {
    let (process, _) = codex_queue_target(directory, registration)?;
    let files = BindingFiles::open(directory)?;
    let key = binding_locator("codex", &registration.native_id, &process)?;
    let binding = files.load(&key)?;
    files.next_epoch(&binding, deadline)
}

#[cfg(all(test, target_os = "linux"))]
pub(in crate::chat) fn install_test_binding(
    directory: &std::path::Path,
    registration: &crate::chat::types::Registration,
) -> anyhow::Result<()> {
    let process = ProcessEvidence::read(std::process::id())?;
    anyhow::ensure!(
        registration.client == "codex"
            && registration.process_start
                == format!(
                    "{}:{}:{}",
                    process.boot_id, process.pid, process.start_ticks
                ),
        "owned test registration does not match process"
    );
    let mut binding = NativeBinding::new(
        "codex",
        "0.153.4",
        &registration.native_id,
        process,
        &registration.project,
        &registration.name,
    )?;
    binding.bind(registration.session.clone())?;
    BindingFiles::open(directory)?.create(
        &binding,
        std::time::Instant::now() + std::time::Duration::from_secs(1),
    )
}

#[cfg(target_os = "linux")]
pub(super) fn connect_native_socket(
    path: &std::path::Path,
    deadline: std::time::Instant,
) -> anyhow::Result<std::os::unix::net::UnixStream> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    };
    crate::chat::daemon::validate_socket(path)?;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    anyhow::ensure!(
        bytes.len() < address.sun_path.len() && !bytes.contains(&0),
        "invalid native socket path"
    );
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    anyhow::ensure!(
        std::time::Instant::now() < deadline,
        "native connection deadline expired"
    );
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    anyhow::ensure!(fd >= 0, "cannot create native connection");
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let connected = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        anyhow::ensure!(
            error.raw_os_error() == Some(libc::EINPROGRESS),
            "native connection failed"
        );
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow::anyhow!("native connection deadline expired"))?;
            let mut poll = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let ready = unsafe {
                libc::poll(
                    &mut poll,
                    1,
                    remaining.as_millis().min(i32::MAX as u128) as i32,
                )
            };
            if ready < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            anyhow::ensure!(ready > 0, "native connection deadline expired");
            if let Some(error) = stream.take_error()? {
                return Err(error.into());
            }
            break;
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(target_os = "macos")]
pub(super) fn connect_native_socket(
    path: &std::path::Path,
    deadline: std::time::Instant,
) -> anyhow::Result<std::os::unix::net::UnixStream> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    };
    crate::chat::daemon::validate_socket(path)?;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    anyhow::ensure!(
        bytes.len() < address.sun_path.len() && !bytes.contains(&0) && length <= u8::MAX as usize,
        "invalid native socket path"
    );
    address.sun_len = length as u8;
    address.sun_family = libc::AF_UNIX as u8;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    anyhow::ensure!(
        std::time::Instant::now() < deadline,
        "native connection deadline expired"
    );
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    anyhow::ensure!(fd >= 0, "cannot create native connection");
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    anyhow::ensure!(
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
        "cannot make native connection close-on-exec"
    );
    stream.set_nonblocking(true)?;
    let connected = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            length as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        anyhow::ensure!(
            error.raw_os_error() == Some(libc::EINPROGRESS),
            "native connection failed"
        );
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow::anyhow!("native connection deadline expired"))?;
            let mut poll = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let ready = unsafe {
                libc::poll(
                    &mut poll,
                    1,
                    remaining.as_millis().min(i32::MAX as u128) as i32,
                )
            };
            if ready < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            anyhow::ensure!(ready > 0, "native connection deadline expired");
            if let Some(error) = stream.take_error()? {
                return Err(error.into());
            }
            break;
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(target_os = "linux")]
fn parse_process_stat(stat: &str) -> anyhow::Result<(u32, u64)> {
    use anyhow::Context;
    let (_, fields) = stat
        .rsplit_once(") ")
        .context("invalid native process stat")?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    anyhow::ensure!(fields.len() >= 20, "incomplete native process stat");
    anyhow::ensure!(
        !matches!(fields[0], "Z" | "X" | "x"),
        "native process has ended"
    );
    let parent = fields[1].parse()?;
    let start = fields[19].parse()?;
    anyhow::ensure!(start > 0, "missing native process birth evidence");
    Ok((parent, start))
}

/// Private integration record. Its secrets never enter model context or argv.
/// Native IDs locate the record; a matching secret selects a fixed authority.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeBinding {
    schema: u32,
    client: String,
    version: String,
    native_id: String,
    process: ProcessEvidence,
    project: String,
    name: String,
    session: Option<crate::chat::types::SessionRef>,
    eligible: bool,
    #[serde(default)]
    native_socket: Option<std::path::PathBuf>,
    #[serde(default)]
    codex_relay: Option<CodexRelay>,
    #[serde(default)]
    observation_epoch: u64,
    enrollment_secret: String,
    participant_secret: String,
    integration_secret: String,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl NativeBinding {
    fn new(
        client: &str,
        version: &str,
        native_id: &str,
        process: ProcessEvidence,
        project: &str,
        name: &str,
    ) -> anyhow::Result<Self> {
        let secret = || {
            format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )
        };
        let record = Self {
            schema: 1,
            client: client.into(),
            version: version.into(),
            native_id: native_id.into(),
            process,
            project: project.into(),
            name: name.into(),
            session: None,
            eligible: true,
            native_socket: None,
            codex_relay: None,
            observation_epoch: 0,
            enrollment_secret: secret(),
            participant_secret: secret(),
            integration_secret: secret(),
        };
        record.validate_shape()?;
        Ok(record)
    }

    fn validate_shape(&self) -> anyhow::Result<()> {
        use crate::chat::protocol;
        anyhow::ensure!(self.schema == 1, "unsupported private binding schema");
        anyhow::ensure!(
            self.observation_epoch <= i64::MAX as u64,
            "invalid native observation epoch"
        );
        self.client_kind()?;
        if let Some(path) = &self.native_socket {
            anyhow::ensure!(
                self.client == "claude",
                "native socket belongs to another client"
            );
            anyhow::ensure!(path.is_absolute(), "native socket path is not absolute");
        }
        #[cfg(target_os = "linux")]
        if self.codex_relay.is_some() {
            anyhow::ensure!(self.client == "codex", "cx relay belongs to another client");
            anyhow::bail!("cx relay is not supported on Linux");
        }
        #[cfg(target_os = "macos")]
        if let Some(relay) = &self.codex_relay {
            anyhow::ensure!(self.client == "codex", "cx relay belongs to another client");
            anyhow::ensure!(
                self.version == "0.155.1" && is_cx_executable(&relay.process.executable),
                "invalid cx relay execution"
            );
            relay.validate_shape()?;
        }
        protocol::validate_uuid(&self.native_id)?;
        protocol::validate_uuid(&self.process.boot_id)?;
        protocol::validate_project(&self.project)?;
        anyhow::ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 256,
            "invalid native binding name"
        );
        anyhow::ensure!(
            !self.version.is_empty() && self.version.len() <= 64,
            "invalid native binding version"
        );
        anyhow::ensure!(
            self.process.pid > 0
                && self.process.start_marker() > 0
                && self.process.executable.is_absolute(),
            "invalid native process evidence"
        );
        if let Some(session) = &self.session {
            protocol::validate_session(session)?;
        }
        for secret in [
            &self.enrollment_secret,
            &self.participant_secret,
            &self.integration_secret,
        ] {
            validate_binding_key(secret)?;
        }
        anyhow::ensure!(
            self.enrollment_secret != self.participant_secret
                && self.enrollment_secret != self.integration_secret
                && self.participant_secret != self.integration_secret,
            "private binding authorities must have distinct credentials"
        );
        Ok(())
    }

    fn client_kind(&self) -> anyhow::Result<crate::chat::registry::ClientKind> {
        use crate::chat::registry::ClientKind;
        match self.client.as_str() {
            "codex" => Ok(ClientKind::Codex),
            "claude" => Ok(ClientKind::Claude),
            "pi" => Ok(ClientKind::Pi),
            _ => anyhow::bail!("unsupported native binding client"),
        }
    }

    fn process_start(&self) -> String {
        format!(
            "{}:{}:{}",
            self.process.boot_id,
            self.process.pid,
            self.process.start_marker()
        )
    }

    fn locator(&self) -> anyhow::Result<String> {
        binding_locator(&self.client, &self.native_id, &self.process)
    }

    fn bind(&mut self, session: crate::chat::types::SessionRef) -> anyhow::Result<()> {
        crate::chat::protocol::validate_session(&session)?;
        anyhow::ensure!(
            self.session
                .as_ref()
                .is_none_or(|current| current == &session),
            "cannot replace a bound incarnation"
        );
        self.session = Some(session);
        Ok(())
    }

    fn context(&self, secret: &str) -> anyhow::Result<crate::chat::daemon::VerifiedContext> {
        use crate::chat::{daemon::VerifiedContext, registry::NativeEvidence, types::Registration};
        self.validate_shape()?;
        if self.session.is_none() && secret == self.enrollment_secret {
            return Ok(VerifiedContext::Enrollment {
                project: self.project.clone(),
                name: self.name.clone(),
                eligible: self.eligible,
                evidence: NativeEvidence {
                    client: self.client_kind()?,
                    native_id: self.native_id.clone(),
                    process_start: self.process_start(),
                },
            });
        }
        let session = self
            .session
            .clone()
            .ok_or_else(|| anyhow::anyhow!("native binding enrollment is incomplete"))?;
        let registration = Registration {
            session,
            project: self.project.clone(),
            name: self.name.clone(),
            client: self.client.clone(),
            native_id: self.native_id.clone(),
            process_start: self.process_start(),
            eligible: self.eligible,
        };
        if secret == self.participant_secret {
            return Ok(VerifiedContext::Participant(registration));
        }
        if secret == self.integration_secret {
            return Ok(VerifiedContext::Integration(registration));
        }
        anyhow::bail!("invalid native binding credential")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CodexRelay {
    pub path: std::path::PathBuf,
    pub process: ProcessEvidence,
}

#[cfg(target_os = "macos")]
impl CodexRelay {
    fn validate_shape(&self) -> anyhow::Result<()> {
        crate::chat::protocol::validate_uuid(&self.process.boot_id)?;
        anyhow::ensure!(
            self.path.is_absolute()
                && self.process.pid > 0
                && self.process.start_marker() > 0
                && self.process.executable.is_absolute(),
            "invalid cx relay evidence"
        );
        Ok(())
    }

    pub(super) fn validate_for(&self, codex: &ProcessEvidence) -> anyhow::Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        self.validate_shape()?;
        self.process.validate()?;
        codex.validate()?;
        anyhow::ensure!(
            codex.parent == self.process.pid && codex.uid == self.process.uid,
            "Codex process is outside its cx runtime"
        );
        crate::chat::config::validate_path_components(&self.path)?;
        crate::chat::daemon::validate_socket(&self.path)?;
        let directory = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing cx relay directory"))?;
        let metadata = directory.symlink_metadata()?;
        anyhow::ensure!(
            metadata.is_dir()
                && metadata.uid() == self.process.uid
                && metadata.permissions().mode() & 0o077 == 0,
            "cx relay directory must be private"
        );
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn validate_binding_key(key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        key.len() == 64
            && key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid private binding key"
    );
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct BindingFiles {
    directory: std::path::PathBuf,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl BindingFiles {
    fn open(directory: &std::path::Path) -> anyhow::Result<Self> {
        crate::chat::config::validate_path_components(directory)?;
        crate::chat::config::ensure_private_dir(directory, "Chat native bindings")?;
        Ok(Self {
            directory: directory.to_owned(),
        })
    }

    fn next_epoch(
        &self,
        binding: &NativeBinding,
        deadline: std::time::Instant,
    ) -> anyhow::Result<u64> {
        self.reserve_epochs(binding, 1, deadline)
    }

    #[cfg(target_os = "macos")]
    fn attach_codex_relay(
        &self,
        binding: &NativeBinding,
        relay: CodexRelay,
        deadline: std::time::Instant,
    ) -> anyhow::Result<NativeBinding> {
        relay.validate_for(&binding.process)?;
        let key = binding.locator()?;
        let _lock = self.lock(&key, deadline)?;
        let mut current = self.load(&key)?;
        anyhow::ensure!(
            current.client == "codex"
                && current.version == "0.155.1"
                && current.process == binding.process
                && current.session == binding.session
                && current.integration_secret == binding.integration_secret,
            "native cx relay enrollment changed"
        );
        current.codex_relay = Some(relay);
        current.validate_shape()?;
        self.replace(&key, &current)?;
        Ok(current)
    }

    fn reserve_epochs(
        &self,
        binding: &NativeBinding,
        count: u64,
        deadline: std::time::Instant,
    ) -> anyhow::Result<u64> {
        anyhow::ensure!((1..=2).contains(&count), "invalid native epoch reservation");
        let key = binding.locator()?;
        let _lock = self.lock(&key, deadline)?;
        let mut current = self.load(&key)?;
        anyhow::ensure!(
            current.session.is_some()
                && current.session == binding.session
                && current.integration_secret == binding.integration_secret,
            "native observation binding changed"
        );
        current.observation_epoch = current
            .observation_epoch
            .checked_add(count)
            .ok_or_else(|| anyhow::anyhow!("native observation epoch exhausted"))?;
        current.validate_shape()?;
        self.replace(&key, &current)?;
        Ok(current.observation_epoch - count + 1)
    }

    fn create(&self, binding: &NativeBinding, deadline: std::time::Instant) -> anyhow::Result<()> {
        use std::ffi::CString;
        use std::io::Write;
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        binding.validate_shape()?;
        let key = binding.locator()?;
        let _lock = self.lock(&key, deadline)?;
        let encoded = serde_json::to_vec(binding)?;
        anyhow::ensure!(
            encoded.len() <= 16_384,
            "private binding record exceeds its bound"
        );
        let path = self.directory.join(format!("{key}.json"));
        let temporary = self
            .directory
            .join(format!("{key}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        let result = (|| -> anyhow::Result<()> {
            file.write_all(&encoded)?;
            file.sync_all()?;
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "native enrollment deadline expired"
            );
            let source = CString::new(temporary.as_os_str().as_bytes())?;
            let target = CString::new(path.as_os_str().as_bytes())?;
            // Readers never see a partial record. An existing record (including
            // a symlink) is never replaced, even outside the cooperative lock.
            #[cfg(target_os = "linux")]
            let renamed = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    libc::AT_FDCWD,
                    target.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(target_os = "macos")]
            let renamed = unsafe {
                libc::renameatx_np(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    libc::AT_FDCWD,
                    target.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if renamed != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            std::fs::File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    /// Initial publication and enrollment share the same private record lock.
    fn lock(&self, key: &str, deadline: std::time::Instant) -> anyhow::Result<std::fs::File> {
        use std::os::{
            fd::AsRawFd,
            unix::fs::{MetadataExt, OpenOptionsExt},
        };
        use std::time::{Duration, Instant};
        validate_binding_key(key)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.directory.join(format!("{key}.lock")))?;
        let metadata = lock.metadata()?;
        anyhow::ensure!(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o777 == 0o600
                && metadata.nlink() == 1,
            "invalid private enrollment lock"
        );
        loop {
            anyhow::ensure!(
                Instant::now() < deadline,
                "native enrollment deadline expired"
            );
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            anyhow::ensure!(
                error.kind() == std::io::ErrorKind::WouldBlock,
                "cannot lock native enrollment"
            );
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow::anyhow!("native enrollment deadline expired"))?;
            std::thread::sleep(remaining.min(Duration::from_millis(2)));
        }
        Ok(lock)
    }

    /// Only the daemon's Store owner assigns the session. Concurrent native
    /// hooks serialize this handshake; a live bound file is never re-enrolled.
    fn enroll(
        &self,
        candidate: &NativeBinding,
        deadline: std::time::Instant,
        has_ended: impl FnOnce(&NativeBinding) -> anyhow::Result<bool>,
        register: impl FnOnce(&NativeBinding) -> anyhow::Result<(crate::chat::types::SessionRef, bool)>,
    ) -> anyhow::Result<NativeBinding> {
        use std::time::Instant;
        let key = candidate.locator()?;
        let _lock = self.lock(&key, deadline)?;
        let mut binding = self.load(&key)?;
        anyhow::ensure!(
            binding.project == candidate.project
                && binding.process == candidate.process
                && binding.version == candidate.version,
            "native enrollment configuration changed"
        );
        if binding.session.is_some() {
            if !has_ended(&binding)? {
                return Ok(binding);
            }
            // Only a trusted startup and the exact owner's confirmed End may
            // rotate credentials. Publish before Register so its authentication
            // sees the fresh enrollment secret; failures can resume this file.
            binding = NativeBinding::new(
                &candidate.client,
                &candidate.version,
                &candidate.native_id,
                candidate.process.clone(),
                &candidate.project,
                &candidate.name,
            )?;
            binding.native_socket = candidate.native_socket.clone();
            self.replace(&key, &binding)?;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "native enrollment deadline expired"
        );
        let (session, eligible) = register(&binding)?;
        binding.bind(session)?;
        binding.eligible = eligible;
        self.replace(&key, &binding)?;
        Ok(binding)
    }

    /// Caller holds the record lock and has validated the existing final file.
    fn replace(&self, key: &str, binding: &NativeBinding) -> anyhow::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let encoded = serde_json::to_vec(binding)?;
        anyhow::ensure!(
            encoded.len() <= 16_384,
            "private binding record exceeds its bound"
        );
        let temporary = self
            .directory
            .join(format!("{key}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let result = (|| -> anyhow::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            std::fs::rename(&temporary, self.directory.join(format!("{key}.json")))?;
            std::fs::File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    fn load(&self, key: &str) -> anyhow::Result<NativeBinding> {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        validate_binding_key(key)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.directory.join(format!("{key}.json")))?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o777 == 0o600
                && metadata.nlink() == 1
                && metadata.len() <= 16_384,
            "invalid private native binding file"
        );
        let mut bytes = Vec::new();
        file.take(16_385).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= 16_384,
            "private binding record exceeds its bound"
        );
        let binding: NativeBinding = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid private native binding record"))?;
        binding.validate_shape()?;
        anyhow::ensure!(binding.locator()? == key, "native binding locator mismatch");
        Ok(binding)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct ObservedBindings(NativeBindings);
    impl crate::chat::daemon::BindingValidator for ObservedBindings {
        fn validate(
            &self,
            peer: crate::chat::daemon::PeerIdentity,
            credential: &str,
        ) -> anyhow::Result<crate::chat::daemon::VerifiedContext> {
            self.0.validate_with(peer, credential, || {
                Ok(NativeRuntime {
                    client: "codex".into(),
                    version: "0.153.4".into(),
                    process: ProcessEvidence::read(std::process::id())?,
                })
            })
        }
    }

    #[test]
    fn native_hook_delivery_keeps_exclusion_during_output_and_finishes_unknown() {
        use crate::chat::{
            protocol::Operation,
            types::{Draft, FeedEvent, Target},
        };
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let socket = dir.path().join("participant.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let owner = crate::chat::daemon::tests::owned_connections(
            listener,
            std::sync::Arc::new(ObservedBindings(
                NativeBindings::open(&files.directory).unwrap(),
            )),
            dir.path().join("chat.sqlite3"),
            9,
        );
        let registered = enroll(
            &files,
            &socket,
            binding(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        let participant = Participant {
            binding: registered,
            socket: socket.clone(),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let send = |key: &str| {
            participant.request(Operation::Send {
                draft: Draft {
                    key: key.into(),
                    project: participant.project().into(),
                    to: vec![Target::Agent(participant.binding.session.clone().unwrap())],
                    body: key.into(),
                    reply_to: None,
                },
            })
        };
        send("owned first body").unwrap();
        let mut wrote_first = false;
        deliver_hook(
            &files,
            &participant.binding,
            &socket,
            participant.deadline,
            |output| {
                let decoded: serde_json::Value = serde_json::from_str(output)?;
                assert!(decoded["hookSpecificOutput"]["additionalContext"]
                    .as_str()
                    .unwrap()
                    .contains("owned first body"));
                assert!(output.len() <= 8192);
                wrote_first = true;
                send("arrived while writing")?;
                deliver_hook(
                    &files,
                    &participant.binding,
                    &socket,
                    participant.deadline,
                    |_| panic!("concurrent hook overtook output"),
                )?;
                Ok(())
            },
        )
        .unwrap();
        assert!(wrote_first, "hook never emitted the claimed batch");
        assert!(deliver_hook(
            &files,
            &participant.binding,
            &socket,
            participant.deadline,
            |output| {
                assert!(output.contains("arrived while writing"));
                assert!(!output.contains("owned first body"));
                anyhow::bail!("owned failed output")
            }
        )
        .is_err());
        deliver_hook(
            &files,
            &participant.binding,
            &socket,
            participant.deadline,
            |_| panic!("Unknown was replayed"),
        )
        .unwrap();
        owner.join().unwrap();
        let store = crate::chat::store::Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        let events = store
            .feed(participant.project(), None, 0, 100, false)
            .unwrap();
        assert_eq!(events.iter().filter(|(_, event)| matches!(event, FeedEvent::Receipt { state, .. } if state == "unknown")).count(), 2);
        assert!(!events.iter().any(|(_, event)| matches!(
            event,
            FeedEvent::Exposure { .. } | FeedEvent::Fetched { .. }
        )));
    }

    #[test]
    fn private_socket_request_sends_credentials_and_honors_one_deadline() {
        use crate::chat::protocol::{self, Operation};
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        use std::time::{Duration, Instant};
        for delayed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("participant.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let auth = protocol::read_frame(&mut stream).unwrap();
                assert_eq!(
                    auth,
                    serde_json::json!({"version":1,"binding":"private-test-credential"})
                );
                let request = protocol::read_frame(&mut stream).unwrap();
                assert_eq!(
                    request,
                    serde_json::json!({"version":1,"operation":{"op":"register"}})
                );
                if delayed {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = protocol::write_frame(
                    &mut stream,
                    &serde_json::json!({"version":1,"ok":true,"data":{"registered":true}}),
                );
            });
            let result = request(
                &socket,
                "private-test-credential",
                Operation::Register {},
                Instant::now()
                    + if delayed {
                        Duration::from_millis(20)
                    } else {
                        Duration::from_secs(1)
                    },
            );
            if delayed {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), serde_json::json!({"registered":true}));
            }
            worker.join().unwrap();
        }
    }

    #[test]
    fn issued_private_binding_enrolls_then_sends_through_the_real_store_owner() {
        use crate::chat::{
            protocol::Operation,
            types::{Draft, Target},
        };
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let socket = dir.path().join("participant.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let validator =
            ObservedBindings(NativeBindings::open(&dir.path().join("bindings")).unwrap());
        let owner = crate::chat::daemon::tests::owned_connections(
            listener,
            std::sync::Arc::new(validator),
            dir.path().join("chat.sqlite3"),
            17,
        );
        let registered = enroll(
            &files,
            &socket,
            binding(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        let session = registered.session.clone().unwrap();
        let key = registered.locator().unwrap();
        let mut other = binding();
        other.native_id = "33333333-3333-4333-8333-333333333333".into();
        other.name = "owned-recipient".into();
        let other = enroll(
            &files,
            &socket,
            other,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        let other_session = other.session.clone().unwrap();
        assert_ne!(session, other_session);
        let recipient = Participant {
            binding: other,
            socket: socket.clone(),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let participant = Participant {
            binding: registered,
            socket,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let sent = participant
            .request(Operation::Send {
                draft: Draft {
                    key: "owned-real-store".into(),
                    project: participant.project().into(),
                    to: vec![Target::Agent(other_session)],
                    body: "authenticated owned message".into(),
                    reply_to: None,
                },
            })
            .unwrap();
        assert!(sent["id"].is_string());
        let shown = recipient
            .request(Operation::Show {
                id: sent["id"].as_str().unwrap().into(),
            })
            .unwrap();
        assert_eq!(
            shown["sender"]["Agent"],
            serde_json::to_value(&session).unwrap()
        );
        assert_eq!(shown["draft"]["body"], "authenticated owned message");
        let reply = recipient
            .request(Operation::Reply {
                id: sent["id"].as_str().unwrap().into(),
                key: "owned-explicit-reply".into(),
                body: "explicit reply".into(),
                all: false,
            })
            .unwrap();
        assert!(reply["id"].is_string());
        let inbox = participant
            .request(Operation::Inbox {
                cursor: None,
                limit: 10,
            })
            .unwrap();
        assert!(serde_json::to_string(&inbox)
            .unwrap()
            .contains("explicit reply"));
        let pending = recipient
            .request(Operation::Reply {
                id: sent["id"].as_str().unwrap().into(),
                key: "owned-pinned-reply".into(),
                body: "pending old incarnation".into(),
                all: false,
            })
            .unwrap();
        assert!(
            participant.request(Operation::End {}).is_err(),
            "participant cannot end a native registration"
        );
        let integration = format!("{}.{}", key, participant.binding.integration_secret);
        request(
            &participant.socket,
            &integration,
            Operation::End {},
            participant.deadline,
        )
        .unwrap();
        assert!(
            request(
                &participant.socket,
                &integration,
                Operation::Register {},
                participant.deadline
            )
            .is_err(),
            "ended incarnation cannot revive"
        );
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));
        let startups: Vec<_> = (0..2)
            .map(|_| {
                let start = start.clone();
                let directory = files.directory.clone();
                let socket = participant.socket.clone();
                std::thread::spawn(move || {
                    let files = BindingFiles::open(&directory).unwrap();
                    start.wait();
                    enroll(
                        &files,
                        &socket,
                        binding(),
                        Instant::now() + Duration::from_secs(1),
                    )
                })
            })
            .collect();
        start.wait();
        let mut results = startups
            .into_iter()
            .map(|worker| worker.join().unwrap().unwrap());
        let renewed = results.next().unwrap();
        let concurrent = results.next().unwrap();
        assert_eq!(concurrent.session, renewed.session);
        assert_eq!(concurrent.participant_secret, renewed.participant_secret);
        assert_ne!(renewed.session, Some(session.clone()));
        assert_ne!(
            renewed.enrollment_secret,
            participant.binding.enrollment_secret
        );
        assert_ne!(
            renewed.participant_secret,
            participant.binding.participant_secret
        );
        assert_ne!(
            renewed.integration_secret,
            participant.binding.integration_secret
        );
        let reconnect =
            enroll(&files, &participant.socket, binding(), participant.deadline).unwrap();
        assert_eq!(reconnect.session, renewed.session);
        assert_eq!(reconnect.participant_secret, renewed.participant_secret);
        assert!(participant.request(Operation::Status {}).is_err());
        let renewed = Participant {
            binding: renewed,
            socket: participant.socket.clone(),
            deadline: participant.deadline,
        };
        assert!(!serde_json::to_string(
            &renewed
                .request(Operation::Inbox {
                    cursor: None,
                    limit: 10,
                })
                .unwrap()
        )
        .unwrap()
        .contains("pending old incarnation"));
        assert!(renewed
            .request(Operation::Show {
                id: pending["id"].as_str().unwrap().into(),
            })
            .is_err());
        owner.join().unwrap();
        let restored = files.load(&key).unwrap();
        assert_eq!(restored.session, renewed.binding.session);
        assert!(restored.context(&restored.enrollment_secret).is_err());
        assert!(restored
            .context(&participant.binding.participant_secret)
            .is_err());
        assert!(restored
            .context(&participant.binding.integration_secret)
            .is_err());
    }

    #[test]
    fn validator_requires_private_secret_matching_runtime_and_live_peer() {
        use crate::chat::daemon::{PeerIdentity, VerifiedContext};
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let binding = binding();
        let key = binding.locator().unwrap();
        let credential = format!("{key}.{}", binding.enrollment_secret);
        files
            .create(&binding, Instant::now() + Duration::from_secs(1))
            .unwrap();
        let validator = NativeBindings { files };
        let peer = PeerIdentity {
            uid: unsafe { libc::geteuid() },
            pid: Some(std::process::id()),
        };
        let observed = || {
            Ok(NativeRuntime {
                client: "codex".into(),
                version: "0.153.4".into(),
                process: ProcessEvidence::read(std::process::id())?,
            })
        };
        assert!(matches!(
            validator
                .validate_with(peer, &credential, observed)
                .unwrap(),
            VerifiedContext::Enrollment { .. }
        ));
        assert!(validator.validate_with(peer, &key, observed).is_err());
        assert!(validator
            .validate_with(peer, &format!("{key}.{}", "0".repeat(64)), observed)
            .is_err());
        assert!(validator
            .validate_with(peer, &credential, || {
                let mut runtime = observed()?;
                runtime.client = "claude".into();
                Ok(runtime)
            })
            .is_err());
        assert!(validator
            .validate_with(peer, &credential, || {
                let mut runtime = observed()?;
                runtime.version = "0.154.0".into();
                Ok(runtime)
            })
            .is_err());
        assert!(validator
            .validate_with(
                PeerIdentity {
                    uid: peer.uid,
                    pid: None
                },
                &credential,
                observed
            )
            .is_err());
        assert!(validator
            .validate_with(
                PeerIdentity {
                    uid: peer.uid + 1,
                    pid: peer.pid
                },
                &credential,
                observed
            )
            .is_err());
    }

    #[test]
    fn runtime_classifier_stops_at_foreign_native_client() {
        use std::path::Path;
        assert_eq!(
            native_client(Path::new("/releases/0.153.4/bin/codex"), &[]),
            Some("codex")
        );
        assert_eq!(
            native_client(
                Path::new("/home/user/.local/share/claude/versions/2.1.270"),
                &[]
            ),
            Some("claude")
        );
        assert_eq!(
            native_client(
                Path::new("/usr/bin/node"),
                &[
                    b"node".to_vec(),
                    b"/opt/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"
                        .to_vec()
                ]
            ),
            Some("pi")
        );
        assert_eq!(native_client(Path::new("/usr/bin/sh"), &[]), None);
        assert!(pi_process_title(
            Path::new("/usr/bin/node"),
            &[b"pi".to_vec()]
        ));
        assert!(!pi_process_title(
            Path::new("/usr/bin/node"),
            &[b"node".to_vec()]
        ));
        assert!(!pi_process_title(
            Path::new("/usr/bin/python"),
            &[b"pi".to_vec()]
        ));
        assert!(user_manager_command(&[
            b"/usr/lib/systemd/systemd".to_vec(),
            b"--user".to_vec(),
            b"--deserialize=14".to_vec(),
        ]));
        assert!(!user_manager_command(&[
            b"/usr/bin/codex".to_vec(),
            b"--user".to_vec(),
        ]));
    }

    #[test]
    fn native_version_invocation_and_locator_are_client_specific() {
        let pi_args = vec![
            b"node".to_vec(),
            b"/opt/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js".to_vec(),
        ];
        assert_eq!(version_args("codex", &[]).unwrap(), vec!["--version"]);
        assert_eq!(version_args("claude", &[]).unwrap(), vec!["--version"]);
        assert_eq!(
            version_args("pi", &pi_args).unwrap(),
            vec![
                "/opt/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
                "--version"
            ]
        );
        assert!(version_args("pi", &[b"node".to_vec()]).is_err());
        assert_eq!(
            parse_version("codex", "codex-cli 0.153.4\n").unwrap(),
            "0.153.4"
        );
        assert_eq!(
            parse_version_for_platform("codex", "codex-cli 0.155.1\n", NativePlatform::MacOs,)
                .unwrap(),
            "0.155.1"
        );
        assert!(
            parse_version_for_platform("codex", "codex-cli 0.155.1\n", NativePlatform::Linux,)
                .is_err()
        );
        assert_eq!(
            parse_version("claude", "2.1.273 (Claude Code)\n").unwrap(),
            "2.1.273"
        );
        assert_eq!(
            parse_version("claude", "2.1.278 (Claude Code)\n").unwrap(),
            "2.1.278"
        );
        assert_eq!(parse_version("pi", "0.86.1\n").unwrap(), "0.86.1");
        assert!(parse_version("pi", "v0.86.1\n").is_err());
        assert!(parse_version("claude", "2.1.999 (Claude Code)\n").is_err());

        let value = |key: &str| match key {
            "CODEX_THREAD_ID" => Some("11111111-1111-4111-8111-111111111111".into()),
            "LAM_CHAT_CLAUDE_SESSION_ID" => Some("22222222-2222-4222-8222-222222222222".into()),
            "PI_SESSION_ID" => Some("33333333-3333-4333-8333-333333333333".into()),
            _ => None,
        };
        assert_eq!(
            native_id_from("codex", value).unwrap(),
            value("CODEX_THREAD_ID").unwrap()
        );
        assert_eq!(
            native_id_from("claude", value).unwrap(),
            value("LAM_CHAT_CLAUDE_SESSION_ID").unwrap()
        );
        assert_eq!(
            native_id_from("pi", value).unwrap(),
            value("PI_SESSION_ID").unwrap()
        );
        assert!(native_id_from("unknown", value).is_err());
        assert!(native_id_from("pi", |_| Some("not-a-uuid".into())).is_err());
    }

    #[test]
    fn native_helper_deadline_and_output_bound_are_enforced() {
        use std::time::{Duration, Instant};
        let output = bounded_command(
            std::path::Path::new("sh"),
            &["-c", "printf bounded"],
            Instant::now() + Duration::from_secs(1),
            16,
        )
        .unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, "bounded");
        assert!(bounded_command(
            std::path::Path::new("sh"),
            &["-c", "printf toolong"],
            Instant::now() + Duration::from_secs(1),
            3
        )
        .is_err());
        let start = Instant::now();
        assert!(bounded_command(
            std::path::Path::new("sleep"),
            &["5"],
            start + Duration::from_millis(20),
            16
        )
        .is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    fn binding() -> NativeBinding {
        NativeBinding::new(
            "codex",
            "0.153.4",
            "11111111-1111-4111-8111-111111111111",
            ProcessEvidence::read(std::process::id()).unwrap(),
            "22222222-2222-4222-8222-222222222222",
            "owned-native",
        )
        .unwrap()
    }

    #[test]
    fn claude_target_requires_the_exact_live_process_listener() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("claude.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let directory = dir.path().join("bindings");
        let files = BindingFiles::open(&directory).unwrap();
        let mut binding = NativeBinding::new(
            "claude",
            "2.1.278",
            "11111111-1111-4111-8111-111111111111",
            ProcessEvidence::read(std::process::id()).unwrap(),
            "22222222-2222-4222-8222-222222222222",
            "owned-claude",
        )
        .unwrap();
        binding.native_socket = Some(socket.clone());
        binding
            .bind(crate::chat::types::SessionRef {
                machine: "33333333-3333-4333-8333-333333333333".into(),
                incarnation: "44444444-4444-4444-8444-444444444444".into(),
            })
            .unwrap();
        let registration = match binding.context(&binding.participant_secret).unwrap() {
            crate::chat::daemon::VerifiedContext::Participant(registration) => registration,
            _ => panic!("expected participant"),
        };
        files
            .create(&binding, Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(claude_target(&directory, &registration).unwrap().1, socket);
        drop(listener);
        assert!(claude_target(&directory, &registration).is_err());
    }

    #[test]
    fn stop_reserves_an_adjacent_claim_epoch_before_other_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let mut record = binding();
        record
            .bind(crate::chat::types::SessionRef {
                machine: "11111111-1111-4111-8111-111111111111".into(),
                incarnation: "33333333-3333-4333-8333-333333333333".into(),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        files.create(&record, deadline).unwrap();
        assert_eq!(files.reserve_epochs(&record, 2, deadline).unwrap(), 1);
        assert_eq!(files.next_epoch(&record, deadline).unwrap(), 3);
    }

    #[test]
    fn backend_socket_selection_uses_the_pinned_process_listener_inode() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = BACKEND_SOCKET_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("app-server-control.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let process = ProcessEvidence::read(std::process::id()).unwrap();
        assert_eq!(backend_socket_path(&process).unwrap(), path);
        drop(listener);
    }

    #[test]
    fn queue_target_requires_the_exact_private_binding_and_process() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = BACKEND_SOCKET_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("app-server-control.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let directory = dir.path().join("bindings");
        let files = BindingFiles::open(&directory).unwrap();
        let mut record = binding();
        record
            .bind(crate::chat::types::SessionRef {
                machine: "11111111-1111-4111-8111-111111111111".into(),
                incarnation: "33333333-3333-4333-8333-333333333333".into(),
            })
            .unwrap();
        files
            .create(
                &record,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();
        let crate::chat::daemon::VerifiedContext::Integration(registration) =
            record.context(&record.integration_secret).unwrap()
        else {
            panic!("not integration");
        };
        let (process, selected) = codex_queue_target(&directory, &registration).unwrap();
        assert_eq!(process, record.process);
        assert_eq!(selected, socket);
        let mut wrong = registration.clone();
        wrong.native_id = "44444444-4444-4444-8444-444444444444".into();
        assert!(codex_queue_target(&directory, &wrong).is_err());
        wrong = registration;
        wrong.process_start.push_str(":replacement");
        assert!(codex_queue_target(&directory, &wrong).is_err());
    }

    #[test]
    fn initial_binding_publication_serializes_with_enrollment() {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        use std::sync::{mpsc, Arc, Barrier};
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("bindings");
        let files = BindingFiles::open(&directory).unwrap();
        let key = binding().locator().unwrap();
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(format!("{key}.lock")))
            .unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        let timed_out = files.create(&binding(), Instant::now() + Duration::from_millis(20));
        assert!(timed_out.is_err());
        let start = Arc::new(Barrier::new(3));
        let (sender, receiver) = mpsc::channel();
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let directory = directory.clone();
                let start = start.clone();
                let sender = sender.clone();
                std::thread::spawn(move || {
                    let files = BindingFiles::open(&directory).unwrap();
                    let candidate = binding();
                    start.wait();
                    sender
                        .send(files.create(&candidate, Instant::now() + Duration::from_secs(1)))
                        .unwrap();
                })
            })
            .collect();
        start.wait();
        let premature = receiver.recv_timeout(Duration::from_millis(100));
        let published_while_locked = directory.join(format!("{key}.json")).exists();
        drop(lock);
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(
            premature.is_err(),
            "initial publication ignored enrollment lock"
        );
        assert!(!published_while_locked);
        let results: Vec<_> = (0..2).map(|_| receiver.recv().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let error = results.into_iter().find_map(Result::err).unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        files.load(&key).unwrap().validate_shape().unwrap();
    }

    #[test]
    fn initial_binding_publication_survives_interrupted_writer() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{Command, Stdio};
        const DIRECTORY: &str = "LAM_CHAT_TEST_INTERRUPTED_BINDING_DIR";
        const PARENT: &str = "LAM_CHAT_TEST_INTERRUPTED_BINDING_PARENT";
        if let Some(directory) = std::env::var_os(DIRECTORY) {
            let files = BindingFiles::open(std::path::Path::new(&directory)).unwrap();
            let mut candidate = binding();
            candidate.process =
                ProcessEvidence::read(std::env::var(PARENT).unwrap().parse().unwrap()).unwrap();
            // Only this owned test subprocess is limited/interrupted. No
            // production failpoint or native client participates in this test.
            unsafe {
                assert_eq!(
                    libc::setrlimit(
                        libc::RLIMIT_CORE,
                        &libc::rlimit {
                            rlim_cur: 0,
                            rlim_max: 0,
                        }
                    ),
                    0
                );
                assert_eq!(
                    libc::setrlimit(
                        libc::RLIMIT_FSIZE,
                        &libc::rlimit {
                            rlim_cur: 128,
                            rlim_max: 128,
                        }
                    ),
                    0
                );
                libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
            }
            let _ = files.create(&candidate, Instant::now() + Duration::from_secs(1));
            panic!("writer was not interrupted");
        }
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("bindings");
        let files = BindingFiles::open(&directory).unwrap();
        let candidate = binding();
        let key = candidate.locator().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "chat::adapters::binding::tests::initial_binding_publication_survives_interrupted_writer"])
            .env(DIRECTORY, &directory)
            .env(PARENT, std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.signal(), Some(libc::SIGXFSZ));
        assert!(
            !directory.join(format!("{key}.json")).exists(),
            "interrupted writer left a poisoned final binding"
        );
        files
            .create(&candidate, Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            files.load(&key).unwrap().participant_secret,
            candidate.participant_secret
        );
    }

    #[test]
    fn enrollment_persists_one_owner_assigned_incarnation_and_reuses_it() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let binding = binding();
        let key = binding.locator().unwrap();
        files
            .create(&binding, Instant::now() + Duration::from_secs(1))
            .unwrap();
        let session = crate::chat::types::SessionRef {
            machine: uuid::Uuid::new_v4().to_string(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        assert!(files
            .enroll(
                &binding,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("unbound record has no ended incarnation"),
                |_| anyhow::bail!("owner unavailable")
            )
            .is_err());
        assert!(files.load(&key).unwrap().session.is_none());
        let enrolled = files
            .enroll(
                &binding,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("unbound record"),
                |_| Ok((session.clone(), false)),
            )
            .unwrap();
        assert_eq!(enrolled.session, Some(session.clone()));
        assert!(!enrolled.eligible);
        let reopened = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let restored = reopened
            .enroll(
                &binding,
                Instant::now() + Duration::from_secs(1),
                |_| Ok(false),
                |_| panic!("must not reenroll a bound incarnation"),
            )
            .unwrap();
        assert_eq!(restored.session, Some(session));
        assert_eq!(restored.participant_secret, binding.participant_secret);
        assert!(restored.context(&binding.enrollment_secret).is_err());
    }

    #[test]
    fn trusted_startup_rotation_fails_closed_and_resumes_pending_enrollment() {
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("bindings");
        let files = BindingFiles::open(&directory).unwrap();
        let mut original = binding();
        let old_session = crate::chat::types::SessionRef {
            machine: uuid::Uuid::new_v4().to_string(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        original.bind(old_session.clone()).unwrap();
        files
            .create(&original, Instant::now() + Duration::from_secs(1))
            .unwrap();
        let key = original.locator().unwrap();
        let candidate = binding();
        assert!(files
            .enroll(
                &candidate,
                Instant::now() + Duration::from_secs(1),
                |_| anyhow::bail!("owner unavailable"),
                |_| panic!("unknown End cannot register")
            )
            .is_err());
        assert_eq!(
            files.load(&key).unwrap().participant_secret,
            original.participant_secret
        );

        // A real filesystem publication failure must preserve the old complete
        // record and never attempt registration with unpublished credentials.
        let displaced = dir.path().join("displaced-bindings");
        let failed = files.enroll(
            &candidate,
            Instant::now() + Duration::from_secs(1),
            |_| {
                std::fs::rename(&directory, &displaced)?;
                std::fs::File::create(&directory)?;
                Ok(true)
            },
            |_| panic!("failed rotation cannot register"),
        );
        std::fs::remove_file(&directory).unwrap();
        std::fs::rename(&displaced, &directory).unwrap();
        assert!(failed.is_err());
        assert_eq!(files.load(&key).unwrap().session, Some(old_session));
        assert_eq!(
            files.load(&key).unwrap().participant_secret,
            original.participant_secret
        );

        assert!(files
            .enroll(
                &candidate,
                Instant::now() + Duration::from_secs(1),
                |_| Ok(true),
                |fresh| {
                    assert!(fresh.session.is_none());
                    assert_ne!(fresh.enrollment_secret, original.enrollment_secret);
                    assert_eq!(
                        files.load(&key).unwrap().enrollment_secret,
                        fresh.enrollment_secret
                    );
                    anyhow::bail!("registration response lost")
                }
            )
            .is_err());
        let pending = files.load(&key).unwrap();
        assert!(pending.session.is_none());
        assert!(pending.context(&original.integration_secret).is_err());
        let next = crate::chat::types::SessionRef {
            machine: original.session.as_ref().unwrap().machine.clone(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        let resumed = files
            .enroll(
                &candidate,
                Instant::now() + Duration::from_secs(1),
                |_| panic!("pending enrollment has no ended incarnation"),
                |_| Ok((next.clone(), true)),
            )
            .unwrap();
        assert_eq!(resumed.session, Some(next));
        assert_eq!(resumed.participant_secret, pending.participant_secret);
        assert_eq!(resumed.integration_secret, pending.integration_secret);
    }

    #[test]
    fn binding_credentials_separate_enrollment_participant_and_integration() {
        use crate::chat::{daemon::VerifiedContext, types::SessionRef};
        let mut binding = binding();
        assert!(matches!(
            binding.context(&binding.enrollment_secret).unwrap(),
            VerifiedContext::Enrollment { .. }
        ));
        assert!(binding.context(&binding.participant_secret).is_err());
        assert!(binding.context(&binding.integration_secret).is_err());
        assert!(binding.context(&binding.native_id).is_err());
        let session = SessionRef {
            machine: uuid::Uuid::new_v4().to_string(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        binding.bind(session.clone()).unwrap();
        assert!(binding.context(&binding.enrollment_secret).is_err());
        match binding.context(&binding.participant_secret).unwrap() {
            VerifiedContext::Participant(registration) => assert_eq!(registration.session, session),
            _ => panic!("participant credential changed role"),
        }
        assert!(matches!(
            binding.context(&binding.integration_secret).unwrap(),
            VerifiedContext::Integration(_)
        ));
        assert!(binding
            .bind(SessionRef {
                machine: session.machine,
                incarnation: uuid::Uuid::new_v4().to_string()
            })
            .is_err());
    }

    #[test]
    fn private_binding_records_survive_reopen_and_refuse_unsafe_paths() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let binding_dir = dir.path().join("bindings");
        let files = BindingFiles::open(&binding_dir).unwrap();
        let binding = binding();
        let key = binding.locator().unwrap();
        files
            .create(&binding, Instant::now() + Duration::from_secs(1))
            .unwrap();
        assert!(files
            .create(&binding, Instant::now() + Duration::from_secs(1))
            .is_err());
        let reopened = BindingFiles::open(&binding_dir).unwrap();
        let restored = reopened.load(&key).unwrap();
        assert_eq!(restored.participant_secret, binding.participant_secret);
        let path = binding_dir.join(format!("{key}.json"));
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(reopened.load("../../unrelated").is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(reopened.load(&key).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let linked = "a".repeat(64);
        std::os::unix::fs::symlink(&path, binding_dir.join(format!("{linked}.json"))).unwrap();
        assert!(reopened.load(&linked).is_err());
        let mut wrong_lifetime = binding;
        wrong_lifetime.process.start_ticks += 1;
        assert_ne!(wrong_lifetime.locator().unwrap(), key);
        let linked_candidate =
            binding_dir.join(format!("{}.json", wrong_lifetime.locator().unwrap()));
        std::os::unix::fs::symlink(&path, &linked_candidate).unwrap();
        assert!(files
            .create(&wrong_lifetime, Instant::now() + Duration::from_secs(1))
            .is_err());
        assert!(linked_candidate.symlink_metadata().unwrap().is_symlink());
        assert_eq!(
            reopened.load(&key).unwrap().participant_secret,
            restored.participant_secret
        );
    }

    #[test]
    fn process_evidence_checks_boot_birth_executable_and_live_ancestry() {
        let current = ProcessEvidence::read(std::process::id()).unwrap();
        current.validate().unwrap();
        current.validate_descendant(std::process::id()).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let child_id = child.id();
        let result = current.validate_descendant(child_id);
        child.kill().unwrap();
        child.wait().unwrap();
        result.unwrap();
        assert!(current.validate_descendant(child_id).is_err());

        let mut stale = current.clone();
        stale.boot_id = uuid::Uuid::new_v4().to_string();
        assert!(stale.validate().is_err());
        stale = current.clone();
        stale.start_ticks += 1;
        assert!(stale.validate().is_err());
        stale = current.clone();
        stale.executable = "/not/the/live/executable".into();
        assert!(stale.validate().is_err());
        assert!(current.validate_descendant(1).is_err());
    }

    #[test]
    fn process_stat_handles_parentheses_in_comm_and_rejects_invalid_birth() {
        let mut fields = vec!["S", "42"];
        fields.extend(std::iter::repeat_n("0", 17));
        fields.push("12345");
        let stat = format!("123 (a ) nested ( name) {}\n", fields.join(" "));
        assert_eq!(parse_process_stat(&stat).unwrap(), (42, 12345));
        assert!(parse_process_stat("123 (name) Z 42").is_err());
        assert!(parse_process_stat("not a process stat").is_err());
    }
}
