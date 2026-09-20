use anyhow::{bail, ensure, Context, Result};

use super::{
    store::Store,
    types::{Actor, Message, SessionRef, Target},
};

pub use super::types::{Registration, SessionState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientKind {
    Claude,
    Codex,
    Pi,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }
}

/// Candidate identifiers only. Selecting one does not validate it or authorize
/// registration. The adapter must validate its native session and live process.
pub struct NativeIdentitySources<'a> {
    pub codex_thread_id: Option<&'a str>,
    pub claude_session_id: Option<&'a str>,
    pub pi_session_id: Option<&'a str>,
}

impl<'a> NativeIdentitySources<'a> {
    pub fn select(&self, client: ClientKind) -> Result<&'a str> {
        match client {
            ClientKind::Claude => self.claude_session_id,
            ClientKind::Codex => self.codex_thread_id,
            ClientKind::Pi => self.pi_session_id,
        }
        .filter(|id| !id.trim().is_empty())
        .with_context(|| format!("missing {} native session identity", client.as_str()))
    }
}

/// Adapter-validated evidence, not an IPC input type. `process_start` identifies
/// one process lifetime, including boot identity, PID and birth evidence where
/// available. A PID or socket path alone must never be passed as this evidence.
#[derive(Clone, Debug)]
pub struct NativeEvidence {
    pub client: ClientKind,
    pub native_id: String,
    pub process_start: String,
}

/// A transient view borrowing the daemon's sole Store. Construction is fallible;
/// snapshots use loaded state and cannot suppress a database read error. Drop the
/// view before sending through Store, then construct another view when needed.
pub struct Registry<'a> {
    store: &'a mut Store,
    machine: String,
    entries: Vec<SessionState>,
}

impl<'a> Registry<'a> {
    pub fn new(store: &'a mut Store) -> Result<Self> {
        let machine = store.machine()?;
        let mut entries = store.load_sessions()?;
        entries.extend(store.load_remote_sessions()?);
        Ok(Self {
            store,
            machine,
            entries,
        })
    }

    /// Called only for trusted native startup. A live reconnect cannot undo
    /// opt-out; startup after explicit End creates a separate incarnation.
    pub fn connect(
        &mut self,
        project: &str,
        names: crate::name::Sources,
        evidence: NativeEvidence,
        eligible: bool,
    ) -> Result<Registration> {
        let name = crate::name::pick(names)?;
        let existing = self.entries.iter().find(|entry| {
            let registration = &entry.registration;
            !entry.ended
                && registration.session.machine == self.machine
                && registration.client == evidence.client.as_str()
                && registration.native_id == evidence.native_id
                && registration.process_start == evidence.process_start
        });
        let registration = Registration {
            session: existing
                .map(|entry| entry.registration.session.clone())
                .unwrap_or_else(|| SessionRef {
                    machine: self.machine.clone(),
                    incarnation: uuid::Uuid::new_v4().to_string(),
                }),
            project: project.into(),
            name,
            client: evidence.client.as_str().into(),
            native_id: evidence.native_id,
            process_start: evidence.process_start,
            eligible: existing.map_or(eligible, |entry| entry.registration.eligible),
        };
        self.register(registration.clone())?;
        Ok(registration)
    }

    pub fn register(&mut self, registration: Registration) -> Result<()> {
        ensure!(
            registration.session.machine == self.machine,
            "registration is not local to this store"
        );
        for (label, value) in [
            ("incarnation", &registration.session.incarnation),
            ("project", &registration.project),
            ("name", &registration.name),
            ("native identity", &registration.native_id),
            ("process lifetime evidence", &registration.process_start),
        ] {
            ensure!(
                !value.trim().is_empty(),
                "registration {label} cannot be empty"
            );
        }
        ensure!(
            matches!(registration.client.as_str(), "claude" | "codex" | "pi"),
            "unsupported Chat client"
        );
        if let Some(current) = self.state(&registration.session) {
            ensure!(!current.ended, "session incarnation has ended");
            let old = &current.registration;
            ensure!(
                old.client == registration.client
                    && old.native_id == registration.native_id
                    && old.process_start == registration.process_start,
                "cannot rebind session incarnation to different native process evidence"
            );
            ensure!(
                old.project == registration.project,
                "cannot change an incarnation's project"
            );
            ensure!(
                old.eligible == registration.eligible,
                "change delivery eligibility explicitly"
            );
        }
        self.persist(SessionState {
            registration,
            connected: true,
            ended: false,
        })
    }

    pub fn state(&self, session: &SessionRef) -> Option<&SessionState> {
        self.entries
            .iter()
            .find(|entry| entry.registration.session == *session)
    }

    pub fn snapshot(&self, project: &str) -> Vec<Registration> {
        self.entries
            .iter()
            .filter(|entry| !entry.ended && entry.registration.project == project)
            .map(|entry| entry.registration.clone())
            .collect()
    }

    pub fn disconnect(&mut self, session: &SessionRef) -> Result<()> {
        let mut entry = self
            .state(session)
            .context("unknown session incarnation")?
            .clone();
        entry.connected = false;
        self.persist(entry)
    }

    pub fn end(&mut self, session: &SessionRef) -> Result<()> {
        let mut entry = self
            .state(session)
            .context("unknown session incarnation")?
            .clone();
        entry.connected = false;
        entry.ended = true;
        entry.registration.eligible = false;
        self.persist(entry)
    }

    pub fn set_eligible(&mut self, session: &SessionRef, eligible: bool) -> Result<()> {
        let mut entry = self
            .state(session)
            .context("unknown session incarnation")?
            .clone();
        ensure!(!entry.ended, "session incarnation has ended");
        entry.registration.eligible = eligible;
        self.persist(entry)
    }

    fn persist(&mut self, entry: SessionState) -> Result<()> {
        self.store.save_session(&entry)?;
        if let Some(current) = self
            .entries
            .iter_mut()
            .find(|current| current.registration.session == entry.registration.session)
        {
            *current = entry;
        } else {
            self.entries.push(entry);
        }
        Ok(())
    }

    /// The trusted transport supplies peer roster freshness. Disconnection does
    /// not prove an incarnation ended; it remains a pinned, queueable recipient.
    pub fn resolve_recipients(
        &self,
        project: &str,
        sender: &Actor,
        addresses: &[&str],
        roster_stale: bool,
        allow_stale_roster: bool,
    ) -> Result<Vec<Target>> {
        ensure!(!addresses.is_empty(), "select at least one recipient");
        let candidates = self.snapshot(project);
        let self_target = actor_target(sender);
        let mut targets = Vec::new();
        for address in addresses {
            if *address == "@all" {
                ensure!(
                    !roster_stale || allow_stale_roster,
                    "project roster is stale; use --allow-stale-roster to broadcast"
                );
                for entry in &candidates {
                    let target = Target::Agent(entry.session.clone());
                    if entry.eligible && target != self_target {
                        push_unique(&mut targets, target);
                    }
                }
            } else {
                push_unique(
                    &mut targets,
                    Target::Agent(resolve_exact(address, &candidates)?),
                );
            }
        }
        ensure!(!targets.is_empty(), "no eligible recipients selected");
        Ok(targets)
    }
}

pub fn reply_targets(message: &Message, sender: &Actor, all: bool) -> Vec<Target> {
    let self_target = actor_target(sender);
    let mut targets = vec![actor_target(&message.sender)];
    if all {
        for target in &message.draft.to {
            push_unique(&mut targets, target.clone());
        }
    }
    targets.retain(|target| *target != self_target);
    targets
}

fn actor_target(actor: &Actor) -> Target {
    match actor {
        Actor::Agent(session) => Target::Agent(session.clone()),
        Actor::Human { machine } => Target::Human {
            machine: machine.clone(),
        },
    }
}

fn push_unique(targets: &mut Vec<Target>, target: Target) {
    if !targets.contains(&target) {
        targets.push(target);
    }
}

pub fn resolve_exact(address: &str, candidates: &[Registration]) -> Result<SessionRef> {
    let matches: Vec<_> = candidates
        .iter()
        .filter(|entry| {
            entry.eligible
                && (entry.name == address
                    || format!("{}/{}", entry.session.machine, entry.session.incarnation)
                        == address)
        })
        .collect();
    match matches.as_slice() {
        [entry] => Ok(entry.session.clone()),
        [] => bail!("no eligible recipient: {address}"),
        _ => bail!("ambiguous recipient: {address}; select a machine/session ID"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{
        store::Store,
        types::{Actor, Draft, Target},
    };
    use crate::name::Sources;

    fn name(value: &str) -> Sources {
        Sources {
            explicit: Some(value.into()),
            lam_name: None,
            multiplexer: None,
        }
    }

    fn evidence(start: &str) -> NativeEvidence {
        NativeEvidence {
            client: ClientKind::Codex,
            native_id: "conversation".into(),
            process_start: start.into(),
        }
    }

    fn store(path: &std::path::Path) -> Store {
        let mut store = Store::open(path).unwrap();
        store.set_machine("pc").unwrap();
        store
    }

    #[test]
    fn reconnect_reuses_only_matching_live_process_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut store = store(&path);
        let mut registry = Registry::new(&mut store).unwrap();
        let first = registry
            .connect("lam", name("pm"), evidence("boot:42:100"), true)
            .unwrap();
        uuid::Uuid::parse_str(&first.session.incarnation).unwrap();
        registry.disconnect(&first.session).unwrap();
        assert!(!registry.state(&first.session).unwrap().connected);
        assert!(!registry.state(&first.session).unwrap().ended);
        assert_eq!(registry.snapshot("lam"), vec![first.clone()]);
        drop(registry);
        drop(store);

        let mut store = super::tests::store(&path);
        let mut registry = Registry::new(&mut store).unwrap();
        assert!(!registry.state(&first.session).unwrap().connected);
        let reconnected = registry
            .connect("lam", name("renamed"), evidence("boot:42:100"), true)
            .unwrap();
        assert_eq!(first.session, reconnected.session);
        assert!(registry.state(&first.session).unwrap().connected);
        // Same native conversation and reused PID/socket do not establish process lifetime.
        let restarted = registry
            .connect("lam", name("pm"), evidence("boot:42:101"), true)
            .unwrap();
        assert_ne!(first.session, restarted.session);
        let mut rebound = first.clone();
        rebound.process_start = "boot:42:101".into();
        assert!(registry.register(rebound).is_err());
    }

    #[test]
    fn remote_presence_cannot_rebind_local_native_identity() {
        use crate::chat::types::{PeerEvent, PeerPayload};
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir.path().join("chat.sqlite3"));
        let peer = "22222222-2222-4222-8222-222222222222";
        let project = "33333333-3333-4333-8333-333333333333";
        let remote = Registration {
            session: SessionRef {
                machine: peer.into(),
                incarnation: uuid::Uuid::new_v4().to_string(),
            },
            project: project.into(),
            name: "remote".into(),
            client: "codex".into(),
            native_id: "conversation".into(),
            process_start: "boot:42:100".into(),
            eligible: true,
        };
        store
            .import_event(
                peer,
                &PeerEvent {
                    id: uuid::Uuid::new_v4().to_string(),
                    origin: peer.into(),
                    seq: 1,
                    project: project.into(),
                    payload: PeerPayload::Presence {
                        session: SessionState {
                            registration: remote.clone(),
                            connected: true,
                            ended: false,
                        },
                        epoch: 1,
                    },
                },
                &[project.into()],
            )
            .unwrap();
        let local = Registry::new(&mut store)
            .unwrap()
            .connect(project, name("local"), evidence("boot:42:100"), true)
            .unwrap();
        assert_eq!(local.session.machine, "pc");
        assert_ne!(local.session.incarnation, remote.session.incarnation);
    }

    #[test]
    fn ended_and_opted_out_sessions_stay_distinct_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut store = store(&path);
        let mut registry = Registry::new(&mut store).unwrap();
        let ended = registry
            .connect("lam", name("old"), evidence("boot:1:1"), true)
            .unwrap();
        let opted_out = registry
            .connect("lam", name("off"), evidence("boot:2:2"), true)
            .unwrap();
        registry.end(&ended.session).unwrap();
        registry.set_eligible(&opted_out.session, false).unwrap();
        drop(registry);
        drop(store);

        let mut store = super::tests::store(&path);
        let mut registry = Registry::new(&mut store).unwrap();
        assert!(registry.state(&ended.session).unwrap().ended);
        assert!(!registry.state(&ended.session).unwrap().connected);
        let replacement = registry
            .connect("lam", name("old"), evidence("boot:1:1"), true)
            .unwrap();
        assert_ne!(replacement.session, ended.session);
        assert!(replacement.eligible);
        assert!(registry.state(&ended.session).unwrap().ended);
        assert_eq!(
            registry
                .connect("lam", name("old"), evidence("boot:1:1"), true)
                .unwrap()
                .session,
            replacement.session
        );
        assert!(registry.register(ended.clone()).is_err());
        assert!(registry.set_eligible(&ended.session, true).is_err());
        let reconnected = registry
            .connect("lam", name("off"), evidence("boot:2:2"), true)
            .unwrap();
        assert!(!reconnected.eligible);
        assert!(!registry.state(&opted_out.session).unwrap().ended);
        assert!(resolve_exact("off", &registry.snapshot("lam")).is_err());
        registry.set_eligible(&opted_out.session, true).unwrap();
        assert_eq!(
            resolve_exact("off", &registry.snapshot("lam")).unwrap(),
            opted_out.session
        );
    }

    #[test]
    fn selected_client_never_falls_back_to_inherited_parent_identity() {
        let sources = NativeIdentitySources {
            codex_thread_id: Some("parent-codex"),
            claude_session_id: None,
            pi_session_id: Some("child-pi"),
        };
        assert_eq!(sources.select(ClientKind::Pi).unwrap(), "child-pi");
        assert_eq!(sources.select(ClientKind::Codex).unwrap(), "parent-codex");
        assert!(sources.select(ClientKind::Claude).is_err());
        let inherited_only = NativeIdentitySources {
            pi_session_id: None,
            ..sources
        };
        assert!(inherited_only.select(ClientKind::Pi).is_err());
    }

    #[test]
    fn missing_name_leaves_no_registration_and_existing_precedence_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir.path().join("chat.sqlite3"));
        let mut registry = Registry::new(&mut store).unwrap();
        let missing = Sources {
            explicit: None,
            lam_name: None,
            multiplexer: None,
        };
        let error = registry
            .connect("lam", missing, evidence("boot:1:1"), true)
            .unwrap_err();
        assert!(error.to_string().contains("--name"));
        assert!(registry.snapshot("lam").is_empty());
        for (explicit, lam_name, expected) in [
            (Some("explicit"), Some("env"), "explicit"),
            (None, Some("env"), "env"),
            (None, None, "mux"),
        ] {
            let sources = Sources {
                explicit: explicit.map(String::from),
                lam_name: lam_name.map(String::from),
                multiplexer: Some("mux".into()),
            };
            assert_eq!(
                registry
                    .connect("lam", sources, evidence(expected), true)
                    .unwrap()
                    .name,
                expected
            );
        }
    }

    #[test]
    fn broadcast_freezes_scoped_nonended_eligible_roster_and_requires_stale_acknowledgement() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir.path().join("chat.sqlite3"));
        let mut registry = Registry::new(&mut store).unwrap();
        let sender = registry
            .connect("lam", name("sender"), evidence("1"), true)
            .unwrap();
        let recipient = registry
            .connect("lam", name("dev"), evidence("2"), true)
            .unwrap();
        registry.disconnect(&recipient.session).unwrap();
        registry
            .connect("other", name("outsider"), evidence("3"), true)
            .unwrap();
        registry
            .connect("lam", name("off"), evidence("4"), false)
            .unwrap();
        let ended = registry
            .connect("lam", name("ended"), evidence("5"), true)
            .unwrap();
        registry.end(&ended.session).unwrap();
        let actor = Actor::Agent(sender.session);
        assert!(registry
            .resolve_recipients("lam", &actor, &["@all"], true, false)
            .is_err());
        let targets = registry
            .resolve_recipients("lam", &actor, &["@all", "dev"], true, true)
            .unwrap();
        assert_eq!(targets, vec![Target::Agent(recipient.session.clone())]);
        registry
            .connect("lam", name("late"), evidence("6"), true)
            .unwrap();
        assert!(registry
            .resolve_recipients("lam", &actor, &[], false, false)
            .is_err());
        assert!(registry
            .resolve_recipients("lam", &actor, &["outsider"], false, false)
            .is_err());
        assert!(registry
            .resolve_recipients("lam", &actor, &["dev"], true, false)
            .is_ok());
        drop(registry);
        let message = store
            .send(
                &actor,
                &Draft {
                    key: "frozen".into(),
                    project: "lam".into(),
                    to: targets,
                    body: "hello".into(),
                    reply_to: None,
                },
            )
            .unwrap();
        assert_eq!(message.draft.to, vec![Target::Agent(recipient.session)]);
    }

    #[test]
    fn replies_use_original_ids_and_agent_names_cannot_change_origin() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir.path().join("chat.sqlite3"));
        let mut registry = Registry::new(&mut store).unwrap();
        let sender = registry
            .connect("lam", name("Human"), evidence("1"), true)
            .unwrap();
        let recipient = registry
            .connect("lam", name("dev"), evidence("2"), true)
            .unwrap();
        let third = registry
            .connect("lam", name("reviewer"), evidence("3"), true)
            .unwrap();
        let actor = Actor::Agent(sender.session.clone());
        drop(registry);
        let message = store
            .send(
                &actor,
                &Draft {
                    key: "one".into(),
                    project: "lam".into(),
                    to: vec![
                        Target::Agent(recipient.session.clone()),
                        Target::Agent(sender.session.clone()),
                        Target::Agent(third.session.clone()),
                    ],
                    body: "hello".into(),
                    reply_to: None,
                },
            )
            .unwrap();
        assert_eq!(message.sender, actor);
        let recipient_actor = Actor::Agent(recipient.session);
        assert_eq!(
            reply_targets(&message, &recipient_actor, false),
            vec![Target::Agent(sender.session.clone())]
        );
        assert_eq!(
            reply_targets(&message, &recipient_actor, true),
            vec![Target::Agent(sender.session), Target::Agent(third.session)]
        );
        let human = Actor::Human {
            machine: "pc".into(),
        };
        let human_message = store
            .send(
                &human,
                &Draft {
                    key: "human".into(),
                    ..message.draft
                },
            )
            .unwrap();
        assert_eq!(
            reply_targets(&human_message, &recipient_actor, false),
            vec![Target::Human {
                machine: "pc".into()
            }]
        );
    }

    #[test]
    fn duplicate_name_cannot_choose_a_recipient() {
        use super::{resolve_exact, Registration};
        use crate::chat::types::SessionRef;
        let make = |machine: &str| Registration {
            session: SessionRef {
                machine: machine.into(),
                incarnation: format!("{machine}-pm"),
            },
            project: "lam".into(),
            name: "pm".into(),
            client: "codex".into(),
            native_id: "native-conversation".into(),
            process_start: machine.into(),
            eligible: true,
        };
        let candidates = vec![make("pc"), make("mac")];
        assert!(resolve_exact("pm", &candidates).is_err());
        assert!(resolve_exact("p", &candidates).is_err());
        assert_eq!(
            resolve_exact("pc/pc-pm", &candidates).unwrap(),
            candidates[0].session
        );
    }
}
