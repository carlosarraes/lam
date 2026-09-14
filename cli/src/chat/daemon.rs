use anyhow::{bail, ensure, Context, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, SyncSender},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use super::config::{Config, Paths};
use super::protocol::{self, Operation};
use super::registry::{reply_targets, NativeEvidence, Registry};
use super::store::Store;
use super::types::{Actor, Draft, Limits, Message, Registration, SessionRef, Target};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 32;
const MAX_SUBSCRIPTIONS: usize = 16;
const WRITER_QUEUE: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct PeerIdentity {
    pub uid: u32,
    pub pid: Option<u32>,
}

/// Task 6 must resolve the private integration-issued binding and verify native
/// identity/process evidence against this peer. A token or claimed PID alone
/// cannot authorize a context. Participant bindings must never return Observer.
pub trait BindingValidator: Send + Sync {
    fn validate(&self, peer: PeerIdentity, binding: &str) -> Result<VerifiedContext>;
}

struct UnavailableBindings;
impl BindingValidator for UnavailableBindings {
    fn validate(&self, _peer: PeerIdentity, _binding: &str) -> Result<VerifiedContext> {
        bail!("Chat native binding validation is unavailable; participant access is disabled until its integration is configured")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authentication {
    version: u32,
    binding: String,
}

struct Work {
    context: VerifiedContext,
    operation: Operation,
    response: SyncSender<Result<Value>>,
    deadline: Instant,
}

#[derive(Default)]
struct Changes {
    generation: Mutex<u64>,
    changed: Condvar,
}
impl Changes {
    fn notify(&self) {
        let mut generation = self.generation.lock().unwrap();
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
    }
}

pub fn run(paths: &Paths) -> Result<()> {
    run_with_validator(paths, Arc::new(UnavailableBindings))
}

pub fn run_with_validator(paths: &Paths, validator: Arc<dyn BindingValidator>) -> Result<()> {
    paths.validate()?;
    let _lock = lock_instance(&paths.lock)?;
    let config = Config::load_or_create(&paths.config)?;
    let mut service = Service::open(&paths.database, &config.machine)?;
    service.limits = config.limits();
    // Register termination handling only for this command. Enabling ctrlc's
    // global termination feature would also change the existing pairing flow.
    let mut signals =
        signal_hook::iterator::Signals::new([libc::SIGINT, libc::SIGTERM, libc::SIGHUP])?;
    let (participant, _participant_path) = bind(&paths.socket)?;
    let (observer, _observer_path) = bind(&paths.observer_socket)?;
    let stop = Arc::new(AtomicBool::new(false));
    let changes = Arc::new(Changes::default());
    let (sender, receiver) = mpsc::sync_channel::<Work>(WRITER_QUEUE);
    let writer_stop = stop.clone();
    let writer_changes = changes.clone();
    let writer = thread::spawn(move || {
        let mut service = service;
        while !writer_stop.load(Ordering::SeqCst) {
            let work = match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(work) => work,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            apply_work(&mut service, work, &writer_changes);
        }
    });
    let subscriptions = Arc::new(Mutex::new(0_usize));
    let mut clients: Vec<thread::JoinHandle<()>> = Vec::new();
    let serving = (|| -> Result<()> {
        while !stop.load(Ordering::SeqCst) {
            if signals.pending().next().is_some() {
                break;
            }
            let mut index = 0;
            while index < clients.len() {
                if clients[index].is_finished() {
                    let _ = clients.swap_remove(index).join();
                } else {
                    index += 1;
                }
            }
            for (listener, is_observer) in [(&participant, false), (&observer, true)] {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if clients.len() >= MAX_CONNECTIONS {
                            drop(stream);
                            continue;
                        }
                        let sender = sender.clone();
                        let validator = validator.clone();
                        let stop = stop.clone();
                        let changes = changes.clone();
                        let subscriptions = subscriptions.clone();
                        clients.push(thread::spawn(move || {
                            let mut stream = stream;
                            if let Err(error) = serve_connection(
                                &mut stream,
                                is_observer,
                                validator.as_ref(),
                                sender,
                                &stop,
                                &changes,
                                &subscriptions,
                            ) {
                                let _ = protocol::write_frame(
                                    &mut DeadlineStream::new(&mut stream, IO_TIMEOUT),
                                    &json!({"version": 1, "ok": false, "error": error.to_string()}),
                                );
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    })();
    stop.store(true, Ordering::SeqCst);
    changes.notify();
    drop(sender);
    for client in clients {
        let _ = client.join();
    }
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Chat store owner panicked"))?;
    serving
}

fn apply_work(service: &mut Service, work: Work, changes: &Changes) {
    if work.deadline < Instant::now() {
        return;
    }
    let mutation = matches!(
        work.operation,
        Operation::Send { .. }
            | Operation::Reply { .. }
            | Operation::Register {}
            | Operation::SetState { .. }
            | Operation::Delivery { .. }
            | Operation::Finish { .. }
            | Operation::Retry { .. }
            | Operation::FetchComplete { .. }
    );
    let result = service.handle(&work.context, work.operation);
    if mutation && result.is_ok() {
        changes.notify();
    }
    let _ = work.response.try_send(result);
}

fn serve_connection(
    stream: &mut UnixStream,
    observer: bool,
    validator: &dyn BindingValidator,
    sender: SyncSender<Work>,
    stop: &AtomicBool,
    changes: &Changes,
    subscriptions: &Mutex<usize>,
) -> Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let peer = peer_identity(stream)?;
    let context = if observer {
        VerifiedContext::Observer
    } else {
        let auth: Authentication = serde_json::from_value(protocol::read_frame(
            &mut DeadlineStream::new(stream, IO_TIMEOUT),
        )?)?;
        ensure!(
            auth.version == 1 && !auth.binding.is_empty() && auth.binding.len() <= 4096,
            "invalid Chat authentication frame"
        );
        let context = validator.validate(peer, &auth.binding)?;
        ensure!(
            !matches!(context, VerifiedContext::Observer),
            "participant endpoint cannot grant observer authority"
        );
        context
    };
    // One operation per connection. Subscribe then owns that connection until
    // disconnect; reconnect with its last received cursor to resume.
    let request = protocol::decode_request(protocol::read_frame(&mut DeadlineStream::new(
        stream, IO_TIMEOUT,
    ))?)?;
    if let Operation::Subscribe {
        project,
        mut cursor,
        limit,
    } = request.operation
    {
        let _slot = SubscriptionSlot::acquire(subscriptions)?;
        let mut first = true;
        while !stop.load(Ordering::SeqCst) {
            let observed = *changes.generation.lock().unwrap();
            let page = dispatch(
                &sender,
                &context,
                Operation::Subscribe {
                    project: project.clone(),
                    cursor,
                    limit,
                },
            )?;
            cursor = Some(
                page["cursor"]
                    .as_str()
                    .context("missing feed cursor")?
                    .into(),
            );
            let has_messages = !page["events"]
                .as_array()
                .context("missing feed page")?
                .is_empty();
            if first || has_messages {
                respond(stream, page)?;
                first = false;
            }
            if has_messages {
                continue;
            }
            let mut generation = changes.generation.lock().unwrap();
            while *generation == observed && !stop.load(Ordering::SeqCst) {
                // Timed wake checks disconnected peers; no empty frames or
                // database polling. Commits wake subscribers immediately.
                generation = changes
                    .changed
                    .wait_timeout(generation, IO_TIMEOUT)
                    .unwrap()
                    .0;
                if peer_disconnected(stream)? {
                    return Ok(());
                }
            }
        }
        Ok(())
    } else {
        let fetch = matches!(
            request.operation,
            Operation::Inbox { .. } | Operation::Show { .. }
        );
        let page = dispatch(&sender, &context, request.operation)?;
        let mut ids = Vec::new();
        if fetch {
            if let VerifiedContext::Participant(registration) = &context {
                let values: Vec<Value> = page
                    .get("events")
                    .and_then(Value::as_array)
                    .map(|events| {
                        events
                            .iter()
                            .filter_map(|e| e["event"].get("message").cloned())
                            .collect()
                    })
                    .unwrap_or_else(|| vec![page.clone()]);
                for value in values {
                    let message: Message = serde_json::from_value(value)?;
                    if message
                        .draft
                        .to
                        .contains(&Target::Agent(registration.session.clone()))
                    {
                        ids.push(message.id);
                    }
                }
            }
        }
        respond(stream, page)?;
        // Every returned message contains its complete body. Only a completed
        // frame records transfer; failed/partial writes leave fetch state alone.
        if !ids.is_empty() {
            let _ = dispatch(&sender, &context, Operation::FetchComplete { ids });
        }
        Ok(())
    }
}

struct SubscriptionSlot<'a>(&'a Mutex<usize>);
impl<'a> SubscriptionSlot<'a> {
    fn acquire(count: &'a Mutex<usize>) -> Result<Self> {
        let mut current = count.lock().unwrap();
        ensure!(
            *current < MAX_SUBSCRIPTIONS,
            "Chat subscription limit reached; reconnect later"
        );
        *current += 1;
        Ok(Self(count))
    }
}
impl Drop for SubscriptionSlot<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap() -= 1;
    }
}

fn dispatch(
    sender: &SyncSender<Work>,
    context: &VerifiedContext,
    operation: Operation,
) -> Result<Value> {
    let (response, receiver) = mpsc::sync_channel(1);
    sender
        .try_send(Work {
            context: context.clone(),
            operation,
            response,
            deadline: Instant::now() + Duration::from_secs(6),
        })
        .map_err(|_| {
            anyhow::anyhow!(
                "Chat writer queue is full or stopped; retry with the same idempotency key"
            )
        })?;
    receiver
        .recv_timeout(Duration::from_secs(6))
        .context("Chat store request timed out; retry with the same idempotency key")?
}
fn respond(stream: &mut UnixStream, data: Value) -> Result<()> {
    protocol::write_frame(
        &mut DeadlineStream::new(stream, IO_TIMEOUT),
        &json!({"version": 1, "ok": true, "data": data}),
    )
}

/// Socket timeouts alone restart on each syscall. Use an absolute deadline so a
/// trickling peer cannot keep a connection slot indefinitely.
struct DeadlineStream<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}
impl<'a> DeadlineStream<'a> {
    fn new(stream: &'a mut UnixStream, timeout: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + timeout,
        }
    }
    fn remaining(&self) -> std::io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "Chat frame deadline expired")
            })
    }
}
impl std::io::Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        std::io::Read::read(self.stream, bytes)
    }
}
impl std::io::Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        std::io::Write::write(self.stream, bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn peer_identity(stream: &UnixStream) -> Result<PeerIdentity> {
    #[cfg(target_os = "linux")]
    let peer = {
        let mut credential: libc::ucred = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credential as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        ensure!(result == 0, "cannot validate Chat peer credentials");
        PeerIdentity {
            uid: credential.uid,
            pid: u32::try_from(credential.pid).ok(),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let peer = {
        let mut uid = 0;
        let mut gid = 0;
        ensure!(
            unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0,
            "cannot validate Chat peer credentials"
        );
        PeerIdentity { uid, pid: None }
    };
    validate_peer_owner(peer.uid, unsafe { libc::geteuid() })?;
    Ok(peer)
}

fn peer_disconnected(stream: &UnixStream) -> Result<bool> {
    let mut byte = 0_u8;
    let result = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            (&mut byte as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if result >= 0 {
        return Ok(true);
    } // EOF or unexpected input after Subscribe.
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error.into())
    }
}

fn lock_instance(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o777 == 0o600
            && metadata.nlink() == 1,
        "Chat lock must be a private, owned regular file"
    );
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Chat daemon already running for this state directory"
    );
    Ok(file)
}

pub fn validate_socket(path: &Path) -> Result<()> {
    let metadata = path.symlink_metadata()?;
    ensure!(
        metadata.file_type().is_socket()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o777 == 0o600,
        "Chat socket must be private and owned by the current user"
    );
    Ok(())
}

struct SocketPath {
    path: std::path::PathBuf,
    inode: u64,
}
impl Drop for SocketPath {
    fn drop(&mut self) {
        if self
            .path
            .symlink_metadata()
            .is_ok_and(|m| m.ino() == self.inode && m.file_type().is_socket())
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
fn bind(path: &Path) -> Result<(UnixListener, SocketPath)> {
    if path.symlink_metadata().is_ok() {
        validate_socket(path)?;
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    let guard = SocketPath {
        path: path.into(),
        inode: path.symlink_metadata()?.ino(),
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    validate_socket(path)?;
    listener.set_nonblocking(true)?;
    Ok((listener, guard))
}

/// Constructed only by endpoint selection or a native binding validator. Never
/// deserialized. Same-UID processes are trusted by the observer endpoint; this
/// does not prove human intent against another process owned by that user.
#[derive(Clone, Debug)]
pub enum VerifiedContext {
    Observer,
    Participant(Registration),
    Integration(Registration),
    Enrollment {
        project: String,
        name: String,
        evidence: NativeEvidence,
        eligible: bool,
    },
}

struct Service {
    store: Store,
    machine: String,
    cursor_key: String,
    limits: Limits,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    role: String,
    session: Option<SessionRef>,
    project: String,
    query: String,
    after: u64,
}

impl Service {
    fn open(path: &Path, machine: &str) -> Result<Self> {
        let mut store = Store::open(path)?;
        store.set_machine(machine)?;
        let cursor_key = store.cursor_key()?;
        store.recover_submitting()?;
        Ok(Self {
            store,
            machine: machine.into(),
            cursor_key,
            limits: super::config::DEFAULT_LIMITS,
        })
    }

    fn handle(&mut self, context: &VerifiedContext, operation: Operation) -> Result<Value> {
        operation.validate()?;
        if let VerifiedContext::Enrollment {
            project,
            name,
            evidence,
            eligible,
        } = context
        {
            ensure!(
                matches!(operation, Operation::Register {}),
                "enrollment authority only permits Register"
            );
            protocol::validate_project(project)?;
            ensure!(name.len() <= 256, "Chat session name exceeds 256 bytes");
            let registration = Registry::new(&mut self.store)?.connect(
                project,
                crate::name::Sources {
                    explicit: Some(name.clone()),
                    lam_name: None,
                    multiplexer: None,
                },
                evidence.clone(),
                *eligible,
            )?;
            return Ok(
                json!({"session": registration.session, "project": registration.project, "name": registration.name, "client": registration.client, "eligible": registration.eligible}),
            );
        }
        if let VerifiedContext::Integration(registration) = context {
            return match operation {
                Operation::Register {} => {
                    protocol::validate_session(&registration.session)?;
                    protocol::validate_project(&registration.project)?;
                    Registry::new(&mut self.store)?.register(registration.clone())?;
                    Ok(json!({"session": registration.session}))
                }
                Operation::SetState { eligible } => {
                    self.verify_registration(registration)?;
                    Registry::new(&mut self.store)?
                        .set_eligible(&registration.session, eligible)?;
                    Ok(json!({"eligible": eligible}))
                }
                Operation::Delivery { event, epoch } => {
                    self.verify_registration(registration)?;
                    let eligible = Registry::new(&mut self.store)?
                        .state(&registration.session)
                        .is_some_and(|s| s.registration.eligible && !s.ended);
                    ensure!(eligible, "Chat recipient is ineligible");
                    Ok(serde_json::to_value(super::delivery::request(
                        &mut self.store,
                        &registration.session,
                        event,
                        epoch,
                        self.limits,
                    )?)?)
                }
                Operation::Finish { attempt, outcome } => {
                    self.verify_registration(registration)?;
                    ensure!(
                        self.store.attempt_recipient(&attempt)? == registration.session,
                        "Chat attempt belongs to another recipient"
                    );
                    self.store.finish(&attempt, outcome)?;
                    Ok(json!({"finished": attempt}))
                }
                _ => bail!("operation requires participant or observer authority"),
            };
        }
        if let VerifiedContext::Participant(registration) = context {
            self.verify_registration(registration)?;
        }
        match operation {
            Operation::Status {} => Ok(
                json!({"running": true, "protocol": 1, "machine": self.machine, "participant_auth": "native binding required"}),
            ),
            Operation::Send { draft } => {
                self.check_project(context, &draft.project)?;
                let actor = self.actor(context)?;
                if let Some(committed) = self.store.retry(&actor, &draft)? {
                    return Ok(serde_json::to_value(committed)?);
                }
                if let Some(id) = &draft.reply_to {
                    ensure!(
                        self.show(context, id)?.draft.project == draft.project,
                        "Chat reply belongs to another project"
                    );
                }
                self.validate_targets(&draft)?;
                Ok(serde_json::to_value(self.store.send(&actor, &draft)?)?)
            }
            Operation::Reply { id, key, body, all } => {
                let original = self.show(context, &id)?;
                let actor = self.actor(context)?;
                let draft = Draft {
                    key,
                    project: original.draft.project.clone(),
                    to: reply_targets(&original, &actor, all),
                    body,
                    reply_to: Some(id),
                };
                self.handle(context, Operation::Send { draft })
            }
            Operation::Show { id } => Ok(serde_json::to_value(self.show(context, &id)?)?),
            Operation::Sessions { project } => {
                self.check_project(context, &project)?;
                let sessions: Vec<Value> = Registry::new(&mut self.store)?.snapshot(&project).into_iter().map(|r| json!({"session": r.session, "name": r.name, "client": r.client, "eligible": r.eligible})).collect();
                Ok(json!({"sessions": sessions}))
            }
            Operation::History {
                project,
                cursor,
                limit,
            } => self.page(context, &project, "history", cursor, limit),
            Operation::Subscribe {
                project,
                cursor,
                limit,
            } => self.page(context, &project, "subscribe", cursor, limit),
            Operation::Inbox { cursor, limit } => {
                let VerifiedContext::Participant(registration) = context else {
                    bail!("Inbox requires participant authority");
                };
                self.page(context, &registration.project, "inbox", cursor, limit)
            }
            Operation::Retry { id } => {
                let VerifiedContext::Participant(registration) = context else {
                    bail!("Retry requires participant authority");
                };
                self.show(context, &id)?;
                self.store.retry_delivery(&registration.session, &id)?;
                Ok(
                    json!({"retry": id, "warning": "Duplicate delivery is possible; retry remains bound to the original recipient incarnation"}),
                )
            }
            Operation::FetchComplete { ids } => {
                let VerifiedContext::Participant(registration) = context else {
                    bail!("Fetch completion requires participant authority");
                };
                self.store.record_fetch(&registration.session, &ids)?;
                Ok(json!({"fetched": ids}))
            }
            _ => bail!("operation requires integration authority"),
        }
    }

    fn verify_registration(&mut self, registration: &Registration) -> Result<()> {
        let registry = Registry::new(&mut self.store)?;
        let state = registry
            .state(&registration.session)
            .context("Chat session is not registered")?;
        let current = &state.registration;
        ensure!(
            !state.ended
                && current.project == registration.project
                && current.client == registration.client
                && current.native_id == registration.native_id
                && current.process_start == registration.process_start,
            "Chat binding is no longer valid"
        );
        Ok(())
    }

    fn actor(&self, context: &VerifiedContext) -> Result<Actor> {
        match context {
            VerifiedContext::Observer => Ok(Actor::Human {
                machine: self.machine.clone(),
            }),
            VerifiedContext::Participant(registration) => {
                Ok(Actor::Agent(registration.session.clone()))
            }
            _ => bail!("integration cannot impersonate a message author"),
        }
    }

    fn check_project(&self, context: &VerifiedContext, project: &str) -> Result<()> {
        if let VerifiedContext::Participant(registration) = context {
            ensure!(
                registration.project == project,
                "Chat project is outside this participant binding"
            );
        }
        Ok(())
    }

    fn show(&self, context: &VerifiedContext, id: &str) -> Result<Message> {
        let message = self.store.message(id)?;
        self.check_project(context, &message.draft.project)?;
        if let VerifiedContext::Participant(registration) = context {
            ensure!(
                message.sender == Actor::Agent(registration.session.clone())
                    || message
                        .draft
                        .to
                        .contains(&Target::Agent(registration.session.clone())),
                "Chat message is unavailable"
            );
        }
        Ok(message)
    }

    fn validate_targets(&mut self, draft: &Draft) -> Result<()> {
        let registry = Registry::new(&mut self.store)?;
        for target in &draft.to {
            if let Target::Agent(session) = target {
                let state = registry
                    .state(session)
                    .context("Chat recipient is not registered")?;
                ensure!(
                    !state.ended
                        && state.registration.eligible
                        && state.registration.project == draft.project,
                    "Chat recipient is unavailable in this project"
                );
            }
        }
        Ok(())
    }

    fn page(
        &self,
        context: &VerifiedContext,
        project: &str,
        query: &str,
        token: Option<String>,
        limit: u16,
    ) -> Result<Value> {
        self.check_project(context, project)?;
        let session = match context {
            VerifiedContext::Participant(r) => Some(r.session.clone()),
            _ => None,
        };
        let mut cursor = Cursor {
            version: 1,
            role: if session.is_some() {
                "participant"
            } else {
                "observer"
            }
            .into(),
            session,
            project: project.into(),
            query: query.into(),
            after: 0,
        };
        if let Some(token) = token {
            let decoded = self.decode_cursor(&token)?;
            cursor.after = decoded.after;
            ensure!(
                cursor == decoded,
                "Chat cursor does not match this role, participant, project or query"
            );
        }
        let page = self.store.feed(
            project,
            cursor.session.as_ref(),
            cursor.after,
            limit,
            query == "inbox",
        )?;
        let mut events = Vec::with_capacity(page.len());
        for (sequence, event) in page {
            cursor.after = sequence;
            events.push(json!({"sequence": sequence, "event": event}));
        }
        Ok(json!({"events": events, "cursor": self.encode_cursor(&cursor)?}))
    }

    fn encode_cursor(&self, cursor: &Cursor) -> Result<String> {
        let payload = serde_json::to_vec(cursor)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.cursor_key.as_bytes())?;
        mac.update(&payload);
        Ok(format!(
            "{}.{}",
            hex(&payload),
            hex(&mac.finalize().into_bytes())
        ))
    }

    fn decode_cursor(&self, token: &str) -> Result<Cursor> {
        let (payload, signature) = token.split_once('.').context("invalid Chat cursor")?;
        let payload = unhex(payload)?;
        let signature = unhex(signature)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.cursor_key.as_bytes())?;
        mac.update(&payload);
        mac.verify_slice(&signature)
            .context("invalid Chat cursor signature")?;
        Ok(serde_json::from_slice(&payload)?)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>> {
    ensure!(
        value.len().is_multiple_of(2) && value.is_ascii(),
        "invalid Chat cursor encoding"
    );
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(Into::into))
        .collect()
}

fn validate_peer_owner(uid: u32, expected: u32) -> Result<()> {
    ensure!(uid == expected, "Chat peer has another owner");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::{Draft, SessionRef, Target};

    const MACHINE: &str = "11111111-1111-4111-8111-111111111111";
    const PROJECT: &str = "22222222-2222-4222-8222-222222222222";

    fn registration(id: &str) -> Registration {
        Registration {
            session: SessionRef {
                machine: MACHINE.into(),
                incarnation: id.into(),
            },
            project: PROJECT.into(),
            name: id.into(),
            client: "codex".into(),
            native_id: id.into(),
            process_start: format!("boot:1:{id}"),
            eligible: true,
        }
    }

    #[test]
    fn delivery_finish_retry_and_notifications_are_recipient_scoped() {
        use crate::chat::types::{ClientEvent, Handoff};
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let a = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let b = registration("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        for r in [&a, &b] {
            service
                .handle(
                    &VerifiedContext::Integration(r.clone()),
                    Operation::Register {},
                )
                .unwrap();
        }
        let message = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "one".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(a.session.clone())],
                        body: "hello".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        let id = message["id"].as_str().unwrap().to_owned();
        let integration = VerifiedContext::Integration(a.clone());
        let claimed = service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Hook,
                    epoch: 1,
                },
            )
            .unwrap();
        let attempt = claimed["id"].as_str().unwrap().to_owned();
        let finish = Operation::Finish {
            attempt,
            outcome: Handoff::Unknown {
                reason: "hook stdout has no native acknowledgement".into(),
            },
        };
        assert!(service
            .handle(&VerifiedContext::Integration(b.clone()), finish.clone())
            .is_err());
        assert!(service
            .handle(&VerifiedContext::Participant(a.clone()), finish.clone())
            .is_err());
        let changes = Changes::default();
        let (response, result) = mpsc::sync_channel(1);
        apply_work(
            &mut service,
            Work {
                context: integration.clone(),
                operation: finish,
                response,
                deadline: Instant::now() + Duration::from_secs(1),
            },
            &changes,
        );
        result.recv().unwrap().unwrap();
        assert_eq!(*changes.generation.lock().unwrap(), 1);
        let feed = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Subscribe {
                    project: PROJECT.into(),
                    cursor: None,
                    limit: 100,
                },
            )
            .unwrap();
        assert_eq!(feed["events"][2]["event"]["state"], "unknown");
        assert!(service
            .handle(
                &VerifiedContext::Observer,
                Operation::Retry { id: id.clone() }
            )
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Participant(b),
                Operation::Retry { id: id.clone() }
            )
            .is_err());
        let retried = service
            .handle(&VerifiedContext::Participant(a), Operation::Retry { id })
            .unwrap();
        assert!(retried["warning"].as_str().unwrap().contains("Duplicate"));
        assert!(service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Idle,
                    epoch: 1
                }
            )
            .unwrap()
            .is_null());
        assert!(service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Idle,
                    epoch: 2
                }
            )
            .unwrap()
            .is_object());
    }

    #[test]
    fn inbox_socket_completion_records_each_full_page_but_failed_write_does_not() {
        use crate::chat::types::FeedEvent;
        struct TestBinding(Registration);
        impl BindingValidator for TestBinding {
            fn validate(&self, _: PeerIdentity, _: &str) -> Result<VerifiedContext> {
                Ok(VerifiedContext::Participant(self.0.clone()))
            }
        }
        for fail_after_bytes in [None, Some(0), Some(128)] {
            let fail_write = fail_after_bytes.is_some();
            let dir = tempfile::tempdir().unwrap();
            let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
            let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
            service
                .handle(
                    &VerifiedContext::Integration(recipient.clone()),
                    Operation::Register {},
                )
                .unwrap();
            for key in ["long", "short"] {
                service
                    .handle(
                        &VerifiedContext::Observer,
                        Operation::Send {
                            draft: Draft {
                                key: key.into(),
                                project: PROJECT.into(),
                                to: vec![Target::Agent(recipient.session.clone())],
                                body: if key == "long" {
                                    "é".repeat(32000)
                                } else {
                                    "next".into()
                                },
                                reply_to: None,
                            },
                        },
                    )
                    .unwrap();
            }
            let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE);
            let changes = Arc::new(Changes::default());
            let owner_changes = changes.clone();
            let owner = thread::spawn(move || {
                while let Ok(work) = receiver.recv() {
                    apply_work(&mut service, work, &owner_changes);
                }
                service
            });
            let mut cursor = None;
            for _ in 0..if fail_write { 1 } else { 2 } {
                let (mut client, mut server) = UnixStream::pair().unwrap();
                let buffer_size: libc::c_int = 4096;
                assert_eq!(
                    unsafe {
                        libc::setsockopt(
                            server.as_raw_fd(),
                            libc::SOL_SOCKET,
                            libc::SO_SNDBUF,
                            (&buffer_size as *const libc::c_int).cast(),
                            std::mem::size_of_val(&buffer_size) as libc::socklen_t,
                        )
                    },
                    0
                );
                protocol::write_frame(
                    &mut client,
                    &json!({"version": 1, "binding": "private-fixture"}),
                )
                .unwrap();
                protocol::write_frame(&mut client, &json!({"version": 1, "operation": {"op": "inbox", "cursor": cursor, "limit": 1}})).unwrap();
                if fail_after_bytes == Some(0) {
                    client.shutdown(std::net::Shutdown::Read).unwrap();
                }
                let sender = sender.clone();
                let changes = changes.clone();
                let validator = TestBinding(recipient.clone());
                let worker = thread::spawn(move || {
                    serve_connection(
                        &mut server,
                        false,
                        &validator,
                        sender,
                        &AtomicBool::new(false),
                        &changes,
                        &Mutex::new(0),
                    )
                });
                if !fail_write {
                    let page = protocol::read_frame(&mut client).unwrap();
                    assert_eq!(page["data"]["events"].as_array().unwrap().len(), 1);
                    cursor = page["data"]["cursor"].as_str().map(str::to_owned);
                }
                if fail_after_bytes == Some(128) {
                    use std::io::Read;
                    client.read_exact(&mut [0_u8; 128]).unwrap();
                    client.shutdown(std::net::Shutdown::Read).unwrap();
                }
                assert_eq!(worker.join().unwrap().is_err(), fail_write);
            }
            drop(sender);
            let mut service = owner.join().unwrap();
            let events = service.store.feed(PROJECT, None, 0, 100, false).unwrap();
            let fetched = events
                .iter()
                .filter(|(_, event)| matches!(event, FeedEvent::Fetched { .. }))
                .count();
            assert_eq!(fetched, if fail_write { 0 } else { 2 });
            assert_eq!(*changes.generation.lock().unwrap(), fetched as u64);
            assert_eq!(
                service
                    .store
                    .claim(&recipient.session, super::super::config::DEFAULT_LIMITS)
                    .unwrap()
                    .is_none(),
                !fail_write
            );
        }
    }

    #[test]
    fn participant_scope_and_observer_reads_preserve_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let a = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let b = registration("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        let c = registration("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
        for registration in [&a, &b, &c] {
            service
                .handle(
                    &VerifiedContext::Integration(registration.clone()),
                    Operation::Register {},
                )
                .unwrap();
        }
        let draft = Draft {
            key: "one".into(),
            project: PROJECT.into(),
            to: vec![Target::Agent(b.session.clone())],
            body: "private".into(),
            reply_to: None,
        };
        let sent = service
            .handle(
                &VerifiedContext::Participant(a.clone()),
                Operation::Send {
                    draft: draft.clone(),
                },
            )
            .unwrap();
        let id = sent["id"].as_str().unwrap().to_string();
        assert!(service
            .handle(
                &VerifiedContext::Participant(c.clone()),
                Operation::Show { id: id.clone() }
            )
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Participant(c.clone()),
                Operation::Reply {
                    id: id.clone(),
                    key: "reply".into(),
                    body: "stolen".into(),
                    all: false
                }
            )
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Participant(a.clone()),
                Operation::History {
                    project: "33333333-3333-4333-8333-333333333333".into(),
                    cursor: None,
                    limit: 10
                }
            )
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Participant(a),
                Operation::SetState { eligible: false }
            )
            .is_err());
        let observed = service
            .handle(
                &VerifiedContext::Observer,
                Operation::History {
                    project: PROJECT.into(),
                    cursor: None,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(observed["events"].as_array().unwrap().len(), 1);
        let inbox = service
            .handle(
                &VerifiedContext::Participant(b.clone()),
                Operation::Inbox {
                    cursor: None,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(inbox["events"][0]["event"]["message"]["id"], id);
        assert!(service
            .handle(
                &VerifiedContext::Participant(b.clone()),
                Operation::Inbox {
                    cursor: Some(observed["cursor"].as_str().unwrap().into()),
                    limit: 10
                }
            )
            .is_err());
        let again = service
            .handle(
                &VerifiedContext::Participant(b.clone()),
                Operation::Inbox {
                    cursor: None,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(again["events"], inbox["events"]);
        assert!(service
            .handle(
                &VerifiedContext::Participant(c.clone()),
                Operation::Inbox {
                    cursor: Some(inbox["cursor"].as_str().unwrap().into()),
                    limit: 10
                }
            )
            .is_err());
        let outsider = service
            .handle(
                &VerifiedContext::Participant(c),
                Operation::History {
                    project: PROJECT.into(),
                    cursor: None,
                    limit: 10,
                },
            )
            .unwrap();
        assert_eq!(outsider["events"], json!([]));
        let reply = service
            .handle(
                &VerifiedContext::Participant(b),
                Operation::Reply {
                    id: id.clone(),
                    key: "legitimate-reply".into(),
                    body: "received".into(),
                    all: false,
                },
            )
            .unwrap();
        assert_eq!(reply["draft"]["reply_to"], id);
        assert_eq!(
            reply["draft"]["to"][0]["Agent"]["incarnation"],
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        );
        let human_reply = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Reply {
                    id,
                    key: "human-reply".into(),
                    body: "human answer".into(),
                    all: false,
                },
            )
            .unwrap();
        assert_eq!(human_reply["sender"]["Human"]["machine"], MACHINE);
        assert!(service
            .handle(
                &VerifiedContext::Observer,
                Operation::Inbox {
                    cursor: None,
                    limit: 10
                }
            )
            .is_err());
    }

    #[test]
    fn cursor_survives_restart_and_binds_query_project_and_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut service = Service::open(&path, MACHINE).unwrap();
        for key in ["first", "second"] {
            service
                .handle(
                    &VerifiedContext::Observer,
                    Operation::Send {
                        draft: Draft {
                            key: key.into(),
                            project: PROJECT.into(),
                            to: vec![Target::Human {
                                machine: MACHINE.into(),
                            }],
                            body: key.into(),
                            reply_to: None,
                        },
                    },
                )
                .unwrap();
        }
        let page = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Subscribe {
                    project: PROJECT.into(),
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(
            page["events"][0]["event"]["message"]["draft"]["body"],
            "first"
        );
        let cursor = page["cursor"].as_str().unwrap().to_string();
        drop(service);
        let mut service = Service::open(&path, MACHINE).unwrap();
        let page = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Subscribe {
                    project: PROJECT.into(),
                    cursor: Some(cursor.clone()),
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(
            page["events"][0]["event"]["message"]["draft"]["body"],
            "second"
        );
        assert!(service
            .handle(
                &VerifiedContext::Observer,
                Operation::History {
                    project: PROJECT.into(),
                    cursor: Some(cursor.clone()),
                    limit: 1
                }
            )
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Observer,
                Operation::Subscribe {
                    project: "33333333-3333-4333-8333-333333333333".into(),
                    cursor: Some(cursor.clone()),
                    limit: 1
                }
            )
            .is_err());
        let mut tampered = cursor.into_bytes();
        tampered[0] = if tampered[0] == b'0' { b'1' } else { b'0' };
        assert!(service
            .handle(
                &VerifiedContext::Observer,
                Operation::Subscribe {
                    project: PROJECT.into(),
                    cursor: Some(String::from_utf8(tampered).unwrap()),
                    limit: 1
                }
            )
            .is_err());
    }

    #[test]
    fn peer_owner_must_match_effective_user() {
        assert!(validate_peer_owner(1000, 1000).is_ok());
        assert!(validate_peer_owner(1001, 1000).is_err());
    }

    #[test]
    fn enrollment_registers_and_reuses_incarnation_but_has_no_participant_authority() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let enrollment = VerifiedContext::Enrollment {
            project: PROJECT.into(),
            name: "test-agent".into(),
            evidence: crate::chat::registry::NativeEvidence {
                client: crate::chat::registry::ClientKind::Codex,
                native_id: "native-thread".into(),
                process_start: "boot:42:100".into(),
            },
            eligible: true,
        };
        assert!(service.handle(&enrollment, Operation::Status {}).is_err());
        assert!(service
            .handle(
                &enrollment,
                Operation::Inbox {
                    cursor: None,
                    limit: 10
                }
            )
            .is_err());
        let first = service.handle(&enrollment, Operation::Register {}).unwrap();
        let again = service.handle(&enrollment, Operation::Register {}).unwrap();
        assert_eq!(first["session"], again["session"]);
        assert_eq!(first["name"], "test-agent");
        assert!(first.get("native_id").is_none());
        let mut restarted = enrollment;
        if let VerifiedContext::Enrollment { evidence, .. } = &mut restarted {
            evidence.process_start = "boot:42:101".into();
        }
        let next = service.handle(&restarted, Operation::Register {}).unwrap();
        assert_ne!(first["session"], next["session"]);
    }

    #[test]
    fn retry_keeps_committed_recipients_after_opt_out_but_new_send_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        let draft = Draft {
            key: "retry".into(),
            project: PROJECT.into(),
            to: vec![Target::Agent(recipient.session)],
            body: "one".into(),
            reply_to: None,
        };
        let original = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: draft.clone(),
                },
            )
            .unwrap();
        service
            .handle(&integration, Operation::SetState { eligible: false })
            .unwrap();
        let retry = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: draft.clone(),
                },
            )
            .unwrap();
        assert_eq!(retry, original);
        let mut next = draft;
        next.key = "new".into();
        assert!(service
            .handle(&VerifiedContext::Observer, Operation::Send { draft: next })
            .is_err());
    }

    #[test]
    fn observer_cannot_link_reply_to_another_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let mut draft = Draft {
            key: "original".into(),
            project: PROJECT.into(),
            to: vec![Target::Human {
                machine: MACHINE.into(),
            }],
            body: "one".into(),
            reply_to: None,
        };
        let original = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: draft.clone(),
                },
            )
            .unwrap();
        draft.key = "reply".into();
        draft.project = "33333333-3333-4333-8333-333333333333".into();
        draft.reply_to = Some(original["id"].as_str().unwrap().into());
        assert!(service
            .handle(&VerifiedContext::Observer, Operation::Send { draft })
            .is_err());
    }

    #[test]
    fn integration_validator_cannot_promote_participant_endpoint_to_observer() {
        struct BadValidator;
        impl BindingValidator for BadValidator {
            fn validate(&self, _peer: PeerIdentity, _binding: &str) -> Result<VerifiedContext> {
                Ok(VerifiedContext::Observer)
            }
        }
        let (mut client, mut server) = UnixStream::pair().unwrap();
        protocol::write_frame(
            &mut client,
            &json!({"version": 1, "binding": "valid-but-wrong-role"}),
        )
        .unwrap();
        let (sender, _receiver) = mpsc::sync_channel(1);
        let error = serve_connection(
            &mut server,
            false,
            &BadValidator,
            sender,
            &AtomicBool::new(false),
            &Changes::default(),
            &Mutex::new(0),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot grant observer"));
    }

    #[test]
    fn full_writer_queue_and_subscription_slots_refuse_without_waiting() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (response, _reply) = mpsc::sync_channel(1);
        sender
            .try_send(Work {
                context: VerifiedContext::Observer,
                operation: Operation::Status {},
                response,
                deadline: Instant::now() + Duration::from_secs(6),
            })
            .unwrap();
        let started = Instant::now();
        assert!(dispatch(&sender, &VerifiedContext::Observer, Operation::Status {}).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        let subscriptions = Mutex::new(0);
        let slots: Vec<_> = (0..MAX_SUBSCRIPTIONS)
            .map(|_| SubscriptionSlot::acquire(&subscriptions).unwrap())
            .collect();
        assert!(SubscriptionSlot::acquire(&subscriptions).is_err());
        drop(slots);
        assert!(SubscriptionSlot::acquire(&subscriptions).is_ok());
    }

    #[test]
    fn frame_deadline_expires_even_if_peer_trickles_bytes() {
        use std::io::Write;
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let writer = thread::spawn(move || {
            for byte in [0, 0, 0, 1, b'0'] {
                if client.write_all(&[byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(35));
            }
        });
        let started = Instant::now();
        let result = protocol::read_frame(&mut DeadlineStream::new(
            &mut server,
            Duration::from_millis(70),
        ));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(130));
        drop(server);
        writer.join().unwrap();
    }
}
