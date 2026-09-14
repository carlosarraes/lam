use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::types::{Actor, Draft, Message, Registration, SessionRef, SessionState, Target};

const SCHEMA_VERSION: u32 = 2;
const MAX_BODY_BYTES: usize = 64 * 1024;

pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens a store whose parent directory was prepared by `chat::config::Paths`.
    ///
    /// This method independently rejects unsafe database leaves and creates new
    /// database files as 0600. The daemon must still discover and validate its
    /// managed directory before calling it.
    pub fn open(path: &Path) -> Result<Self> {
        prepare_database_path(path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut connection = Connection::open_with_flags(path, flags)
            .with_context(|| format!("cannot open Chat database {}", path.display()))?;
        validate_database_file(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;

        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= SCHEMA_VERSION,
            "Chat database schema {version} is newer than supported schema {SCHEMA_VERSION}"
        );
        if version < SCHEMA_VERSION {
            migrate(&mut connection, version)?;
        }
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if mode != "wal" {
            let mode: String =
                connection
                    .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
            ensure!(mode == "wal", "SQLite refused WAL journal mode");
        }

        Ok(Self { connection })
    }

    pub fn set_machine(&mut self, machine: &str) -> Result<()> {
        ensure!(!machine.is_empty(), "machine identity cannot be empty");
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<String> = transaction
            .query_row(
                "SELECT value FROM metadata WHERE key = 'machine'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match current {
            Some(current) if current != machine => {
                bail!("Chat database belongs to machine {current}, not {machine}")
            }
            Some(_) => {}
            None => {
                transaction.execute(
                    "INSERT INTO metadata(key, value) VALUES ('machine', ?1)",
                    [machine],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn machine(&self) -> Result<String> {
        self.connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'machine'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .context("Chat machine identity is not initialized")
    }

    pub(super) fn load_sessions(&self) -> Result<Vec<SessionState>> {
        let mut statement = self.connection.prepare(
            "SELECT machine, incarnation, project, name, client, native_id, process_start,
                    eligible, connected, ended FROM sessions ORDER BY machine, incarnation",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SessionState {
                registration: Registration {
                    session: SessionRef {
                        machine: row.get(0)?,
                        incarnation: row.get(1)?,
                    },
                    project: row.get(2)?,
                    name: row.get(3)?,
                    client: row.get(4)?,
                    native_id: row.get(5)?,
                    process_start: row.get(6)?,
                    eligible: row.get(7)?,
                },
                connected: row.get(8)?,
                ended: row.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(super) fn save_session(&mut self, entry: &SessionState) -> Result<()> {
        let registration = &entry.registration;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO sessions(machine, incarnation, project, name, client, native_id, process_start, eligible, connected, ended)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(machine, incarnation) DO UPDATE SET
                 name = excluded.name, eligible = excluded.eligible,
                 connected = excluded.connected, ended = excluded.ended",
            params![registration.session.machine, registration.session.incarnation, registration.project,
                registration.name, registration.client, registration.native_id, registration.process_start,
                registration.eligible, entry.connected, entry.ended],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn send(&mut self, sender: &Actor, draft: &Draft) -> Result<Message> {
        ensure!(
            draft.body.len() <= MAX_BODY_BYTES,
            "Chat message body exceeds 64 KiB"
        );
        let canonical_draft = canonical_draft(draft)?;
        let sender_key = serde_json::to_string(sender)?;
        let canonical_draft_json = serde_json::to_string(&canonical_draft)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        let machine: String = transaction
            .query_row(
                "SELECT value FROM metadata WHERE key = 'machine'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .context("Chat machine identity is not initialized")?;
        ensure!(
            actor_machine(sender) == machine,
            "sender is not local to this store"
        );

        let existing: Option<String> = transaction
            .query_row(
                "SELECT payload_json FROM messages
                 WHERE sender_key = ?1 AND idempotency_key = ?2",
                params![sender_key, canonical_draft.key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(payload) = existing {
            let message: Message = serde_json::from_str(&payload)
                .context("stored Chat message contains invalid JSON")?;
            let existing_draft_json = serde_json::to_string(&message.draft)?;
            ensure!(
                existing_draft_json == canonical_draft_json,
                "idempotency key was already used for a different Chat message"
            );
            return Ok(message);
        }

        let sender_seq = next_sequence(
            &transaction,
            "SELECT COALESCE(MAX(sender_seq), 0) + 1 FROM messages WHERE sender_key = ?1",
            &sender_key,
        )?;
        let project_seq = next_sequence(
            &transaction,
            "SELECT COALESCE(MAX(project_seq), 0) + 1 FROM events WHERE project = ?1",
            &canonical_draft.project,
        )?;
        let message = Message {
            id: uuid::Uuid::new_v4().to_string(),
            sender: sender.clone(),
            sender_seq,
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            draft: canonical_draft,
        };
        let payload = serde_json::to_string(&message)?;

        transaction.execute(
            "INSERT INTO messages(id, sender_key, sender_seq, idempotency_key, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                sender_key,
                to_sql_integer(message.sender_seq)?,
                message.draft.key,
                payload,
            ],
        )?;

        for target in &message.draft.to {
            let target_key = serde_json::to_string(target)?;
            transaction.execute(
                "INSERT INTO recipients(message_id, target_key) VALUES (?1, ?2)",
                params![message.id, target_key],
            )?;
            if let Target::Agent(session) = target {
                if session.machine == machine {
                    let inbox_seq = next_sequence(
                        &transaction,
                        "SELECT COALESCE(MAX(inbox_seq), 0) + 1 FROM inbox_entries
                         WHERE recipient_key = ?1",
                        &target_key,
                    )?;
                    transaction.execute(
                        "INSERT INTO inbox_entries(recipient_key, inbox_seq, message_id)
                         VALUES (?1, ?2, ?3)",
                        params![target_key, to_sql_integer(inbox_seq)?, message.id],
                    )?;
                    transaction.execute(
                        "INSERT INTO delivery_receipts(message_id, target_key)
                         VALUES (?1, ?2)",
                        params![message.id, target_key],
                    )?;
                    transaction.execute(
                        "INSERT INTO exposures(message_id, target_key) VALUES (?1, ?2)",
                        params![message.id, target_key],
                    )?;
                }
            }
        }

        transaction.execute(
            "INSERT INTO events(event_id, project, project_seq, payload_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                uuid::Uuid::new_v4().to_string(),
                message.draft.project,
                to_sql_integer(project_seq)?,
                payload,
            ],
        )?;
        transaction.commit()?;
        Ok(message)
    }
}

fn migrate(connection: &mut Connection, version: u32) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if version == 0 {
        transaction.execute_batch(include_str!("schema.sql"))?;
    }
    if version < 2 {
        transaction.execute_batch(include_str!("migration-002.sql"))?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn canonical_draft(draft: &Draft) -> Result<Draft> {
    let mut recipients = BTreeMap::new();
    for target in &draft.to {
        recipients.insert(serde_json::to_string(target)?, target.clone());
    }
    Ok(Draft {
        key: draft.key.clone(),
        project: draft.project.clone(),
        to: recipients.into_values().collect(),
        body: draft.body.clone(),
        reply_to: draft.reply_to.clone(),
    })
}

fn actor_machine(actor: &Actor) -> &str {
    match actor {
        Actor::Agent(session) => &session.machine,
        Actor::Human { machine } => machine,
    }
}

fn next_sequence(transaction: &rusqlite::Transaction<'_>, query: &str, scope: &str) -> Result<u64> {
    let value: i64 = transaction.query_row(query, [scope], |row| row.get(0))?;
    u64::try_from(value).context("Chat sequence is outside the supported range")
}

fn to_sql_integer(value: u64) -> Result<i64> {
    i64::try_from(value).context("Chat sequence is outside SQLite's integer range")
}

#[cfg(unix)]
fn prepare_database_path(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    match path.symlink_metadata() {
        Ok(_) => validate_database_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
                .with_context(|| format!("cannot create Chat database {}", path.display()))?;
            validate_database_file(path)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn validate_database_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = path.symlink_metadata()?;
    ensure!(
        metadata.file_type().is_file(),
        "Chat database must be a regular file"
    );
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "Chat database has another owner"
    );
    ensure!(
        metadata.mode() & 0o777 == 0o600,
        "Chat database permissions must be 0600"
    );
    Ok(())
}

#[cfg(not(unix))]
fn prepare_database_path(path: &Path) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| !metadata.is_file())
    {
        bail!("Chat database must be a regular file");
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_database_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use rusqlite::Connection;

    use super::Store;
    use crate::chat::types::{Actor, Draft, SessionRef, Target};

    #[test]
    fn v1_upgrade_preserves_messages_and_initializes_registry() {
        use crate::chat::registry::Registry;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!("schema.sql"))
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        let mut legacy = Store { connection };
        legacy.set_machine("pc").unwrap();
        let original = legacy.send(&sender(), &draft("before-upgrade")).unwrap();
        drop(legacy);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut upgraded = Store::open(&path).unwrap();
        let retry = upgraded.send(&sender(), &draft("before-upgrade")).unwrap();
        assert_eq!(retry.id, original.id);
        assert_eq!(retry.created_at, original.created_at);
        assert_eq!(retry.sender_seq, original.sender_seq);
        assert!(Registry::new(&mut upgraded)
            .unwrap()
            .snapshot("lam")
            .is_empty());
    }

    #[test]
    fn registry_database_errors_propagate_without_changing_cached_state() {
        use crate::chat::registry::{ClientKind, NativeEvidence, Registry};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut store = Store::open(&path).unwrap();
        store.set_machine("pc").unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_session_update BEFORE UPDATE ON sessions
             BEGIN SELECT RAISE(ABORT, 'session update rejected'); END;",
            )
            .unwrap();
        let mut registry = Registry::new(&mut store).unwrap();
        let registration = registry
            .connect(
                "lam",
                crate::name::Sources {
                    explicit: Some("pm".into()),
                    lam_name: None,
                    multiplexer: None,
                },
                NativeEvidence {
                    client: ClientKind::Codex,
                    native_id: "native".into(),
                    process_start: "boot:1:2".into(),
                },
                true,
            )
            .unwrap();
        assert!(registry.disconnect(&registration.session).is_err());
        assert!(registry.state(&registration.session).unwrap().connected);
        drop(registry);
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(
            Registry::new(&mut store)
                .unwrap()
                .state(&registration.session)
                .unwrap()
                .connected
        );
        store
            .connection
            .execute_batch("DROP TABLE sessions;")
            .unwrap();
        assert!(Registry::new(&mut store).is_err());
    }

    fn sender() -> Actor {
        Actor::Agent(SessionRef {
            machine: "pc".into(),
            incarnation: "pm-1".into(),
        })
    }

    fn draft(key: &str) -> Draft {
        Draft {
            key: key.into(),
            project: "lam".into(),
            to: vec![Target::Agent(SessionRef {
                machine: "pc".into(),
                incarnation: "dev-1".into(),
            })],
            body: "Pause after your current tool".into(),
            reply_to: None,
        }
    }

    #[test]
    fn retry_returns_one_immutable_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let sender = sender();
        let mut draft = draft("retry-key");
        let first = store.send(&sender, &draft).unwrap();
        let retried = store.send(&sender, &draft).unwrap();
        uuid::Uuid::parse_str(&first.id).unwrap();
        assert_eq!(first.id, retried.id);
        assert_eq!(first.created_at, retried.created_at);
        assert!(first.created_at.ends_with('Z'));
        DateTime::parse_from_rfc3339(&first.created_at).unwrap();
        draft.body = "Different request".into();
        assert!(store.send(&sender, &draft).is_err());
        draft.key = "intentional-new-send".into();
        assert_ne!(first.id, store.send(&sender, &draft).unwrap().id);
    }

    #[test]
    fn retry_survives_reopen_with_original_sequence_and_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let sender = sender();
        let draft = draft("durable-key");
        let first = {
            let mut store = Store::open(&path).unwrap();
            store.set_machine("pc").unwrap();
            store.send(&sender, &draft).unwrap()
        };
        let retried = {
            let mut store = Store::open(&path).unwrap();
            store.set_machine("pc").unwrap();
            store.send(&sender, &draft).unwrap()
        };

        assert_eq!(retried.id, first.id);
        assert_eq!(retried.sender_seq, 1);
        assert_eq!(retried.created_at, first.created_at);
    }

    #[test]
    fn failure_before_event_commit_rolls_back_every_row_and_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_event BEFORE INSERT ON events
                 BEGIN SELECT RAISE(ABORT, 'injected event failure'); END;",
            )
            .unwrap();

        assert!(store.send(&sender(), &draft("rolled-back")).is_err());
        for table in [
            "messages",
            "recipients",
            "inbox_entries",
            "delivery_receipts",
            "exposures",
            "events",
        ] {
            let count: i64 = store
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} retained a rolled-back row");
        }
        store
            .connection
            .execute_batch("DROP TRIGGER fail_event")
            .unwrap();
        assert_eq!(
            store
                .send(&sender(), &draft("after-rollback"))
                .unwrap()
                .sender_seq,
            1
        );
    }

    #[test]
    fn recipients_are_frozen_as_a_deduplicated_canonical_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let recipient = Target::Agent(SessionRef {
            machine: "pc".into(),
            incarnation: "dev-1".into(),
        });
        let mut duplicated = draft("same-set");
        duplicated.to = vec![recipient.clone(), recipient.clone()];
        let first = store.send(&sender(), &duplicated).unwrap();
        assert_eq!(first.draft.to.len(), 1);

        let canonical = draft("same-set");
        assert_eq!(store.send(&sender(), &canonical).unwrap().id, first.id);
        let recipient_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM recipients", [], |row| row.get(0))
            .unwrap();
        let inbox_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM inbox_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!((recipient_count, inbox_count), (1, 1));
        let receipt: String = store
            .connection
            .query_row("SELECT state FROM delivery_receipts", [], |row| row.get(0))
            .unwrap();
        let exposure: String = store
            .connection
            .query_row("SELECT state FROM exposures", [], |row| row.get(0))
            .unwrap();
        assert_eq!((receipt.as_str(), exposure.as_str()), ("queued", "unseen"));
    }

    #[test]
    fn recipient_order_does_not_change_idempotency_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let mut first = draft("ordered-set");
        first.to.push(Target::Human {
            machine: "pc".into(),
        });
        let first = store.send(&sender(), &first).unwrap();
        let mut reversed = draft("ordered-set");
        reversed.to.insert(
            0,
            Target::Human {
                machine: "pc".into(),
            },
        );

        assert_eq!(store.send(&sender(), &reversed).unwrap().id, first.id);
    }

    #[test]
    fn reused_key_rejects_a_changed_recipient_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let mut changed = draft("recipient-change");
        store.send(&sender(), &changed).unwrap();
        changed.to.push(Target::Human {
            machine: "pc".into(),
        });

        assert!(store.send(&sender(), &changed).is_err());
    }

    #[test]
    fn a_new_key_with_identical_content_creates_a_new_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let first = store.send(&sender(), &draft("first-key")).unwrap();
        let second = store.send(&sender(), &draft("second-key")).unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!((first.sender_seq, second.sender_seq), (1, 2));
    }

    #[test]
    fn body_bound_accepts_64_kib_and_rejects_one_more_byte() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let mut at_limit = draft("at-limit");
        at_limit.body = "x".repeat(64 * 1024);
        store.send(&sender(), &at_limit).unwrap();
        let mut over_limit = draft("over-limit");
        over_limit.body = "x".repeat(64 * 1024 + 1);

        assert!(store.send(&sender(), &over_limit).is_err());
    }

    #[test]
    fn human_targets_are_feed_records_without_agent_inbox_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let mut human = draft("human-target");
        human.to = vec![Target::Human {
            machine: "pc".into(),
        }];
        store.send(&sender(), &human).unwrap();

        let inbox_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM inbox_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(inbox_count, 0);
    }

    #[test]
    fn remote_agent_targets_do_not_create_local_inbox_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let mut remote = draft("remote-target");
        remote.to = vec![Target::Agent(SessionRef {
            machine: "mac".into(),
            incarnation: "dev-remote".into(),
        })];
        store.send(&sender(), &remote).unwrap();

        let inbox_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM inbox_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(inbox_count, 0);
    }

    #[test]
    fn origin_sequences_are_contiguous_per_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        store.send(&sender(), &draft("lam-1")).unwrap();
        let mut other = draft("other-1");
        other.project = "other".into();
        store.send(&sender(), &other).unwrap();
        store.send(&sender(), &draft("lam-2")).unwrap();

        let sequences: Vec<(String, i64)> = store
            .connection
            .prepare("SELECT project, project_seq FROM events ORDER BY seq")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            sequences,
            vec![("lam".into(), 1), ("other".into(), 1), ("lam".into(), 2)]
        );
    }

    #[test]
    fn changing_the_initialized_machine_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        store.set_machine("pc").unwrap();
        assert!(store.set_machine("mac").is_err());
    }

    #[test]
    fn open_refuses_a_newer_schema_without_rewriting_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "user_version", super::SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let error = Store::open(&path).err().unwrap();
        assert!(format!("{error:#}").contains("newer than supported"));
        let connection = Connection::open(&path).unwrap();
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION + 1);
    }

    #[test]
    fn every_store_connection_enables_foreign_keys_and_wal() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let store = Store::open(&path).unwrap();
        let foreign_keys: bool = store
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        let journal_mode: String = store
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert!(foreign_keys);
        assert_eq!(journal_mode, "wal");
        assert_eq!(path.symlink_metadata().unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn unsafe_existing_database_file_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        std::fs::write(&path, []).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(Store::open(&path).is_err());
    }

    #[test]
    fn symlinked_database_file_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("actual.sqlite3");
        std::fs::write(&actual, []).unwrap();
        let linked = dir.path().join("linked.sqlite3");
        symlink(&actual, &linked).unwrap();

        assert!(Store::open(&linked).is_err());
    }
}
