/// Authenticate only on the private participant endpoint. Connection and both
/// frames share the caller's absolute deadline, including a congested listener.
fn request(
    socket: &std::path::Path,
    credential: &str,
    operation: crate::chat::protocol::Operation,
    deadline: std::time::Instant,
) -> anyhow::Result<serde_json::Value> {
    use crate::chat::{daemon, protocol};
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    };
    use std::time::Instant;
    daemon::validate_socket(socket)?;
    operation.validate()?;
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
    let mut stream = daemon::DeadlineStream::until(&mut stream, deadline);
    protocol::write_frame(
        &mut stream,
        &serde_json::json!({"version":1,"binding":credential}),
    )?;
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
        "Chat participant request failed: {}",
        response["error"]
    );
    Ok(response["data"].clone())
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
            runtime.process,
            &project,
            &name,
        )?;
        enroll(&files, &paths.socket, candidate, deadline)?
    } else {
        let key = binding_locator("codex", input.native_id(), &runtime.process)?;
        let binding = files.load(&key)?;
        anyhow::ensure!(
            binding.session.is_some()
                && binding.version == runtime.version
                && binding.process == runtime.process,
            "native hook binding is missing or changed"
        );
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

impl Participant {
    pub fn current(
        paths: &crate::chat::config::Paths,
        deadline: std::time::Instant,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        (|| -> anyhow::Result<Self> {
            let native_id = std::env::var("CODEX_THREAD_ID").context("missing native thread locator")?;
            crate::chat::protocol::validate_uuid(&native_id)?;
            let runtime = NativeRuntime::discover(std::process::id(), deadline)?;
            let directory = paths.database.parent().context("missing Chat data directory")?.join("bindings");
            anyhow::ensure!(directory.exists(), "native integration has not issued a binding");
            let files = BindingFiles::open(&directory)?;
            let locator = binding_locator(&runtime.client, &native_id, &runtime.process)?;
            let binding = files.load(&locator)?;
            anyhow::ensure!(binding.session.is_some() && binding.version == runtime.version && binding.process == runtime.process, "native enrollment missing or changed");
            Ok(Self { binding, socket: paths.socket.clone(), deadline })
        })().context("Chat requires a validated native participant binding; configure the client integration")
    }

    pub fn project(&self) -> &str {
        &self.binding.project
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
}

fn binding_locator(
    client: &str,
    native_id: &str,
    process: &ProcessEvidence,
) -> anyhow::Result<String> {
    use sha2::Digest;
    let lifetime = format!(
        "{}:{}:{}",
        process.boot_id, process.pid, process.start_ticks
    );
    let input = serde_json::to_vec(&(client, native_id, lifetime))?;
    Ok(format!("{:x}", sha2::Sha256::digest(input)))
}

#[cfg(target_os = "linux")]
struct NativeRuntime {
    client: String,
    version: String,
    process: ProcessEvidence,
}

#[cfg(target_os = "linux")]
impl NativeRuntime {
    fn discover(mut pid: u32, deadline: std::time::Instant) -> anyhow::Result<Self> {
        use std::io::Read;
        for _ in 0..64 {
            anyhow::ensure!(pid > 1, "no supported native execution ancestor");
            let process = ProcessEvidence::read(pid)?;
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
            if let Some(client) = native_client(&process.executable, &args) {
                // A nearer foreign client must never inherit an outer Codex identity.
                anyhow::ensure!(
                    client == "codex",
                    "native client binding is not implemented for this client"
                );
                let executable = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
                let output = bounded_command(&executable, &["--version"], deadline, 256)?;
                let version = output
                    .stdout
                    .trim()
                    .strip_prefix("codex-cli ")
                    .ok_or_else(|| anyhow::anyhow!("unrecognized native execution version"))?;
                anyhow::ensure!(
                    output.success && matches!(version, "0.153.4" | "0.154.0"),
                    "unsupported native execution version"
                );
                process.validate()?;
                return Ok(Self {
                    client: client.into(),
                    version: version.into(),
                    process,
                });
            }
            let (parent, _) =
                parse_process_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat"))?)?;
            anyhow::ensure!(parent != pid, "invalid native process ancestry");
            pid = parent;
        }
        anyhow::bail!("native process ancestry exceeds validation bound")
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub(in crate::chat) struct NativeBindings {
    files: BindingFiles,
}

#[cfg(target_os = "linux")]
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
        let runtime = observe()?;
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

#[cfg(target_os = "linux")]
impl crate::chat::daemon::BindingValidator for NativeBindings {
    fn status(&self) -> &'static str {
        "experimental native validator; native gates incomplete"
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
#[cfg(target_os = "linux")]
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
    observation_epoch: u64,
    enrollment_secret: String,
    participant_secret: String,
    integration_secret: String,
}

#[cfg(target_os = "linux")]
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
                && self.process.start_ticks > 0
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
            self.process.boot_id, self.process.pid, self.process.start_ticks
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
struct BindingFiles {
    directory: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
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
            if unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    libc::AT_FDCWD,
                    target.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } != 0
            {
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
