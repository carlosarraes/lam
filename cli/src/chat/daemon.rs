use anyhow::{bail, ensure, Context, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap};
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
use super::types::{Actor, ClientEvent, Draft, Limits, Message, Registration, SessionRef, Target};

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
    fn status(&self) -> &'static str {
        "custom validator"
    }
}

struct UnavailableBindings;
impl BindingValidator for UnavailableBindings {
    fn status(&self) -> &'static str {
        "disabled"
    }
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

#[cfg(target_os = "linux")]
enum QueueAction {
    Claim {
        registration: Registration,
        token: u64,
        event: ClientEvent,
        epoch: u64,
        response: SyncSender<Option<super::types::Attempt>>,
    },
    Finish {
        attempt: String,
        outcome: super::types::Handoff,
    },
    Done {
        session: SessionRef,
        token: u64,
    },
}

#[derive(Default)]
struct Changes {
    generation: Mutex<u64>,
    changed: Condvar,
    fetches: Mutex<BTreeMap<String, FetchCompletion>>,
}

#[derive(Clone)]
struct FetchCompletion {
    recipient: SessionRef,
    ids: Vec<String>,
    ready: bool,
    failures: u8,
    next_retry: Instant,
}

/// Reserves bounded completion capacity before socket I/O. Dropping an unused
/// reservation, including during unwinding, cancels it without claiming a fetch.
struct FetchReservation<'a> {
    changes: &'a Changes,
    id: String,
    completed: bool,
}
impl FetchReservation<'_> {
    fn complete(mut self) {
        self.changes
            .fetches
            .lock()
            .unwrap()
            .get_mut(&self.id)
            .unwrap()
            .ready = true;
        self.completed = true;
    }
}
impl Drop for FetchReservation<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.changes.fetches.lock().unwrap().remove(&self.id);
        }
    }
}

impl Changes {
    fn notify(&self) {
        let mut generation = self.generation.lock().unwrap();
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
    }

    fn reserve_fetch(
        &self,
        recipient: &SessionRef,
        ids: Vec<String>,
    ) -> Result<FetchReservation<'_>> {
        let mut fetches = self.fetches.lock().unwrap();
        ensure!(
            fetches.len() < MAX_CONNECTIONS,
            "Chat fetch completion backlog is full; retry the fetch later"
        );
        let id = uuid::Uuid::new_v4().to_string();
        fetches.insert(
            id.clone(),
            FetchCompletion {
                recipient: recipient.clone(),
                ids,
                ready: false,
                failures: 0,
                next_retry: Instant::now(),
            },
        );
        Ok(FetchReservation {
            changes: self,
            id,
            completed: false,
        })
    }

    fn fetch_pending(&self, recipient: &SessionRef) -> bool {
        self.fetches
            .lock()
            .unwrap()
            .values()
            .any(|fetch| &fetch.recipient == recipient)
    }

    fn fetch_status(&self) -> Value {
        let fetches = self.fetches.lock().unwrap();
        let failed = fetches.values().filter(|fetch| fetch.failures > 0).count();
        json!({"unconfirmed": fetches.len(), "failed": failed,
            "diagnostic": if failed > 0 { Some("Fetch persistence failed; automatic delivery is paused for affected recipients while completion retries") } else { None }})
    }
}

/// One due completion per owner turn keeps unrelated recipients responsive.
/// SQLite work happens after releasing the backlog lock, with bounded backoff on
/// failure. This is persistence retry, never a native delivery retry.
fn repair_fetch(service: &mut Service, changes: &Changes, now: Instant) -> bool {
    let next = changes
        .fetches
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, fetch)| fetch.ready && fetch.next_retry <= now)
        .min_by_key(|(_, fetch)| fetch.next_retry)
        .map(|(id, fetch)| (id.clone(), fetch.clone()));
    let Some((id, fetch)) = next else {
        return false;
    };
    match service.store.record_fetch(&fetch.recipient, &fetch.ids) {
        Ok(()) => {
            changes.fetches.lock().unwrap().remove(&id);
            changes.notify();
        }
        Err(_) => {
            let mut fetches = changes.fetches.lock().unwrap();
            let fetch = fetches.get_mut(&id).unwrap();
            let first_failure = fetch.failures == 0;
            fetch.failures = fetch.failures.saturating_add(1);
            fetch.next_retry =
                now.max(Instant::now()) + Duration::from_millis(100_u64 << fetch.failures.min(8));
            drop(fetches);
            if first_failure {
                eprintln!("Chat fetch completion persistence failed; retaining unconfirmed transfer and pausing automatic delivery for its recipient");
            }
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn run_queue_worker(
    directory: std::path::PathBuf,
    registration: Registration,
    token: u64,
    reserved_epoch: Option<u64>,
    sender: SyncSender<QueueAction>,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let epoch = match reserved_epoch {
        Some(epoch) => epoch,
        None => {
            match super::adapters::reserve_codex_queue_epoch(&directory, &registration, deadline) {
                Ok(epoch) => epoch,
                Err(_) => {
                    let _ = sender.send(QueueAction::Done {
                        session: registration.session,
                        token,
                    });
                    return;
                }
            }
        }
    };
    let result =
        super::adapters::submit_queue_owned(&directory, &registration, deadline, |event| {
            let (response, receiver) = mpsc::sync_channel(1);
            sender.try_send(QueueAction::Claim {
                registration: registration.clone(),
                token,
                event,
                epoch,
                response,
            })?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("native queue claim deadline expired")?;
            Ok(receiver.recv_timeout(remaining)?)
        });
    if let Some((attempt, outcome)) = result {
        let _ = sender.send(QueueAction::Finish {
            attempt: attempt.id,
            outcome,
        });
    }
    let _ = sender.send(QueueAction::Done {
        session: registration.session,
        token,
    });
}

#[cfg(target_os = "linux")]
fn apply_queue_action(
    service: &mut Service,
    action: QueueAction,
    changes: &Changes,
    running: &mut usize,
) {
    match action {
        QueueAction::Claim {
            registration,
            token,
            event,
            epoch,
            response,
        } => {
            let attempt = service
                .queue_claim(
                    &registration,
                    token,
                    event,
                    epoch,
                    changes.fetch_pending(&registration.session),
                )
                .ok()
                .flatten();
            if let Some(attempt) = &attempt {
                service
                    .queue_deadlines
                    .insert(attempt.id.clone(), Instant::now() + Duration::from_secs(3));
                changes.notify();
            }
            let _ = response.try_send(attempt);
        }
        QueueAction::Finish { attempt, outcome } => {
            if service.store.finish(&attempt, outcome).is_ok() {
                service.queue_deadlines.remove(&attempt);
                changes.notify();
            }
        }
        QueueAction::Done { session, token } => {
            service.queue_done(&session, token);
            *running = running.saturating_sub(1);
        }
    }
}

fn run_owner(mut service: Service, receiver: mpsc::Receiver<Work>, changes: &Changes) -> Service {
    #[cfg(target_os = "linux")]
    let (queue_sender, queue_receiver) = mpsc::sync_channel::<QueueAction>(WRITER_QUEUE);
    #[cfg(target_os = "linux")]
    let mut queue_running = 0_usize;
    loop {
        if service.expire_hooks(Instant::now()) {
            changes.notify();
        }
        #[cfg(target_os = "linux")]
        {
            if service.expire_queues(Instant::now()) {
                changes.notify();
            }
            for _ in 0..WRITER_QUEUE {
                let Ok(action) = queue_receiver.try_recv() else {
                    break;
                };
                apply_queue_action(&mut service, action, changes, &mut queue_running);
            }
            if queue_running < 4 {
                if let Some((registration, token)) = service.next_queue() {
                    if let Some(epoch) = service.queue_epoch(&registration.session, token) {
                        let directory = service.bindings_directory.clone();
                        let worker_sender = queue_sender.clone();
                        let session = registration.session.clone();
                        if thread::Builder::new()
                            .name("lam-chat-queue".into())
                            .spawn(move || {
                                run_queue_worker(
                                    directory,
                                    registration,
                                    token,
                                    epoch,
                                    worker_sender,
                                )
                            })
                            .is_ok()
                        {
                            queue_running += 1;
                        } else {
                            service.queue_done(&session, token);
                        }
                    } else {
                        service.queue_done(&registration.session, token);
                    }
                }
            }
        }
        repair_fetch(&mut service, changes, Instant::now());
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(work) => apply_work(&mut service, work, changes),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // Connection workers have exited and their reservation guards have settled.
    for _ in 0..MAX_CONNECTIONS {
        if !repair_fetch(&mut service, changes, Instant::now()) {
            break;
        }
    }
    if !changes.fetches.lock().unwrap().is_empty() {
        eprintln!("Chat stopped with unconfirmed fetch completions; fetch transfer persistence is uncertain");
    }
    service
}

pub fn run(paths: &Paths) -> Result<()> {
    run_with_validator(paths, Arc::new(UnavailableBindings))
}

pub fn run_with_validator(paths: &Paths, validator: Arc<dyn BindingValidator>) -> Result<()> {
    paths.validate()?;
    let _lock = lock_instance(&paths.lock)?;
    let config = Config::load_or_create(&paths.config)?;
    let mut service = Service::open(&paths.database, &config.machine)?;
    service.participant_auth = validator.status();
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
    let writer_changes = changes.clone();
    let writer = thread::spawn(move || {
        run_owner(service, receiver, &writer_changes);
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
            | Operation::End {}
            | Operation::SetState { .. }
            | Operation::Delivery { .. }
            | Operation::Observe { .. }
            | Operation::Finish { .. }
            | Operation::Retry { .. }
    );
    let status = matches!(work.operation, Operation::Status {});
    let mut result = match (&work.context, &work.operation) {
        (VerifiedContext::Integration(registration), Operation::Delivery { event, epoch })
            if changes.fetch_pending(&registration.session) =>
        {
            service.delivery(registration, *event, *epoch, true)
        }
        _ => service.handle(&work.context, work.operation),
    };
    if status {
        if let Ok(data) = &mut result {
            data["fetch_completions"] = changes.fetch_status();
        }
    }
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
        let reservation = if let VerifiedContext::Participant(registration) = &context {
            if ids.is_empty() {
                None
            } else {
                Some(changes.reserve_fetch(&registration.session, ids)?)
            }
        } else {
            None
        };
        respond(stream, page)?;
        // Every returned message contains its complete body. Only a completed
        // frame records transfer; failed/partial writes leave fetch state alone.
        if let Some(reservation) = reservation {
            reservation.complete();
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
pub(super) struct DeadlineStream<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}
impl<'a> DeadlineStream<'a> {
    pub(super) fn until(stream: &'a mut UnixStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }
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
    participant_auth: &'static str,
    store: Store,
    machine: String,
    cursor_key: String,
    limits: Limits,
    hook_deadlines: BTreeMap<String, Instant>,
    queue_deadlines: BTreeMap<String, Instant>,
    bindings_directory: std::path::PathBuf,
    queue: HashMap<SessionRef, QueueWake>,
    next_queue_token: u64,
}

struct QueueWake {
    registration: Registration,
    stop_epoch: u64,
    claim_epoch: Option<u64>,
    external_state: ClientEvent,
    ready: bool,
    in_flight: Option<u64>,
    arrival_during_flight: bool,
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
    fn observe_native(
        &mut self,
        registration: &Registration,
        event: ClientEvent,
        epoch: u64,
    ) -> Result<bool> {
        self.verify_registration(registration)?;
        if !self.store.observe(&registration.session, event, epoch)? {
            return Ok(false);
        }
        let ready = event == ClientEvent::Idle && self.store.has_pending(&registration.session)?;
        let wake = self
            .queue
            .entry(registration.session.clone())
            .or_insert_with(|| QueueWake {
                registration: registration.clone(),
                stop_epoch: epoch,
                claim_epoch: None,
                external_state: event,
                ready: false,
                in_flight: None,
                arrival_during_flight: false,
            });
        wake.registration = registration.clone();
        wake.external_state = event;
        // A newer native observation supersedes any worker still holding an
        // older Stop token. Its late claim or completion cannot own this wake.
        wake.in_flight = None;
        wake.arrival_during_flight = false;
        if event == ClientEvent::Idle {
            wake.stop_epoch = epoch;
            wake.claim_epoch = epoch.checked_add(1);
            wake.ready = ready;
        } else {
            wake.claim_epoch = None;
            wake.ready = false;
        }
        Ok(true)
    }

    fn next_queue(&mut self) -> Option<(Registration, u64)> {
        let wake = self
            .queue
            .values_mut()
            .find(|wake| wake.ready && wake.in_flight.is_none())?;
        self.next_queue_token = self.next_queue_token.wrapping_add(1);
        let token = self.next_queue_token;
        wake.ready = false;
        wake.in_flight = Some(token);
        Some((wake.registration.clone(), token))
    }

    fn queue_done(&mut self, session: &SessionRef, token: u64) {
        if let Some(wake) = self.queue.get_mut(session) {
            if wake.in_flight == Some(token) {
                wake.in_flight = None;
                if wake.arrival_during_flight && wake.external_state == ClientEvent::Idle {
                    wake.ready = true;
                    wake.claim_epoch = None;
                }
                wake.arrival_during_flight = false;
            }
        }
    }

    fn queue_epoch(&self, session: &SessionRef, token: u64) -> Option<Option<u64>> {
        let wake = self.queue.get(session)?;
        (wake.in_flight == Some(token)).then_some(wake.claim_epoch)
    }

    fn note_arrival(&mut self, message: &Message) {
        for target in &message.draft.to {
            if let Target::Agent(session) = target {
                if let Some(wake) = self.queue.get_mut(session) {
                    if wake.external_state == ClientEvent::Idle && wake.in_flight.is_none() {
                        wake.ready = true;
                        // A new arrival after an earlier queue claim needs its
                        // own file-reserved native epoch, not the spent Stop+1.
                        wake.claim_epoch = None;
                    } else if wake.external_state == ClientEvent::Idle {
                        wake.arrival_during_flight = true;
                    }
                }
            }
        }
    }

    fn queue_claim(
        &mut self,
        registration: &Registration,
        token: u64,
        event: ClientEvent,
        epoch: u64,
        fetch_pending: bool,
    ) -> Result<Option<super::types::Attempt>> {
        if fetch_pending || !matches!(event, ClientEvent::Idle | ClientEvent::Busy) {
            return Ok(None);
        }
        let Some(wake) = self.queue.get(&registration.session) else {
            return Ok(None);
        };
        if wake.in_flight != Some(token)
            || wake.external_state != ClientEvent::Idle
            || wake.registration != *registration
            || wake.claim_epoch.is_some_and(|reserved| reserved != epoch)
            || epoch <= wake.stop_epoch
        {
            return Ok(None);
        }
        if self.registration_ended(registration)?
            || !Registry::new(&mut self.store)?
                .state(&registration.session)
                .is_some_and(|state| state.registration.eligible)
        {
            return Ok(None);
        }
        if !self.store.observe(&registration.session, event, epoch)? {
            return Ok(None);
        }
        self.store.claim(&registration.session, self.limits)
    }

    fn open(path: &Path, machine: &str) -> Result<Self> {
        let mut store = Store::open(path)?;
        store.set_machine(machine)?;
        let cursor_key = store.cursor_key()?;
        store.recover_submitting()?;
        Ok(Self {
            participant_auth: "disabled",
            store,
            machine: machine.into(),
            cursor_key,
            limits: super::config::DEFAULT_LIMITS,
            hook_deadlines: BTreeMap::new(),
            queue_deadlines: BTreeMap::new(),
            bindings_directory: path
                .parent()
                .context("Chat database has no parent directory")?
                .join("bindings"),
            queue: HashMap::new(),
            next_queue_token: 0,
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
                Operation::Lifecycle {} => {
                    let ended = self.registration_ended(registration)?;
                    Ok(json!({"session": registration.session, "ended": ended}))
                }
                Operation::End {} => {
                    self.verify_registration(registration)?;
                    Registry::new(&mut self.store)?.end(&registration.session)?;
                    self.queue.remove(&registration.session);
                    Ok(json!({"ended": registration.session}))
                }
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
                    if !eligible {
                        self.queue.remove(&registration.session);
                    }
                    Ok(json!({"eligible": eligible}))
                }
                Operation::Delivery { event, epoch } => {
                    self.delivery(registration, event, epoch, false)
                }
                Operation::Observe { event, epoch } => {
                    self.observe_native(registration, event, epoch)?;
                    Ok(Value::Null)
                }
                Operation::Finish { attempt, outcome } => {
                    self.verify_registration(registration)?;
                    ensure!(
                        self.store.attempt_recipient(&attempt)? == registration.session,
                        "Chat attempt belongs to another recipient"
                    );
                    self.store.finish(&attempt, outcome)?;
                    self.hook_deadlines.remove(&attempt);
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
                json!({"running": true, "protocol": 1, "machine": self.machine, "participant_auth": self.participant_auth}),
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
                let message = self.store.send(&actor, &draft)?;
                self.note_arrival(&message);
                Ok(serde_json::to_value(message)?)
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
            _ => bail!("operation requires integration authority"),
        }
    }

    fn delivery(
        &mut self,
        registration: &Registration,
        event: ClientEvent,
        epoch: u64,
        fetch_pending: bool,
    ) -> Result<Value> {
        protocol::Operation::Delivery { event, epoch }.validate()?;
        self.verify_registration(registration)?;
        let eligible = Registry::new(&mut self.store)?
            .state(&registration.session)
            .is_some_and(|s| s.registration.eligible && !s.ended);
        ensure!(eligible, "Chat recipient is ineligible");
        if fetch_pending {
            self.observe_native(registration, event, epoch)?;
            return Ok(Value::Null);
        }
        ensure!(
            event != ClientEvent::Hook || self.hook_deadlines.len() < MAX_CONNECTIONS,
            "Chat hook handoff capacity is full"
        );
        if !self.observe_native(registration, event, epoch)? {
            return Ok(Value::Null);
        }
        // Direct integration delivery consumes this observation. Queue wakes
        // are reserved for the asynchronous Stop path only.
        if let Some(wake) = self.queue.get_mut(&registration.session) {
            wake.ready = false;
        }
        let attempt = if matches!(event, ClientEvent::Idle | ClientEvent::Hook) {
            self.store.claim(&registration.session, self.limits)?
        } else {
            None
        };
        if event == ClientEvent::Hook {
            if let Some(attempt) = &attempt {
                // Claim happens after hook startup. Its two-second expiry is
                // therefore later than the hook's start-relative output bound.
                self.hook_deadlines.insert(
                    attempt.id.clone(),
                    Instant::now() + super::delivery::HOOK_DEADLINE,
                );
            }
        }
        Ok(serde_json::to_value(attempt)?)
    }

    fn expire_hooks(&mut self, now: Instant) -> bool {
        let expired: Vec<_> = self
            .hook_deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        let mut changed = false;
        for id in expired {
            let outcome = super::types::Handoff::Unknown {
                reason: super::delivery::HOOK_UNCONFIRMED.into(),
            };
            if self.store.finish(&id, outcome).is_ok() {
                self.hook_deadlines.remove(&id);
                changed = true;
            } else {
                // Failed persistence retains durable Submitting and exclusion.
                // Retry at a bounded cadence, never requeue or expose the body.
                self.hook_deadlines
                    .insert(id, now + Duration::from_millis(100));
            }
        }
        changed
    }

    #[cfg(target_os = "linux")]
    fn expire_queues(&mut self, now: Instant) -> bool {
        let expired: Vec<_> = self
            .queue_deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        let mut changed = false;
        for id in expired {
            if self
                .store
                .finish(
                    &id,
                    super::types::Handoff::Unknown {
                        reason: "Codex native queue completion is unconfirmed".into(),
                    },
                )
                .is_ok()
            {
                self.queue_deadlines.remove(&id);
                changed = true;
            } else {
                self.queue_deadlines
                    .insert(id, now + Duration::from_millis(100));
            }
        }
        changed
    }

    fn verify_registration(&mut self, registration: &Registration) -> Result<()> {
        ensure!(
            !self.registration_ended(registration)?,
            "Chat binding is no longer valid"
        );
        Ok(())
    }

    fn registration_ended(&mut self, registration: &Registration) -> Result<bool> {
        let registry = Registry::new(&mut self.store)?;
        let state = registry
            .state(&registration.session)
            .context("Chat session is not registered")?;
        let current = &state.registration;
        ensure!(
            current.project == registration.project
                && current.client == registration.client
                && current.native_id == registration.native_id
                && current.process_start == registration.process_start,
            "Chat binding is no longer valid"
        );
        Ok(state.ended)
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
pub(super) mod tests {
    use super::*;

    /// Owned local socket fixture using the production connection dispatcher and
    /// sole Store owner. Only the native observation is supplied by its caller.
    pub(in crate::chat) fn owned_connections(
        listener: UnixListener,
        validator: Arc<dyn BindingValidator>,
        path: std::path::PathBuf,
        count: usize,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let service = Service::open(&path, MACHINE).unwrap();
            let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE);
            let changes = Arc::new(Changes::default());
            let owner_changes = changes.clone();
            let owner = thread::spawn(move || run_owner(service, receiver, &owner_changes));
            for _ in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                let result = serve_connection(
                    &mut stream,
                    false,
                    validator.as_ref(),
                    sender.clone(),
                    &AtomicBool::new(false),
                    &changes,
                    &Mutex::new(0),
                );
                if let Err(error) = result {
                    protocol::write_frame(
                        &mut stream,
                        &json!({"version":1,"ok":false,"error":error.to_string()}),
                    )
                    .unwrap();
                }
            }
            drop(sender);
            owner.join().unwrap();
        })
    }
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
    fn queue_claim_requires_current_stop_token_and_excludes_hook_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        let send = |service: &mut Service, key: &str| {
            service
                .handle(
                    &VerifiedContext::Observer,
                    Operation::Send {
                        draft: Draft {
                            key: key.into(),
                            project: PROJECT.into(),
                            to: vec![Target::Agent(recipient.session.clone())],
                            body: "owned queued message".into(),
                            reply_to: None,
                        },
                    },
                )
                .unwrap();
        };
        service
            .observe_native(&recipient, ClientEvent::Busy, 1)
            .unwrap();
        send(&mut service, "during-user-turn");
        assert!(
            service.next_queue().is_none(),
            "arrival must not manufacture Stop eligibility"
        );
        service
            .observe_native(&recipient, ClientEvent::Idle, 2)
            .unwrap();
        let (scheduled, token) = service.next_queue().unwrap();
        assert_eq!(scheduled, recipient);
        service
            .observe_native(&recipient, ClientEvent::Busy, 3)
            .unwrap();
        assert!(service
            .queue_claim(&recipient, token, ClientEvent::Busy, 4, false)
            .unwrap()
            .is_none());
        service.queue_done(&recipient.session, token);
        assert!(
            service.next_queue().is_none(),
            "stale work must not rearm itself"
        );

        service
            .observe_native(&recipient, ClientEvent::Idle, 5)
            .unwrap();
        let (_, token) = service.next_queue().unwrap();
        assert!(
            service
                .queue_claim(&recipient, token, ClientEvent::Busy, 7, false)
                .unwrap()
                .is_none(),
            "worker cannot invent a later native epoch"
        );
        assert!(
            service
                .queue_claim(&recipient, token, ClientEvent::Busy, 5, false)
                .unwrap()
                .is_none(),
            "persisted epoch must be fresh"
        );
        let attempt = service
            .queue_claim(&recipient, token, ClientEvent::Busy, 6, false)
            .unwrap()
            .unwrap();
        // Active is a legitimate queue snapshot while Stop is returning. It is
        // recorded as Busy, not fabricated Idle, and does not revoke Stop.
        let db = rusqlite::Connection::open(dir.path().join("chat.sqlite3")).unwrap();
        let state: String = db
            .query_row("SELECT state FROM delivery_observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "\"busy\"");
        send(&mut service, "during-handoff");
        assert!(service.next_queue().is_none());
        assert!(service
            .delivery(&recipient, ClientEvent::Hook, 7, false)
            .unwrap()
            .is_null());
        service
            .store
            .finish(
                &attempt.id,
                super::super::types::Handoff::NotSubmitted {
                    reason: "owned pre-write failure".into(),
                },
            )
            .unwrap();
        service.queue_done(&recipient.session, token);
        assert!(
            service.next_queue().is_none(),
            "newer Hook revokes queued work including arrival during handoff"
        );

        service
            .observe_native(&recipient, ClientEvent::Idle, 8)
            .unwrap();
        let (_, token) = service.next_queue().unwrap();
        let attempt = service
            .queue_claim(&recipient, token, ClientEvent::Idle, 9, false)
            .unwrap()
            .unwrap();
        service
            .store
            .finish(
                &attempt.id,
                super::super::types::Handoff::NotSubmitted {
                    reason: "owned pre-write failure".into(),
                },
            )
            .unwrap();
        service.queue_done(&recipient.session, token);
        assert!(
            service.next_queue().is_none(),
            "NotSubmitted completion is not an external wake"
        );
        send(&mut service, "later-legitimate-arrival");
        let (_, arrival_token) = service.next_queue().unwrap();
        assert!(
            service
                .queue_claim(&recipient, arrival_token, ClientEvent::Idle, 10, false)
                .unwrap()
                .is_some(),
            "a later idle arrival can use its separately reserved epoch"
        );
    }

    #[test]
    fn opt_out_revokes_a_pending_queue_wake_before_native_io() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "before-stop".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "pending".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        service
            .observe_native(&recipient, ClientEvent::Idle, 1)
            .unwrap();
        service
            .handle(&integration, Operation::SetState { eligible: false })
            .unwrap();
        assert!(service.next_queue().is_none());
    }

    #[test]
    fn arrival_after_empty_stop_wakes_once_without_replaying_a_stale_stop() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .observe_native(&recipient, ClientEvent::Idle, 1)
            .unwrap();
        assert!(service.next_queue().is_none());
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "after-empty-stop".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "new arrival".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        let (_, token) = service.next_queue().unwrap();
        assert!(service.next_queue().is_none());
        service.queue_done(&recipient.session, token);
        service
            .observe_native(&recipient, ClientEvent::Idle, 1)
            .unwrap();
        assert!(service.next_queue().is_none());
    }

    #[test]
    fn newer_stop_revokes_an_older_queue_owner_token() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "before-two-stops".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "pending".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        service
            .observe_native(&recipient, ClientEvent::Idle, 1)
            .unwrap();
        let (_, old_token) = service.next_queue().unwrap();
        service
            .observe_native(&recipient, ClientEvent::Idle, 2)
            .unwrap();
        assert!(service
            .queue_claim(&recipient, old_token, ClientEvent::Idle, 3, false)
            .unwrap()
            .is_none());
        let (_, new_token) = service.next_queue().unwrap();
        assert_ne!(old_token, new_token);
    }

    #[test]
    fn native_stop_records_a_wake_without_claiming_or_model_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "before-stop".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "pending".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        let observe = Operation::Observe {
            event: ClientEvent::Idle,
            epoch: 1,
        };
        assert!(service
            .handle(
                &VerifiedContext::Participant(recipient.clone()),
                observe.clone()
            )
            .is_err());
        assert!(service.handle(&integration, observe).unwrap().is_null());
        assert!(service.next_queue().is_some());
        let db = rusqlite::Connection::open(dir.path().join("chat.sqlite3")).unwrap();
        let attempts: i64 = db
            .query_row("SELECT COUNT(*) FROM delivery_attempts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(attempts, 0);
    }

    #[test]
    fn new_arrival_during_handoff_wakes_after_completion_without_replaying_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        let send = |service: &mut Service, key: &str| {
            service
                .handle(
                    &VerifiedContext::Observer,
                    Operation::Send {
                        draft: Draft {
                            key: key.into(),
                            project: PROJECT.into(),
                            to: vec![Target::Agent(recipient.session.clone())],
                            body: key.into(),
                            reply_to: None,
                        },
                    },
                )
                .unwrap();
        };
        send(&mut service, "first");
        service
            .observe_native(&recipient, ClientEvent::Idle, 1)
            .unwrap();
        let (_, token) = service.next_queue().unwrap();
        let attempt = service
            .queue_claim(&recipient, token, ClientEvent::Idle, 2, false)
            .unwrap()
            .unwrap();
        send(&mut service, "second");
        service
            .store
            .finish(
                &attempt.id,
                super::super::types::Handoff::Accepted {
                    receipt: "native receipt".into(),
                },
            )
            .unwrap();
        service.queue_done(&recipient.session, token);
        let (_, second_token) = service
            .next_queue()
            .expect("external arrival must wake after completion");
        assert_ne!(token, second_token);
        assert!(service
            .queue_claim(&recipient, second_token, ClientEvent::Idle, 3, false)
            .unwrap()
            .is_some());
        service.queue_done(&recipient.session, second_token);
        assert!(
            service.next_queue().is_none(),
            "completion alone cannot rearm"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lost_queue_finish_expires_to_unknown_without_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut service = Service::open(&path, MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "unknown-on-expiry".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "pending".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        service
            .observe_native(&recipient, ClientEvent::Idle, 1)
            .unwrap();
        let (_, token) = service.next_queue().unwrap();
        let attempt = service
            .queue_claim(&recipient, token, ClientEvent::Idle, 2, false)
            .unwrap()
            .unwrap();
        service.queue_deadlines.insert(attempt.id, Instant::now());
        assert!(service.expire_queues(Instant::now() + Duration::from_millis(1)));
        service.queue_done(&recipient.session, token);
        assert!(service.next_queue().is_none());
        let db = rusqlite::Connection::open(path).unwrap();
        let state: String = db
            .query_row("SELECT state FROM delivery_receipts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "unknown");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn owner_worker_queues_after_stop_and_records_only_the_native_receipt() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = super::super::adapters::BACKEND_SOCKET_TEST_LOCK
            .lock()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.path().join("app-server-control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let read = |ws: &mut tungstenite::WebSocket<UnixStream>| -> Value {
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap()
            };
            let init = read(&mut ws);
            assert_eq!(init["method"], "initialize");
            ws.send(tungstenite::Message::Text(
                json!({"id":init["id"],"result":{}}).to_string().into(),
            ))
            .unwrap();
            assert_eq!(read(&mut ws)["method"], "initialized");
            let state = read(&mut ws);
            assert_eq!(state["method"], "thread/read");
            ws.send(tungstenite::Message::Text(json!({"id":state["id"],"result":{"thread":{"id":state["params"]["threadId"],"status":{"type":"idle"}}}}).to_string().into())).unwrap();
            let queue = read(&mut ws);
            assert_eq!(queue["method"], "thread/queue/add");
            ws.send(tungstenite::Message::Text(
                json!({"id":queue["id"],"result":{"queuedSubmission":{
                    "id":"55555555-5555-4555-8555-555555555555",
                    "clientUserMessageId":queue["params"]["clientUserMessageId"],
                    "input":queue["params"]["input"]
                }}})
                .to_string()
                .into(),
            ))
            .unwrap();
        });
        let process = super::super::adapters::ProcessEvidence::read(std::process::id()).unwrap();
        let mut recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        recipient.native_id = "66666666-6666-4666-8666-666666666666".into();
        recipient.process_start = format!(
            "{}:{}:{}",
            process.boot_id, process.pid, process.start_ticks
        );
        super::super::adapters::install_test_binding(&dir.path().join("bindings"), &recipient)
            .unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut service = Service::open(&path, MACHINE).unwrap();
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "owned-native-stop".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "native queue body".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        service
            .handle(
                &integration,
                Operation::Observe {
                    event: ClientEvent::Idle,
                    epoch: 1,
                },
            )
            .unwrap();
        let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE);
        let changes = Changes::default();
        let owner = thread::spawn(move || run_owner(service, receiver, &changes));
        let started = Instant::now();
        loop {
            let db = rusqlite::Connection::open(&path).unwrap();
            let state: String = db
                .query_row("SELECT state FROM delivery_receipts", [], |row| row.get(0))
                .unwrap();
            if state == "accepted" {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "native queue did not finish: {state}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        drop(sender);
        owner.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn hook_expiry_persistence_failure_retains_exclusion_and_retries_without_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut service = Service::open(&path, MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "owned".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "owned".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Hook,
                    epoch: 1,
                },
            )
            .unwrap();
        let fault = rusqlite::Connection::open(&path).unwrap();
        fault.execute_batch("CREATE TRIGGER reject_hook_timeout BEFORE UPDATE ON delivery_receipts WHEN NEW.state = 'unknown' BEGIN SELECT RAISE(ABORT, 'owned persistence failure'); END;").unwrap();
        let expired = Instant::now() + Duration::from_secs(3);
        assert!(!service.expire_hooks(expired));
        assert_eq!(service.hook_deadlines.len(), 1);
        assert!(service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Idle,
                    epoch: 2
                }
            )
            .unwrap()
            .is_null());
        let state: String = fault
            .query_row("SELECT state FROM delivery_attempts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            state, "submitting",
            "terminal write failure must roll back atomically"
        );
        fault
            .execute_batch("DROP TRIGGER reject_hook_timeout")
            .unwrap();
        drop(fault);
        assert!(
            !service.expire_hooks(expired + Duration::from_millis(50)),
            "retry cadence must remain bounded"
        );
        assert!(service.expire_hooks(expired + Duration::from_millis(100)));
        assert!(service.hook_deadlines.is_empty());
        assert!(!service.expire_hooks(expired + Duration::from_secs(1)));
        assert!(service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Hook,
                    epoch: 3
                }
            )
            .unwrap()
            .is_null());
    }

    #[test]
    fn live_owner_expires_lost_hook_finish_without_replay_or_overtaking() {
        use crate::chat::types::{ClientEvent, Handoff};
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(recipient.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        let send = |key: &str| Operation::Send {
            draft: Draft {
                key: key.into(),
                project: PROJECT.into(),
                to: vec![Target::Agent(recipient.session.clone())],
                body: key.into(),
                reply_to: None,
            },
        };
        service
            .handle(&VerifiedContext::Observer, send("first"))
            .unwrap();
        let first = service
            .handle(
                &integration,
                Operation::Delivery {
                    event: ClientEvent::Hook,
                    epoch: 1,
                },
            )
            .unwrap();
        let attempt = first["id"].as_str().unwrap().to_owned();
        let next = service
            .handle(&VerifiedContext::Observer, send("arrived-during-output"))
            .unwrap();
        for (event, epoch) in [(ClientEvent::Idle, 2), (ClientEvent::Hook, 3)] {
            assert!(service
                .handle(&integration, Operation::Delivery { event, epoch })
                .unwrap()
                .is_null());
        }
        let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE);
        let changes = Arc::new(Changes::default());
        let owner_changes = changes.clone();
        let owner = thread::spawn(move || run_owner(service, receiver, &owner_changes));
        let deadline = Instant::now() + Duration::from_secs(4);
        let recovered = loop {
            let history = dispatch(
                &sender,
                &VerifiedContext::Observer,
                Operation::History {
                    project: PROJECT.into(),
                    cursor: None,
                    limit: 100,
                },
            )
            .unwrap();
            if history["events"].as_array().unwrap().iter().any(|entry| {
                entry["event"]["attempt_id"] == attempt && entry["event"]["state"] == "unknown"
            }) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(20));
        };
        if !recovered {
            drop(sender);
            owner.join().unwrap();
            panic!("live daemon stranded a lost hook Finish in Submitting");
        }
        assert!(dispatch(
            &sender,
            &integration,
            Operation::Finish {
                attempt: attempt.clone(),
                outcome: Handoff::Accepted {
                    receipt: "unsupported late acceptance".into()
                },
            }
        )
        .is_err());
        dispatch(
            &sender,
            &integration,
            Operation::Finish {
                attempt,
                outcome: Handoff::Unknown {
                    reason: "Codex hook output and native acceptance are unconfirmed".into(),
                },
            },
        )
        .unwrap();
        let claimed = dispatch(
            &sender,
            &integration,
            Operation::Delivery {
                event: ClientEvent::Idle,
                epoch: 4,
            },
        )
        .unwrap();
        assert_eq!(claimed["batch"]["full_ids"], json!([next["id"]]));
        dispatch(
            &sender,
            &integration,
            Operation::Finish {
                attempt: claimed["id"].as_str().unwrap().into(),
                outcome: Handoff::Unknown {
                    reason: "owned idle fixture".into(),
                },
            },
        )
        .unwrap();
        assert!(dispatch(
            &sender,
            &integration,
            Operation::Delivery {
                event: ClientEvent::Hook,
                epoch: 5,
            }
        )
        .unwrap()
        .is_null());
        drop(sender);
        owner.join().unwrap();
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
    fn completed_fetch_store_failure_is_visible_and_suppresses_delivery_until_recovered() {
        struct TestBinding(Registration);
        impl BindingValidator for TestBinding {
            fn validate(&self, _: PeerIdentity, _: &str) -> Result<VerifiedContext> {
                Ok(VerifiedContext::Participant(self.0.clone()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut service = Service::open(&path, MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let other = registration("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        service
            .handle(
                &VerifiedContext::Integration(recipient.clone()),
                Operation::Register {},
            )
            .unwrap();
        service
            .handle(
                &VerifiedContext::Integration(other.clone()),
                Operation::Register {},
            )
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "one".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "full body transferred".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "other".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(other.session.clone())],
                        body: "unaffected".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        let injector = rusqlite::Connection::open(&path).unwrap();
        injector.execute_batch("CREATE TRIGGER fail_fetch BEFORE INSERT ON events WHEN NEW.kind = 'fetched' BEGIN SELECT RAISE(ABORT, 'private injected detail'); END;").unwrap();
        let (sender, receiver) = mpsc::sync_channel(WRITER_QUEUE);
        let changes = Arc::new(Changes::default());
        let owner_changes = changes.clone();
        let owner = thread::spawn(move || run_owner(service, receiver, &owner_changes));
        let (mut client, mut server) = UnixStream::pair().unwrap();
        protocol::write_frame(
            &mut client,
            &json!({"version": 1, "binding": "private-fixture"}),
        )
        .unwrap();
        protocol::write_frame(
            &mut client,
            &json!({"version": 1, "operation": {"op": "inbox", "limit": 1}}),
        )
        .unwrap();
        let worker_sender = sender.clone();
        let worker_changes = changes.clone();
        let validator = TestBinding(recipient.clone());
        let worker = thread::spawn(move || {
            serve_connection(
                &mut server,
                false,
                &validator,
                worker_sender,
                &AtomicBool::new(false),
                &worker_changes,
                &Mutex::new(0),
            )
        });
        let response = protocol::read_frame(&mut client).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(
            response["data"]["events"][0]["event"]["message"]["draft"]["body"],
            "full body transferred"
        );
        worker.join().unwrap().unwrap();
        assert!(
            protocol::read_frame(&mut client).is_err(),
            "response must remain one successful frame"
        );
        drop(sender);
        let mut service = owner.join().unwrap();
        let status = owner_request(
            &mut service,
            &changes,
            VerifiedContext::Observer,
            Operation::Status {},
        )
        .unwrap();
        assert_eq!(status["fetch_completions"]["failed"], 1);
        assert!(!status.to_string().contains("private injected detail"));
        assert!(!status.to_string().contains("full body transferred"));
        let before = Instant::now();
        let blocked = owner_request(
            &mut service,
            &changes,
            VerifiedContext::Integration(recipient.clone()),
            Operation::Delivery {
                event: ClientEvent::Hook,
                epoch: 1,
            },
        )
        .unwrap();
        assert!(
            blocked.is_null(),
            "unconfirmed fetch must suppress automatic handoff"
        );
        assert!(before.elapsed() < Duration::from_secs(1));
        assert!(owner_request(
            &mut service,
            &changes,
            VerifiedContext::Integration(other),
            Operation::Delivery {
                event: ClientEvent::Idle,
                epoch: 1
            }
        )
        .unwrap()
        .is_object());
        let events = service
            .store
            .feed(PROJECT, Some(&recipient.session), 0, 100, false)
            .unwrap();
        assert_eq!(
            events.len(),
            1,
            "failed fetch commit must leave only its original message event"
        );
        let retry_at = changes
            .fetches
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .next_retry;
        assert!(
            !repair_fetch(&mut service, &changes, retry_at - Duration::from_nanos(1)),
            "failed completion must honor backoff"
        );
        injector.execute_batch("DROP TRIGGER fail_fetch;").unwrap();
        assert!(repair_fetch(&mut service, &changes, retry_at));
        assert!(!repair_fetch(&mut service, &changes, retry_at));
        assert_eq!(changes.fetch_status()["unconfirmed"], 0);
        let events = service
            .store
            .feed(PROJECT, Some(&recipient.session), 0, 100, false)
            .unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[1].1,
            super::super::types::FeedEvent::Fetched { .. }
        ));
        assert!(service
            .store
            .claim(&recipient.session, super::super::config::DEFAULT_LIMITS)
            .unwrap()
            .is_none());
    }

    fn owner_request(
        service: &mut Service,
        changes: &Changes,
        context: VerifiedContext,
        operation: Operation,
    ) -> Result<Value> {
        let (response, result) = mpsc::sync_channel(1);
        apply_work(
            service,
            Work {
                context,
                operation,
                response,
                deadline: Instant::now() + Duration::from_secs(1),
            },
            changes,
        );
        result.recv().unwrap()
    }

    #[test]
    fn fetch_reservations_bound_capacity_cancel_on_unwind_and_bypass_full_work_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let recipient = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        service
            .handle(
                &VerifiedContext::Integration(recipient.clone()),
                Operation::Register {},
            )
            .unwrap();
        let sent = service
            .handle(
                &VerifiedContext::Observer,
                Operation::Send {
                    draft: Draft {
                        key: "one".into(),
                        project: PROJECT.into(),
                        to: vec![Target::Agent(recipient.session.clone())],
                        body: "hello".into(),
                        reply_to: None,
                    },
                },
            )
            .unwrap();
        let ids = vec![sent["id"].as_str().unwrap().to_owned()];
        let changes = Arc::new(Changes::default());
        let slots: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| {
                changes
                    .reserve_fetch(&recipient.session, ids.clone())
                    .unwrap()
            })
            .collect();
        assert!(changes
            .reserve_fetch(&recipient.session, ids.clone())
            .is_err());
        assert!(
            !repair_fetch(&mut service, &changes, Instant::now()),
            "reserved transfer is not a completed fetch"
        );
        assert!(owner_request(
            &mut service,
            &changes,
            VerifiedContext::Integration(recipient.clone()),
            Operation::Delivery {
                event: ClientEvent::Hook,
                epoch: 1
            }
        )
        .unwrap()
        .is_null());
        drop(slots);
        assert!(!changes.fetch_pending(&recipient.session));
        let worker_changes = changes.clone();
        let worker_session = recipient.session.clone();
        let worker_ids = ids.clone();
        assert!(thread::spawn(move || {
            let _unused = worker_changes
                .reserve_fetch(&worker_session, worker_ids)
                .unwrap();
            panic!("fixture writer crashed before transfer");
        })
        .join()
        .is_err());
        assert_eq!(changes.fetch_status()["unconfirmed"], 0);
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (response, _reply) = mpsc::sync_channel(1);
        sender
            .try_send(Work {
                context: VerifiedContext::Observer,
                operation: Operation::Status {},
                response,
                deadline: Instant::now() + Duration::from_secs(1),
            })
            .unwrap();
        assert!(dispatch(&sender, &VerifiedContext::Observer, Operation::Status {}).is_err());
        changes
            .reserve_fetch(&recipient.session, ids.clone())
            .unwrap()
            .complete();
        changes
            .reserve_fetch(&recipient.session, ids)
            .unwrap()
            .complete();
        assert!(repair_fetch(&mut service, &changes, Instant::now()));
        assert!(changes.fetch_pending(&recipient.session));
        assert!(repair_fetch(&mut service, &changes, Instant::now()));
        assert!(!changes.fetch_pending(&recipient.session));
        let events = service.store.feed(PROJECT, None, 0, 100, false).unwrap();
        assert_eq!(
            events.len(),
            2,
            "duplicate completion produces one Fetched event"
        );
        assert!(service
            .store
            .claim(&recipient.session, super::super::config::DEFAULT_LIMITS)
            .unwrap()
            .is_none());
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
            let owner = thread::spawn(move || run_owner(service, receiver, &owner_changes));
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
    fn lifecycle_query_is_exact_integration_only_and_never_revives_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = Service::open(&dir.path().join("chat.sqlite3"), MACHINE).unwrap();
        let registration = registration("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        let integration = VerifiedContext::Integration(registration.clone());
        service
            .handle(&integration, Operation::Register {})
            .unwrap();
        let live = service
            .handle(&integration, Operation::Lifecycle {})
            .unwrap();
        assert_eq!(
            live,
            json!({"session": registration.session, "ended": false})
        );
        assert!(service
            .handle(&VerifiedContext::Observer, Operation::Lifecycle {})
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Participant(registration.clone()),
                Operation::Lifecycle {}
            )
            .is_err());
        let mut foreign = registration.clone();
        foreign.project = "33333333-3333-4333-8333-333333333333".into();
        assert!(service
            .handle(
                &VerifiedContext::Integration(foreign),
                Operation::Lifecycle {}
            )
            .is_err());
        let mut unknown = registration.clone();
        unknown.session.incarnation = uuid::Uuid::new_v4().to_string();
        assert!(service
            .handle(
                &VerifiedContext::Integration(unknown),
                Operation::Lifecycle {}
            )
            .is_err());
        service.handle(&integration, Operation::End {}).unwrap();
        assert_eq!(
            service
                .handle(&integration, Operation::Lifecycle {})
                .unwrap(),
            json!({"session": registration.session, "ended": true})
        );
        assert!(service
            .handle(&integration, Operation::Register {})
            .is_err());
        assert!(service
            .handle(
                &VerifiedContext::Participant(registration),
                Operation::Status {}
            )
            .is_err());
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
