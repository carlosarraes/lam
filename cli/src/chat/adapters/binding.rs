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
pub(super) struct NativeBindings {
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
        use sha2::Digest;
        let input = serde_json::to_vec(&(&self.client, &self.native_id, self.process_start()))?;
        Ok(format!("{:x}", sha2::Sha256::digest(input)))
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

    fn create(&self, binding: &NativeBinding) -> anyhow::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        binding.validate_shape()?;
        let encoded = serde_json::to_vec(binding)?;
        anyhow::ensure!(
            encoded.len() <= 16_384,
            "private binding record exceeds its bound"
        );
        let path = self.directory.join(format!("{}.json", binding.locator()?));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        Ok(())
    }

    /// Only the daemon's Store owner assigns the session. Concurrent native
    /// hooks serialize this handshake, and a bound file is never re-enrolled.
    fn enroll(
        &self,
        key: &str,
        deadline: std::time::Instant,
        register: impl FnOnce(&NativeBinding) -> anyhow::Result<(crate::chat::types::SessionRef, bool)>,
    ) -> anyhow::Result<NativeBinding> {
        use std::io::Write;
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
        let mut binding = self.load(key)?;
        if binding.session.is_some() {
            return Ok(binding);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "native enrollment deadline expired"
        );
        let (session, eligible) = register(&binding)?;
        binding.bind(session)?;
        binding.eligible = eligible;
        let encoded = serde_json::to_vec(&binding)?;
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
        result?;
        Ok(binding)
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

    #[test]
    fn validator_requires_private_secret_matching_runtime_and_live_peer() {
        use crate::chat::daemon::{PeerIdentity, VerifiedContext};
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let binding = binding();
        let key = binding.locator().unwrap();
        let credential = format!("{key}.{}", binding.enrollment_secret);
        files.create(&binding).unwrap();
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
    fn enrollment_persists_one_owner_assigned_incarnation_and_reuses_it() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let files = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let binding = binding();
        let key = binding.locator().unwrap();
        files.create(&binding).unwrap();
        let session = crate::chat::types::SessionRef {
            machine: uuid::Uuid::new_v4().to_string(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        assert!(files
            .enroll(
                &key,
                Instant::now() + Duration::from_secs(1),
                |_| anyhow::bail!("owner unavailable")
            )
            .is_err());
        assert!(files.load(&key).unwrap().session.is_none());
        let enrolled = files
            .enroll(&key, Instant::now() + Duration::from_secs(1), |_| {
                Ok((session.clone(), false))
            })
            .unwrap();
        assert_eq!(enrolled.session, Some(session.clone()));
        assert!(!enrolled.eligible);
        let reopened = BindingFiles::open(&dir.path().join("bindings")).unwrap();
        let restored = reopened
            .enroll(&key, Instant::now() + Duration::from_secs(1), |_| {
                panic!("must not reenroll a bound incarnation")
            })
            .unwrap();
        assert_eq!(restored.session, Some(session));
        assert_eq!(restored.participant_secret, binding.participant_secret);
        assert!(restored.context(&binding.enrollment_secret).is_err());
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
        files.create(&binding).unwrap();
        assert!(files.create(&binding).is_err());
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
