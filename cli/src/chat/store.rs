use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::types::{
    Actor, Attempt, ClientEvent, Draft, FeedEvent, Handoff, ImportResult, Limits, Message,
    PeerEvent, PeerPayload, Registration, SessionRef, SessionState, Target,
};

const SCHEMA_VERSION: u32 = 6;
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

    pub(super) fn load_remote_sessions(&self) -> Result<Vec<SessionState>> {
        if schema_version(&self.connection)? < 6 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare("SELECT payload_json FROM remote_sessions ORDER BY machine, incarnation")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|raw| Ok(serde_json::from_str(&raw)?))
            .collect()
    }

    pub(super) fn save_session(&mut self, entry: &SessionState) -> Result<()> {
        let registration = &entry.registration;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let was_ended: bool = transaction
            .query_row(
                "SELECT ended FROM sessions WHERE machine = ?1 AND incarnation = ?2",
                params![
                    registration.session.machine,
                    registration.session.incarnation
                ],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
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
        if entry.ended && !was_ended {
            let target = serde_json::to_string(&Target::Agent(registration.session.clone()))?;
            let pending = transaction
                .prepare(
                    "SELECT d.message_id FROM delivery_receipts d
                     WHERE d.target_key = ?1 AND d.state = 'queued'
                       AND NOT EXISTS (
                         SELECT 1 FROM delivery_attempt_messages m
                         JOIN delivery_attempts a ON a.id = m.attempt_id
                         WHERE m.message_id = d.message_id AND a.recipient_key = d.target_key
                           AND a.state = 'submitting'
                       )",
                )?
                .query_map([&target], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let now = Utc::now().to_rfc3339();
            for id in pending {
                transaction.execute(
                    "UPDATE delivery_receipts SET state = 'unavailable', explicit_retry = 0, updated_at = ?3
                     WHERE message_id = ?1 AND target_key = ?2 AND state = 'queued'",
                    params![id, target, now],
                )?;
                append_event(
                    &transaction,
                    &id,
                    &FeedEvent::Receipt {
                        message_id: id.clone(),
                        recipient: registration.session.clone(),
                        attempt_id: None,
                        state: "unavailable".into(),
                        outcome: None,
                    },
                )?;
            }
        }
        if schema_version(&transaction)? >= 6 {
            let sequence = next_sequence(
                &transaction,
                "SELECT COALESCE(MAX(origin_seq), 0) + 1 FROM peer_outbox WHERE project = ?1",
                &registration.project,
            )?;
            transaction.execute(
                "INSERT INTO peer_outbox(project, origin_seq, event_id, payload_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    registration.project,
                    to_sql_integer(sequence)?,
                    uuid::Uuid::new_v4().to_string(),
                    serde_json::to_string(&PeerPayload::Presence {
                        session: entry.clone(),
                        epoch: sequence,
                    })?
                ],
            )?;
        }
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

        if let Some(message) = find_retry(
            &transaction,
            &sender_key,
            &canonical_draft.key,
            &canonical_draft_json,
        )? {
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
                }
                transaction.execute(
                    "INSERT INTO delivery_receipts(message_id, target_key) VALUES (?1, ?2)",
                    params![message.id, target_key],
                )?;
                transaction.execute(
                    "INSERT INTO exposures(message_id, target_key) VALUES (?1, ?2)",
                    params![message.id, target_key],
                )?;
            }
        }

        let event_id = uuid::Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO events(event_id, project, project_seq, payload_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                event_id,
                message.draft.project,
                to_sql_integer(project_seq)?,
                payload,
            ],
        )?;
        append_peer_outbox(
            &transaction,
            &message.draft.project,
            &event_id,
            &PeerPayload::Message {
                message: message.clone(),
            },
        )?;
        transaction.commit()?;
        Ok(message)
    }

    pub fn claim(&mut self, recipient: &SessionRef, limits: Limits) -> Result<Option<Attempt>> {
        let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM delivery_attempts WHERE recipient_key = ?1 AND state = 'submitting')", [&target], |r| r.get(0))?;
        if active {
            return Ok(None);
        }
        let (messages, pending_count) = {
            let mut statement = transaction.prepare(
                "WITH pending AS (
                    SELECT i.message_id, i.inbox_seq FROM inbox_entries i
                    JOIN delivery_receipts d ON d.message_id = i.message_id AND d.target_key = i.recipient_key
                    JOIN exposures x ON x.message_id = i.message_id AND x.target_key = i.recipient_key
                    WHERE i.recipient_key = ?1 AND d.state = 'queued'
                      AND (x.fetched_at IS NULL OR d.explicit_retry = 1)
                 )
                 SELECT m.payload_json, (SELECT COUNT(*) FROM pending)
                 FROM pending i JOIN messages m ON m.id = i.message_id
                 ORDER BY i.inbox_seq LIMIT 100",
            )?;
            let mut rows = statement.query([&target])?;
            let mut messages = Vec::new();
            let mut count = 0;
            while let Some(row) = rows.next()? {
                let payload: String = row.get(0)?;
                count = usize::try_from(row.get::<_, i64>(1)?)?;
                messages.push(serde_json::from_str::<Message>(&payload)?);
            }
            (messages, count)
        };
        if messages.is_empty() {
            return Ok(None);
        }
        let batch = if pending_count == messages.len() {
            super::render::render_batch(&messages, limits)?
        } else {
            super::render::render_pending(&messages, limits, pending_count)?
        };
        let attempt = Attempt {
            id: uuid::Uuid::new_v4().to_string(),
            recipient: recipient.clone(),
            batch,
        };
        let now = Utc::now().to_rfc3339();
        transaction.execute("INSERT INTO delivery_attempts(id, recipient_key, state, payload_json, created_at) VALUES (?1, ?2, 'submitting', ?3, ?4)", params![attempt.id, target, serde_json::to_string(&attempt)?, now])?;
        for (ordinal, id) in messages
            .iter()
            .map(|m| &m.id)
            .filter(|id| {
                attempt.batch.full_ids.contains(id) || attempt.batch.preview_ids.contains(id)
            })
            .enumerate()
        {
            transaction.execute("INSERT INTO delivery_attempt_messages(attempt_id, ordinal, message_id) VALUES (?1, ?2, ?3)", params![attempt.id, ordinal as i64, id])?;
            transaction.execute("UPDATE delivery_receipts SET state = 'submitting', evidence_json = NULL, updated_at = ?3 WHERE message_id = ?1 AND target_key = ?2", params![id, target, now])?;
            append_event(
                &transaction,
                id,
                &FeedEvent::Receipt {
                    message_id: id.clone(),
                    recipient: recipient.clone(),
                    attempt_id: Some(attempt.id.clone()),
                    state: "submitting".into(),
                    outcome: None,
                },
            )?;
        }
        transaction.commit()?;
        Ok(Some(attempt))
    }

    pub(super) fn has_pending(&self, recipient: &SessionRef) -> Result<bool> {
        let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM inbox_entries i
             JOIN delivery_receipts d ON d.message_id = i.message_id AND d.target_key = i.recipient_key
             JOIN exposures x ON x.message_id = i.message_id AND x.target_key = i.recipient_key
             WHERE i.recipient_key = ?1 AND d.state = 'queued'
               AND (x.fetched_at IS NULL OR d.explicit_retry = 1))",
            [&target],
            |row| row.get(0),
        )?)
    }

    pub fn finish(&mut self, attempt_id: &str, outcome: Handoff) -> Result<()> {
        let evidence = match &outcome {
            Handoff::Accepted { receipt } => receipt,
            Handoff::NotSubmitted { reason }
            | Handoff::Refused { reason }
            | Handoff::Unknown { reason } => reason,
        };
        ensure!(
            !evidence.is_empty() && evidence.len() <= 4096,
            "handoff evidence must be 1..4096 bytes"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (state, payload, previous): (String, String, Option<String>) = transaction
            .query_row(
                "SELECT state, payload_json, outcome_json FROM delivery_attempts WHERE id = ?1",
                [attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .context("Chat attempt is unavailable")?;
        let encoded = serde_json::to_string(&outcome)?;
        if previous.as_deref() == Some(&encoded) {
            return Ok(());
        }
        ensure!(state == "submitting", "Chat handoff outcome is already final; reconciliation requires separate validated evidence");
        let attempt: Attempt = serde_json::from_str(&payload)?;
        let target = serde_json::to_string(&Target::Agent(attempt.recipient.clone()))?;
        let state = match &outcome {
            Handoff::NotSubmitted { .. } => "queued",
            Handoff::Accepted { .. } => "accepted",
            Handoff::Refused { .. } => "refused",
            Handoff::Unknown { .. } => "unknown",
        };
        let ended: bool = transaction
            .query_row(
                "SELECT ended FROM sessions WHERE machine = ?1 AND incarnation = ?2",
                params![attempt.recipient.machine, attempt.recipient.incarnation],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        let receipt_state = if state == "queued" && ended {
            "unavailable"
        } else {
            state
        };
        let now = Utc::now().to_rfc3339();
        transaction.execute("UPDATE delivery_attempts SET state = ?2, outcome_json = ?3, completed_at = ?4 WHERE id = ?1", params![attempt_id, if state == "queued" { "not_submitted" } else { state }, encoded, now])?;
        let ids = transaction.prepare("SELECT message_id FROM delivery_attempt_messages WHERE attempt_id = ?1 ORDER BY ordinal")?.query_map([attempt_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for id in &ids {
            transaction.execute("UPDATE delivery_receipts SET state = ?3, evidence_json = ?4, updated_at = ?5, explicit_retry = CASE WHEN ?3 = 'queued' THEN explicit_retry ELSE 0 END WHERE message_id = ?1 AND target_key = ?2", params![id, target, receipt_state, encoded, now])?;
            append_event(
                &transaction,
                id,
                &FeedEvent::Receipt {
                    message_id: id.clone(),
                    recipient: attempt.recipient.clone(),
                    attempt_id: Some(attempt.id.clone()),
                    state: receipt_state.into(),
                    outcome: Some(outcome.clone()),
                },
            )?;
            if matches!(outcome, Handoff::Accepted { .. }) {
                let exposure = if attempt.batch.full_ids.contains(id) {
                    "full"
                } else {
                    "preview"
                };
                transaction.execute("UPDATE exposures SET state = ?3, updated_at = ?4 WHERE message_id = ?1 AND target_key = ?2", params![id, target, exposure, now])?;
                append_event(
                    &transaction,
                    id,
                    &FeedEvent::Exposure {
                        message_id: id.clone(),
                        recipient: attempt.recipient.clone(),
                        state: exposure.into(),
                    },
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn attempt_recipient(&self, id: &str) -> Result<SessionRef> {
        let payload: String = self.connection.query_row(
            "SELECT payload_json FROM delivery_attempts WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str::<Attempt>(&payload)?.recipient)
    }

    pub(super) fn recover_submitting(&mut self) -> Result<usize> {
        let ids = self
            .connection
            .prepare("SELECT id FROM delivery_attempts WHERE state = 'submitting'")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in &ids {
            self.finish(
                id,
                Handoff::Unknown {
                    reason: "daemon restarted after claim; native handoff may have occurred".into(),
                },
            )?;
        }
        Ok(ids.len())
    }

    pub(super) fn observe(
        &mut self,
        recipient: &SessionRef,
        event: ClientEvent,
        epoch: u64,
    ) -> Result<bool> {
        let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
        Ok(self.connection.execute("INSERT INTO delivery_observations(recipient_key, epoch, state) VALUES (?1, ?2, ?3) ON CONFLICT(recipient_key) DO UPDATE SET epoch = excluded.epoch, state = excluded.state WHERE excluded.epoch > delivery_observations.epoch", params![target, to_sql_integer(epoch)?, serde_json::to_string(&event)?])? > 0)
    }

    pub(super) fn observation(&self, recipient: &SessionRef) -> Result<Option<(ClientEvent, u64)>> {
        let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
        let row: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT epoch, state FROM delivery_observations WHERE recipient_key = ?1",
                [target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(epoch, state)| {
            Ok((
                serde_json::from_str(&state)?,
                u64::try_from(epoch).context("Chat observation epoch is negative")?,
            ))
        })
        .transpose()
    }

    pub(super) fn retry_delivery(&mut self, recipient: &SessionRef, id: &str) -> Result<()> {
        let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(transaction.execute("UPDATE delivery_receipts SET state = 'queued', explicit_retry = 1, updated_at = ?3 WHERE message_id = ?1 AND target_key = ?2 AND state IN ('unknown', 'refused')", params![id, target, Utc::now().to_rfc3339()])? == 1, "only the original recipient may explicitly retry an unknown or refused handoff; duplicate delivery is possible");
        append_event(
            &transaction,
            id,
            &FeedEvent::Receipt {
                message_id: id.into(),
                recipient: recipient.clone(),
                attempt_id: None,
                state: "queued".into(),
                outcome: None,
            },
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn record_fetch(&mut self, recipient: &SessionRef, ids: &[String]) -> Result<()> {
        let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for id in ids {
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM exposures WHERE message_id = ?1 AND target_key = ?2)",
                params![id, target],
                |r| r.get(0),
            )?;
            ensure!(exists, "fetch is outside this recipient binding");
            if transaction.execute("UPDATE exposures SET fetched_at = ?3 WHERE message_id = ?1 AND target_key = ?2 AND fetched_at IS NULL", params![id, target, Utc::now().to_rfc3339()])? > 0 {
                append_event(&transaction, id, &FeedEvent::Fetched { message_id: id.clone(), recipient: recipient.clone() })?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub(super) fn cursor_key(&mut self) -> Result<String> {
        let fresh = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        self.connection.execute(
            "INSERT OR IGNORE INTO metadata(key, value) VALUES ('cursor_key', ?1)",
            [fresh],
        )?;
        Ok(self.connection.query_row(
            "SELECT value FROM metadata WHERE key = 'cursor_key'",
            [],
            |row| row.get(0),
        )?)
    }

    /// The sole owner checks committed retries before applying current routing
    /// eligibility. A retry must not recalculate an immutable recipient set.
    pub(super) fn retry(&self, sender: &Actor, draft: &Draft) -> Result<Option<Message>> {
        let canonical = canonical_draft(draft)?;
        find_retry(
            &self.connection,
            &serde_json::to_string(sender)?,
            &canonical.key,
            &serde_json::to_string(&canonical)?,
        )
    }

    pub(super) fn message(&self, id: &str) -> Result<Message> {
        let payload: String = self
            .connection
            .query_row(
                "SELECT payload_json FROM messages WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .context("Chat message is unavailable")?;
        Ok(serde_json::from_str(&payload)?)
    }

    /// Locally originated events use a separate, gapless sequence. Imported
    /// observer events must never enter this stream or be echoed to a peer.
    pub fn export_events(&self, project: &str, after: u64, limit: usize) -> Result<Vec<PeerEvent>> {
        ensure!((1..=100).contains(&limit), "invalid peer page limit");
        let latest: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(origin_seq), 0) FROM peer_outbox WHERE project = ?1",
            [project],
            |row| row.get(0),
        )?;
        ensure!(
            to_sql_integer(after)? <= latest,
            "peer replay cursor is newer than origin history"
        );
        let origin = self.machine()?;
        let mut statement = self.connection.prepare(
            "SELECT origin_seq, event_id, payload_json FROM peer_outbox
             WHERE project = ?1 AND origin_seq > ?2 ORDER BY origin_seq LIMIT ?3",
        )?;
        let mut rows = statement.query(params![
            project,
            to_sql_integer(after)?,
            i64::try_from(limit)?
        ])?;
        let mut events = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            let payload: String = row.get(2)?;
            if bytes + payload.len() > 768 * 1024 {
                break;
            }
            bytes += payload.len();
            events.push(PeerEvent {
                id: row.get(1)?,
                origin: origin.clone(),
                seq: u64::try_from(row.get::<_, i64>(0)?)?,
                project: project.into(),
                payload: serde_json::from_str(&payload)?,
            });
        }
        Ok(events)
    }

    /// Applies one peer event and its replay cursor in the same transaction.
    /// A duplicate is accepted only if every immutable field still matches.
    pub fn import_event(
        &mut self,
        authenticated_peer: &str,
        event: &PeerEvent,
        projects: &[String],
    ) -> Result<ImportResult> {
        ensure!(
            event.origin == authenticated_peer,
            "peer event origin does not match authenticated peer"
        );
        ensure!(
            projects.iter().any(|project| project == &event.project),
            "peer project is not authorized"
        );
        ensure!(event.seq > 0, "peer event sequence must be positive");
        super::protocol::validate_uuid(&event.id)?;
        super::protocol::validate_uuid(&event.origin)?;
        super::protocol::validate_uuid(&event.project)?;
        let payload = serde_json::to_string(&event.payload)?;
        ensure!(
            payload.len() <= 768 * 1024,
            "peer event exceeds payload limit"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let machine: String = transaction.query_row(
            "SELECT value FROM metadata WHERE key = 'machine'",
            [],
            |row| row.get(0),
        )?;
        ensure!(
            authenticated_peer != machine,
            "cannot import own peer events"
        );
        let through: i64 = transaction
            .query_row(
                "SELECT received_through FROM peer_cursors WHERE peer = ?1 AND project = ?2",
                params![authenticated_peer, event.project],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let through = u64::try_from(through)?;
        if event.seq <= through {
            let stored: Option<(String, String)> = transaction
                .query_row(
                    "SELECT event_id, payload_json FROM imported_events
                 WHERE origin = ?1 AND project = ?2 AND origin_seq = ?3",
                    params![
                        authenticated_peer,
                        event.project,
                        to_sql_integer(event.seq)?
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            ensure!(
                matches!(stored, Some((ref id, ref body)) if id == &event.id && body == &payload),
                "conflicting peer replay"
            );
            return Ok(ImportResult::Duplicate { through });
        }
        ensure!(event.seq == through + 1, "peer event sequence gap");
        let reused: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM imported_events WHERE event_id = ?1)",
            [&event.id],
            |row| row.get(0),
        )?;
        ensure!(!reused, "peer event ID was reused");
        apply_peer_payload(&transaction, authenticated_peer, &machine, event)?;
        transaction.execute(
            "INSERT INTO imported_events(event_id, origin, project, origin_seq, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.id,
                authenticated_peer,
                event.project,
                to_sql_integer(event.seq)?,
                payload
            ],
        )?;
        transaction.execute(
            "INSERT INTO peer_cursors(peer, project, received_through) VALUES (?1, ?2, ?3)
             ON CONFLICT(peer, project) DO UPDATE SET received_through = excluded.received_through",
            params![
                authenticated_peer,
                event.project,
                to_sql_integer(event.seq)?
            ],
        )?;
        transaction.commit()?;
        Ok(ImportResult::Applied { through: event.seq })
    }

    pub fn record_peer_ack(&mut self, peer: &str, project: &str, through: u64) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(origin_seq), 0) FROM peer_outbox WHERE project = ?1",
            [project],
            |row| row.get(0),
        )?;
        ensure!(
            to_sql_integer(through)? <= latest,
            "peer acknowledged unseen events"
        );
        transaction.execute(
            "INSERT INTO peer_cursors(peer, project, acknowledged_through) VALUES (?1, ?2, ?3)
             ON CONFLICT(peer, project) DO UPDATE SET
             acknowledged_through = MAX(peer_cursors.acknowledged_through, excluded.acknowledged_through)",
            params![peer, project, to_sql_integer(through)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn peer_cursor(&self, peer: &str, project: &str) -> Result<(u64, u64)> {
        let pair: Option<(i64, i64)> = self
            .connection
            .query_row(
                "SELECT received_through, acknowledged_through FROM peer_cursors
             WHERE peer = ?1 AND project = ?2",
                params![peer, project],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (received, acknowledged) = pair.unwrap_or((0, 0));
        Ok((u64::try_from(received)?, u64::try_from(acknowledged)?))
    }

    /// Durable feed sequence, filtered before pagination. Reads never update inbox,
    /// receipt or exposure state. The serialized page is capped below a frame.
    pub(super) fn feed(
        &self,
        project: &str,
        viewer: Option<&SessionRef>,
        after: u64,
        limit: u16,
        inbox: bool,
    ) -> Result<Vec<(u64, FeedEvent)>> {
        ensure!((1..=100).contains(&limit), "invalid Chat page limit");
        let sender = viewer
            .map(|s| serde_json::to_string(&Actor::Agent(s.clone())))
            .transpose()?;
        let target = viewer
            .map(|s| serde_json::to_string(&Target::Agent(s.clone())))
            .transpose()?;
        let mut statement = self.connection.prepare(
            "SELECT e.project_seq, e.payload_json, e.kind FROM events e
             JOIN messages m ON m.id = COALESCE(e.message_id, json_extract(e.payload_json, '$.id'))
             WHERE e.project = ?1 AND e.project_seq > ?2
               AND (?5 = 0 OR e.kind = 'message')
               AND (?3 IS NULL OR (?5 = 0 AND m.sender_key = ?3) OR EXISTS (
                   SELECT 1 FROM recipients r WHERE r.message_id = m.id AND r.target_key = ?4))
             ORDER BY e.project_seq LIMIT ?6",
        )?;
        let mut rows = statement.query(params![
            project,
            to_sql_integer(after)?,
            sender,
            target,
            inbox,
            limit
        ])?;
        let mut page = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let payload: String = row.get(1)?;
            if bytes + payload.len() > 768 * 1024 {
                break;
            }
            bytes += payload.len();
            let kind: String = row.get(2)?;
            let event = if kind == "message" {
                FeedEvent::Message {
                    message: serde_json::from_str(&payload)?,
                }
            } else {
                serde_json::from_str(&payload)?
            };
            page.push((u64::try_from(seq)?, event));
        }
        Ok(page)
    }

    pub(super) fn latest_project_sequence(&self, project: &str) -> Result<u64> {
        let latest: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(project_seq), 0) FROM events WHERE project = ?1",
            [project],
            |row| row.get(0),
        )?;
        u64::try_from(latest).context("Chat event sequence is negative")
    }

    pub(super) fn feed_before(
        &self,
        project: &str,
        viewer: Option<&SessionRef>,
        before: Option<u64>,
        limit: u16,
    ) -> Result<Vec<(u64, FeedEvent)>> {
        ensure!((1..=100).contains(&limit), "invalid Chat page limit");
        let sender = viewer
            .map(|session| serde_json::to_string(&Actor::Agent(session.clone())))
            .transpose()?;
        let target = viewer
            .map(|session| serde_json::to_string(&Target::Agent(session.clone())))
            .transpose()?;
        let mut statement = self.connection.prepare(
            "SELECT e.project_seq, e.payload_json, e.kind FROM events e
             JOIN messages m ON m.id = COALESCE(e.message_id, json_extract(e.payload_json, '$.id'))
             WHERE e.project = ?1 AND (?2 IS NULL OR e.project_seq < ?2)
               AND (?3 IS NULL OR m.sender_key = ?3 OR EXISTS (
                   SELECT 1 FROM recipients r WHERE r.message_id = m.id AND r.target_key = ?4))
             ORDER BY e.project_seq DESC LIMIT ?5",
        )?;
        let mut rows = statement.query(params![
            project,
            before.map(to_sql_integer).transpose()?,
            sender,
            target,
            limit,
        ])?;
        let mut page = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            let sequence: i64 = row.get(0)?;
            let payload: String = row.get(1)?;
            if bytes + payload.len() > 768 * 1024 {
                break;
            }
            bytes += payload.len();
            let kind: String = row.get(2)?;
            let event = if kind == "message" {
                FeedEvent::Message {
                    message: serde_json::from_str(&payload)?,
                }
            } else {
                serde_json::from_str(&payload)?
            };
            page.push((u64::try_from(sequence)?, event));
        }
        page.reverse();
        Ok(page)
    }

    pub(super) fn has_feed_before(
        &self,
        project: &str,
        viewer: Option<&SessionRef>,
        before: u64,
    ) -> Result<bool> {
        let sender = viewer
            .map(|session| serde_json::to_string(&Actor::Agent(session.clone())))
            .transpose()?;
        let target = viewer
            .map(|session| serde_json::to_string(&Target::Agent(session.clone())))
            .transpose()?;
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events e
             JOIN messages m ON m.id = COALESCE(e.message_id, json_extract(e.payload_json, '$.id'))
             WHERE e.project = ?1 AND e.project_seq < ?2
               AND (?3 IS NULL OR m.sender_key = ?3 OR EXISTS (
                   SELECT 1 FROM recipients r WHERE r.message_id = m.id AND r.target_key = ?4)))",
            params![project, to_sql_integer(before)?, sender, target],
            |row| row.get(0),
        )?;
        Ok(exists)
    }
}

fn append_event(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
    event: &FeedEvent,
) -> Result<()> {
    let project: String = transaction.query_row(
        "SELECT json_extract(payload_json, '$.draft.project') FROM messages WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    let sequence = next_sequence(
        transaction,
        "SELECT COALESCE(MAX(project_seq), 0) + 1 FROM events WHERE project = ?1",
        &project,
    )?;
    let kind = match event {
        FeedEvent::Message { .. } => "message",
        FeedEvent::Receipt { .. } => "receipt",
        FeedEvent::Exposure { .. } => "exposure",
        FeedEvent::Fetched { .. } => "fetched",
    };
    let event_id = uuid::Uuid::new_v4().to_string();
    transaction.execute("INSERT INTO events(event_id, project, project_seq, payload_json, kind, message_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![event_id, project, to_sql_integer(sequence)?, serde_json::to_string(event)?, kind, id])?;
    append_peer_outbox(transaction, &project, &event_id, &peer_payload(event))?;
    Ok(())
}

fn peer_payload(event: &FeedEvent) -> PeerPayload {
    match event {
        FeedEvent::Message { message } => PeerPayload::Message {
            message: message.clone(),
        },
        FeedEvent::Receipt {
            message_id,
            recipient,
            attempt_id,
            state,
            outcome,
        } => PeerPayload::Receipt {
            message_id: message_id.clone(),
            recipient: recipient.clone(),
            state: Some(state.clone()),
            exposure: None,
            attempt_id: attempt_id.clone(),
            outcome: outcome.clone(),
        },
        FeedEvent::Exposure {
            message_id,
            recipient,
            state,
        } => PeerPayload::Receipt {
            message_id: message_id.clone(),
            recipient: recipient.clone(),
            state: None,
            exposure: Some(state.clone()),
            attempt_id: None,
            outcome: None,
        },
        FeedEvent::Fetched {
            message_id,
            recipient,
        } => PeerPayload::Receipt {
            message_id: message_id.clone(),
            recipient: recipient.clone(),
            state: None,
            exposure: Some("fetched".into()),
            attempt_id: None,
            outcome: None,
        },
    }
}

fn append_peer_outbox(
    transaction: &rusqlite::Transaction<'_>,
    project: &str,
    event_id: &str,
    payload: &PeerPayload,
) -> Result<()> {
    if schema_version(transaction)? < 6 {
        return Ok(());
    }
    let sequence = next_sequence(
        transaction,
        "SELECT COALESCE(MAX(origin_seq), 0) + 1 FROM peer_outbox WHERE project = ?1",
        project,
    )?;
    transaction.execute(
        "INSERT INTO peer_outbox(project, origin_seq, event_id, payload_json) VALUES (?1, ?2, ?3, ?4)",
        params![project, to_sql_integer(sequence)?, event_id, serde_json::to_string(payload)?],
    )?;
    Ok(())
}

fn backfill_peer_outbox(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let mut statement = transaction.prepare(
        "SELECT event_id, project, project_seq, kind, payload_json FROM events ORDER BY project, project_seq",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (id, project, sequence, kind, raw) in rows {
        let payload = if kind == "message" {
            PeerPayload::Message {
                message: serde_json::from_str(&raw)?,
            }
        } else {
            peer_payload(&serde_json::from_str::<FeedEvent>(&raw)?)
        };
        transaction.execute(
            "INSERT INTO peer_outbox(project, origin_seq, event_id, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![project, sequence, id, serde_json::to_string(&payload)?],
        )?;
    }
    let sessions = transaction
        .prepare(
            "SELECT machine, incarnation, project, name, client, native_id, process_start,
                eligible, connected, ended FROM sessions ORDER BY project, machine, incarnation",
        )?
        .query_map([], |row| {
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
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for session in sessions {
        let project = session.registration.project.clone();
        let sequence = next_sequence(
            transaction,
            "SELECT COALESCE(MAX(origin_seq), 0) + 1 FROM peer_outbox WHERE project = ?1",
            &project,
        )?;
        transaction.execute(
            "INSERT INTO peer_outbox(project, origin_seq, event_id, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![project, to_sql_integer(sequence)?, uuid::Uuid::new_v4().to_string(),
                serde_json::to_string(&PeerPayload::Presence { session, epoch: sequence })?],
        )?;
    }
    Ok(())
}

fn apply_peer_payload(
    transaction: &rusqlite::Transaction<'_>,
    origin: &str,
    machine: &str,
    event: &PeerEvent,
) -> Result<()> {
    match &event.payload {
        PeerPayload::Message { message } => {
            ensure!(
                actor_machine(&message.sender) == origin,
                "peer message sender is not origin-owned"
            );
            ensure!(
                message.draft.project == event.project,
                "peer message project differs from event"
            );
            ensure!(
                message.sender_seq > 0,
                "peer sender sequence must be positive"
            );
            ensure!(
                !message.draft.key.is_empty() && message.draft.key.len() <= 128,
                "invalid peer message key"
            );
            ensure!(
                !message.draft.to.is_empty() && message.draft.to.len() <= 128,
                "invalid peer recipient count"
            );
            ensure!(
                message.draft.body.len() <= MAX_BODY_BYTES,
                "peer message body exceeds 64 KiB"
            );
            super::protocol::validate_uuid(&message.id)?;
            match &message.sender {
                Actor::Agent(session) => super::protocol::validate_session(session)?,
                Actor::Human { machine } => super::protocol::validate_uuid(machine)?,
            }
            for target in &message.draft.to {
                match target {
                    Target::Agent(session) => super::protocol::validate_session(session)?,
                    Target::Human { machine } => super::protocol::validate_uuid(machine)?,
                }
            }
            if let Some(reply_to) = &message.draft.reply_to {
                super::protocol::validate_uuid(reply_to)?;
            }
            ensure!(
                serde_json::to_string(&canonical_draft(&message.draft)?)?
                    == serde_json::to_string(&message.draft)?,
                "peer message recipients are not canonical",
            );
            let sender_key = serde_json::to_string(&message.sender)?;
            transaction.execute(
                "INSERT INTO messages(id, sender_key, sender_seq, idempotency_key, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    message.id,
                    sender_key,
                    to_sql_integer(message.sender_seq)?,
                    message.draft.key,
                    serde_json::to_string(message)?
                ],
            )?;
            let mut local_recipients = Vec::new();
            for target in &message.draft.to {
                let target_key = serde_json::to_string(target)?;
                transaction.execute(
                    "INSERT INTO recipients(message_id, target_key) VALUES (?1, ?2)",
                    params![message.id, target_key],
                )?;
                if let Target::Agent(recipient) = target {
                    if recipient.machine == machine {
                        let ended: bool = transaction.query_row(
                            "SELECT ended FROM sessions WHERE machine = ?1 AND incarnation = ?2",
                            params![machine, recipient.incarnation], |row| row.get(0),
                        ).optional()?.unwrap_or(false);
                        if !ended {
                            let inbox_seq = next_sequence(
                                transaction,
                                "SELECT COALESCE(MAX(inbox_seq), 0) + 1 FROM inbox_entries WHERE recipient_key = ?1",
                                &target_key,
                            )?;
                            transaction.execute(
                                "INSERT INTO inbox_entries(recipient_key, inbox_seq, message_id) VALUES (?1, ?2, ?3)",
                                params![target_key, to_sql_integer(inbox_seq)?, message.id],
                            )?;
                        }
                        transaction.execute(
                            "INSERT INTO delivery_receipts(message_id, target_key, state) VALUES (?1, ?2, ?3)",
                            params![message.id, target_key, if ended { "unavailable" } else { "queued" }],
                        )?;
                        transaction.execute(
                            "INSERT INTO exposures(message_id, target_key) VALUES (?1, ?2)",
                            params![message.id, target_key],
                        )?;
                        local_recipients.push((recipient.clone(), ended));
                    }
                }
            }
            append_imported_observer(
                transaction,
                event,
                &message.id,
                &FeedEvent::Message {
                    message: message.clone(),
                },
            )?;
            for (recipient, ended) in local_recipients {
                // This receipt is locally originated; the imported message is not.
                append_event(
                    transaction,
                    &message.id,
                    &FeedEvent::Receipt {
                        message_id: message.id.clone(),
                        recipient,
                        attempt_id: None,
                        state: if ended {
                            "unavailable"
                        } else {
                            "received_remotely"
                        }
                        .into(),
                        outcome: None,
                    },
                )?;
            }
        }
        PeerPayload::Receipt {
            message_id,
            recipient,
            state,
            exposure,
            attempt_id,
            outcome,
        } => {
            ensure!(
                recipient.machine == origin,
                "peer receipt recipient is not origin-owned"
            );
            ensure!(
                state.is_some() != exposure.is_some(),
                "peer receipt needs exactly one state field"
            );
            super::protocol::validate_uuid(message_id)?;
            super::protocol::validate_session(recipient)?;
            if let Some(attempt_id) = attempt_id {
                super::protocol::validate_uuid(attempt_id)?;
            }
            let target = serde_json::to_string(&Target::Agent(recipient.clone()))?;
            let recorded_project: Option<String> = transaction
                .query_row(
                    "SELECT json_extract(m.payload_json, '$.draft.project') FROM messages m
                 JOIN recipients r ON r.message_id = m.id
                 WHERE m.id = ?1 AND r.target_key = ?2",
                    params![message_id, target],
                    |row| row.get(0),
                )
                .optional()?;
            ensure!(
                recorded_project.as_deref() == Some(event.project.as_str()),
                "peer receipt is outside frozen recipients or project"
            );
            let now = Utc::now().to_rfc3339();
            if let Some(state) = state {
                ensure!(
                    matches!(
                        state.as_str(),
                        "queued"
                            | "received_remotely"
                            | "unavailable"
                            | "submitting"
                            | "accepted"
                            | "refused"
                            | "unknown"
                    ),
                    "invalid peer receipt state"
                );
                ensure!(exposure.is_none(), "mixed peer receipt fields");
                transaction.execute(
                    "UPDATE delivery_receipts SET state = ?3, evidence_json = ?4, updated_at = ?5
                     WHERE message_id = ?1 AND target_key = ?2",
                    params![
                        message_id,
                        target,
                        state,
                        outcome.as_ref().map(serde_json::to_string).transpose()?,
                        now
                    ],
                )?;
                append_imported_observer(
                    transaction,
                    event,
                    message_id,
                    &FeedEvent::Receipt {
                        message_id: message_id.clone(),
                        recipient: recipient.clone(),
                        attempt_id: attempt_id.clone(),
                        state: state.clone(),
                        outcome: outcome.clone(),
                    },
                )?;
            } else if let Some(exposure) = exposure {
                ensure!(
                    matches!(exposure.as_str(), "unseen" | "preview" | "full" | "fetched"),
                    "invalid peer exposure state"
                );
                if exposure == "fetched" {
                    transaction.execute(
                        "UPDATE exposures SET fetched_at = ?3 WHERE message_id = ?1 AND target_key = ?2",
                        params![message_id, target, now],
                    )?;
                    append_imported_observer(
                        transaction,
                        event,
                        message_id,
                        &FeedEvent::Fetched {
                            message_id: message_id.clone(),
                            recipient: recipient.clone(),
                        },
                    )?;
                } else {
                    transaction.execute(
                        "UPDATE exposures SET state = ?3, updated_at = ?4 WHERE message_id = ?1 AND target_key = ?2",
                        params![message_id, target, exposure, now],
                    )?;
                    append_imported_observer(
                        transaction,
                        event,
                        message_id,
                        &FeedEvent::Exposure {
                            message_id: message_id.clone(),
                            recipient: recipient.clone(),
                            state: exposure.clone(),
                        },
                    )?;
                }
            }
        }
        PeerPayload::Presence { session, epoch } => {
            super::protocol::validate_session(&session.registration.session)?;
            ensure!(
                session.registration.session.machine == origin,
                "peer presence is not origin-owned"
            );
            ensure!(
                session.registration.project == event.project,
                "peer presence project differs from event"
            );
            ensure!(*epoch > 0, "peer presence epoch must be positive");
            ensure!(
                !session.registration.name.trim().is_empty()
                    && session.registration.name.len() <= 256,
                "invalid peer session name"
            );
            ensure!(
                matches!(
                    session.registration.client.as_str(),
                    "codex" | "claude" | "pi"
                ),
                "unsupported peer client"
            );
            ensure!(
                !session.registration.native_id.is_empty()
                    && !session.registration.process_start.is_empty(),
                "peer native identity is empty"
            );
            ensure!(
                !session.ended || (!session.connected && !session.registration.eligible),
                "ended peer session cannot be active"
            );
            let previous: Option<(String, i64)> = transaction.query_row(
                "SELECT payload_json, epoch FROM remote_sessions WHERE machine = ?1 AND incarnation = ?2",
                params![origin, session.registration.session.incarnation],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if let Some((raw, old_epoch)) = &previous {
                let old: SessionState = serde_json::from_str(raw)?;
                ensure!(
                    !old.ended || session.ended,
                    "ended peer incarnation cannot revive"
                );
                ensure!(
                    *epoch > u64::try_from(*old_epoch)?,
                    "peer presence epoch did not advance"
                );
                ensure!(
                    old.registration.project == event.project,
                    "peer incarnation changed project"
                );
                ensure!(
                    old.registration.client == session.registration.client
                        && old.registration.native_id == session.registration.native_id
                        && old.registration.process_start == session.registration.process_start,
                    "peer incarnation changed native identity"
                );
            }
            transaction.execute(
                "INSERT INTO remote_sessions(machine, incarnation, project, payload_json, epoch)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(machine, incarnation) DO UPDATE SET
                 payload_json = excluded.payload_json, epoch = excluded.epoch",
                params![
                    origin,
                    session.registration.session.incarnation,
                    event.project,
                    serde_json::to_string(session)?,
                    to_sql_integer(*epoch)?
                ],
            )?;
        }
    }
    Ok(())
}

fn append_imported_observer(
    transaction: &rusqlite::Transaction<'_>,
    event: &PeerEvent,
    message_id: &str,
    feed: &FeedEvent,
) -> Result<()> {
    let sequence = next_sequence(
        transaction,
        "SELECT COALESCE(MAX(project_seq), 0) + 1 FROM events WHERE project = ?1",
        &event.project,
    )?;
    let (kind, payload) = match feed {
        FeedEvent::Message { message } => ("message", serde_json::to_string(message)?),
        FeedEvent::Receipt { .. } => ("receipt", serde_json::to_string(feed)?),
        FeedEvent::Exposure { .. } => ("exposure", serde_json::to_string(feed)?),
        FeedEvent::Fetched { .. } => ("fetched", serde_json::to_string(feed)?),
    };
    transaction.execute(
        "INSERT INTO events(event_id, project, project_seq, payload_json, kind, message_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.id,
            event.project,
            to_sql_integer(sequence)?,
            payload,
            kind,
            message_id
        ],
    )?;
    Ok(())
}

fn find_retry(
    connection: &Connection,
    sender_key: &str,
    key: &str,
    canonical_draft_json: &str,
) -> Result<Option<Message>> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM messages WHERE sender_key = ?1 AND idempotency_key = ?2",
            params![sender_key, key],
            |row| row.get(0),
        )
        .optional()?;
    let Some(payload) = payload else {
        return Ok(None);
    };
    let message: Message =
        serde_json::from_str(&payload).context("stored Chat message contains invalid JSON")?;
    ensure!(
        serde_json::to_string(&message.draft)? == canonical_draft_json,
        "idempotency key was already used for a different Chat message"
    );
    Ok(Some(message))
}

fn migrate(connection: &mut Connection, version: u32) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if version == 0 {
        transaction.execute_batch(include_str!("schema.sql"))?;
    }
    if version < 2 {
        transaction.execute_batch(include_str!("migration-002.sql"))?;
    }
    if version < 3 {
        transaction.execute_batch(include_str!("migration-003.sql"))?;
    }
    if version < 4 {
        transaction.execute_batch(include_str!("migration-004.sql"))?;
    }
    if version < 5 {
        transaction.execute_batch(include_str!("migration-005.sql"))?;
    }
    if version < 6 {
        transaction.execute_batch(include_str!("migration-006.sql"))?;
        backfill_peer_outbox(&transaction)?;
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

fn schema_version(connection: &Connection) -> Result<u32> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
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
    use rusqlite::{params, Connection};

    use super::Store;
    use crate::chat::types::{
        Actor, Draft, ImportResult, PeerEvent, PeerPayload, Registration, SessionRef, SessionState,
        Target,
    };

    #[test]
    fn ending_local_session_makes_queued_receipt_unavailable_without_losing_message() {
        use crate::chat::registry::{ClientKind, NativeEvidence, Registry};
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let registration = Registry::new(&mut store)
            .unwrap()
            .connect(
                "lam",
                crate::name::Sources {
                    explicit: Some("pm".into()),
                    lam_name: None,
                    multiplexer: None,
                },
                NativeEvidence {
                    client: ClientKind::Claude,
                    native_id: "native".into(),
                    process_start: "boot:1:2".into(),
                },
                true,
            )
            .unwrap();
        let mut draft = draft("orphaned");
        draft.to = vec![Target::Agent(registration.session.clone())];
        draft.body = "orphaned".into();
        let message = store.send(&sender(), &draft).unwrap();
        Registry::new(&mut store)
            .unwrap()
            .end(&registration.session)
            .unwrap();
        let target = serde_json::to_string(&Target::Agent(registration.session.clone())).unwrap();
        let state: String = store
            .connection
            .query_row(
                "SELECT state FROM delivery_receipts WHERE message_id = ?1 AND target_key = ?2",
                params![message.id, target],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "unavailable");
        assert_eq!(store.message(&message.id).unwrap().draft.body, "orphaned");
        assert!(store
            .export_events("lam", 0, 100)
            .unwrap()
            .iter()
            .any(|event| {
                matches!(&event.payload, PeerPayload::Receipt { message_id, state: Some(state), .. }
                if message_id == &message.id && state == "unavailable")
            }));
    }

    #[test]
    fn not_submitted_handoff_after_session_end_stays_unavailable() {
        use crate::chat::{
            config::DEFAULT_LIMITS,
            registry::{ClientKind, NativeEvidence, Registry},
            types::Handoff,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let registration = Registry::new(&mut store)
            .unwrap()
            .connect(
                "lam",
                crate::name::Sources {
                    explicit: Some("pm".into()),
                    lam_name: None,
                    multiplexer: None,
                },
                NativeEvidence {
                    client: ClientKind::Claude,
                    native_id: "native".into(),
                    process_start: "boot:1:2".into(),
                },
                true,
            )
            .unwrap();
        let mut draft = draft("late-finish");
        draft.to = vec![Target::Agent(registration.session.clone())];
        let message = store.send(&sender(), &draft).unwrap();
        let attempt = store
            .claim(&registration.session, DEFAULT_LIMITS)
            .unwrap()
            .unwrap();
        Registry::new(&mut store)
            .unwrap()
            .end(&registration.session)
            .unwrap();
        store
            .finish(
                &attempt.id,
                Handoff::NotSubmitted {
                    reason: "closed".into(),
                },
            )
            .unwrap();
        let target = serde_json::to_string(&Target::Agent(registration.session)).unwrap();
        let state: String = store
            .connection
            .query_row(
                "SELECT state FROM delivery_receipts WHERE message_id = ?1 AND target_key = ?2",
                params![message.id, target],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "unavailable");
    }

    #[test]
    fn mixed_preview_and_full_attempt_preserves_inbox_order() {
        use crate::chat::config::DEFAULT_LIMITS;
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let mut long = draft("long");
        long.body = "a".repeat(12000);
        let first = store.send(&sender(), &long).unwrap().id;
        let second = store.send(&sender(), &draft("short")).unwrap().id;
        let recipient = SessionRef {
            machine: "pc".into(),
            incarnation: "dev-1".into(),
        };
        let attempt = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        assert_eq!(
            attempt.batch.preview_ids.as_slice(),
            std::slice::from_ref(&first)
        );
        assert_eq!(
            attempt.batch.full_ids.as_slice(),
            std::slice::from_ref(&second)
        );
        let ids = store
            .connection
            .prepare("SELECT message_id FROM delivery_attempt_messages ORDER BY ordinal")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ids, [first, second]);
    }

    #[test]
    fn delivery_transactions_roll_back_at_claim_finish_and_fetch_boundaries() {
        use crate::chat::{config::DEFAULT_LIMITS, types::Handoff};
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        let id = store.send(&sender(), &draft("one")).unwrap().id;
        let recipient = SessionRef {
            machine: "pc".into(),
            incarnation: "dev-1".into(),
        };
        store.connection.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON events WHEN NEW.kind = 'receipt' BEGIN SELECT RAISE(ABORT, 'interrupted before commit'); END;").unwrap();
        assert!(store.claim(&recipient, DEFAULT_LIMITS).is_err());
        let count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM delivery_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        store
            .connection
            .execute_batch("DROP TRIGGER reject_receipt;")
            .unwrap();
        let attempt = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        store.connection.execute_batch("CREATE TRIGGER reject_exposure BEFORE INSERT ON events WHEN NEW.kind = 'exposure' BEGIN SELECT RAISE(ABORT, 'interrupted after handoff'); END;").unwrap();
        assert!(store
            .finish(
                &attempt.id,
                Handoff::Accepted {
                    receipt: "native acceptance".into()
                }
            )
            .is_err());
        let state: String = store
            .connection
            .query_row("SELECT state FROM delivery_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "submitting");
        assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
        store
            .connection
            .execute_batch("DROP TRIGGER reject_exposure;")
            .unwrap();
        store.recover_submitting().unwrap();
        assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
        store.connection.execute_batch("CREATE TRIGGER reject_fetch BEFORE INSERT ON events WHEN NEW.kind = 'fetched' BEGIN SELECT RAISE(ABORT, 'interrupted fetch commit'); END;").unwrap();
        assert!(store
            .record_fetch(&recipient, std::slice::from_ref(&id))
            .is_err());
        let fetched: Option<String> = store
            .connection
            .query_row("SELECT fetched_at FROM exposures", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fetched, None);
        store
            .connection
            .execute_batch("DROP TRIGGER reject_fetch;")
            .unwrap();
        store.record_fetch(&recipient, &[id]).unwrap();
    }

    #[test]
    fn simultaneous_claims_commit_only_one_attempt() {
        use crate::chat::config::DEFAULT_LIMITS;
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let mut store = Store::open(&path).unwrap();
        store.set_machine("pc").unwrap();
        store.send(&sender(), &draft("one")).unwrap();
        drop(store);
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = Store::open(&path).unwrap();
                    barrier.wait();
                    store
                        .claim(
                            &SessionRef {
                                machine: "pc".into(),
                                incarnation: "dev-1".into(),
                            },
                            DEFAULT_LIMITS,
                        )
                        .unwrap()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .filter_map(|w| w.join().unwrap())
                .count(),
            1
        );
    }

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
    fn v4_upgrade_keeps_ended_recipients_and_allows_one_replacement() {
        use crate::chat::registry::{ClientKind, NativeEvidence, Registry};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite3");
        let connection = Connection::open(&path).unwrap();
        for schema in [
            include_str!("schema.sql"),
            include_str!("migration-002.sql"),
            include_str!("migration-003.sql"),
            include_str!("migration-004.sql"),
        ] {
            connection.execute_batch(schema).unwrap();
        }
        connection.pragma_update(None, "user_version", 4).unwrap();
        let mut legacy = Store { connection };
        legacy.set_machine("pc").unwrap();
        let connect = |store: &mut Store| {
            Registry::new(store).unwrap().connect(
                "lam",
                crate::name::Sources {
                    explicit: Some("owned".into()),
                    lam_name: None,
                    multiplexer: None,
                },
                NativeEvidence {
                    client: ClientKind::Codex,
                    native_id: "same-thread".into(),
                    process_start: "same-backend".into(),
                },
                true,
            )
        };
        let original = connect(&mut legacy).unwrap();
        let mut queued = draft("pinned-before-upgrade");
        queued.to = vec![Target::Agent(original.session.clone())];
        let message = legacy.send(&sender(), &queued).unwrap();
        Registry::new(&mut legacy)
            .unwrap()
            .end(&original.session)
            .unwrap();
        drop(legacy);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut upgraded = Store::open(&path).unwrap();
        let replay = upgraded.export_events("lam", 0, 10).unwrap();
        assert!(replay.iter().any(|event| matches!(
            &event.payload, PeerPayload::Presence { session, .. } if session.ended
        )));
        let replacement = connect(&mut upgraded).unwrap();
        assert_ne!(replacement.session, original.session);
        assert_eq!(connect(&mut upgraded).unwrap().session, replacement.session);
        assert_eq!(
            serde_json::to_value(upgraded.message(&message.id).unwrap()).unwrap(),
            serde_json::to_value(&message).unwrap()
        );
        assert!(upgraded
            .feed("lam", Some(&replacement.session), 0, 100, true)
            .unwrap()
            .is_empty());
        let mut registry = Registry::new(&mut upgraded).unwrap();
        assert!(registry.state(&original.session).unwrap().ended);
        let mut duplicate = replacement.clone();
        duplicate.session.incarnation = uuid::Uuid::new_v4().to_string();
        assert!(
            registry.register(duplicate).is_err(),
            "only one live native binding is permitted"
        );
        assert!(registry.register(original).is_err());
        drop(registry);
        let check_preserved = |store: &Store| {
            let violations: i64 = store
                .connection
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(violations, 0);
            let receipt: (String, String) = store
                .connection
                .query_row(
                    "SELECT target_key, state FROM delivery_receipts WHERE message_id = ?1",
                    [&message.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(receipt.0, serde_json::to_string(&queued.to[0]).unwrap());
            assert_eq!(receipt.1, "unavailable");
            assert_eq!(
                serde_json::to_value(store.message(&message.id).unwrap()).unwrap(),
                serde_json::to_value(&message).unwrap()
            );
        };
        check_preserved(&upgraded);
        drop(upgraded);
        let reopened = Store::open(&path).unwrap();
        check_preserved(&reopened);
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
    fn imported_message_is_durable_and_never_reexported() {
        let dir = tempfile::tempdir().unwrap();
        let pc_id = "11111111-1111-4111-8111-111111111111";
        let mac_id = "22222222-2222-4222-8222-222222222222";
        let project = "33333333-3333-4333-8333-333333333333";
        let mut pc = Store::open(&dir.path().join("pc.sqlite3")).unwrap();
        let mut mac = Store::open(&dir.path().join("mac.sqlite3")).unwrap();
        pc.set_machine(pc_id).unwrap();
        mac.set_machine(mac_id).unwrap();
        let sender = Actor::Agent(SessionRef {
            machine: pc_id.into(),
            incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        });
        let recipient = SessionRef {
            machine: mac_id.into(),
            incarnation: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into(),
        };
        let message = pc
            .send(
                &sender,
                &Draft {
                    key: "one".into(),
                    project: project.into(),
                    to: vec![Target::Agent(recipient.clone())],
                    body: "Pause after the tool".into(),
                    reply_to: None,
                },
            )
            .unwrap();
        let event = pc.export_events(project, 0, 10).unwrap().remove(0);
        assert!(
            pc.export_events(project, 2, 10).is_err(),
            "an origin restore must not silently skip its missing history"
        );
        let allowed = vec![project.to_owned()];
        assert!(matches!(
            mac.import_event(pc_id, &event, &allowed).unwrap(),
            ImportResult::Applied { through: 1 }
        ));
        assert!(matches!(
            mac.import_event(pc_id, &event, &allowed).unwrap(),
            ImportResult::Duplicate { through: 1 }
        ));
        assert_eq!(
            mac.message(&message.id).unwrap().draft.body,
            "Pause after the tool"
        );
        let outbound = mac.export_events(project, 0, 10).unwrap();
        assert!(!outbound
            .iter()
            .any(|event| matches!(event.payload, PeerPayload::Message { .. })));
        assert_eq!(
            outbound
                .iter()
                .filter(|event| matches!(event.payload, PeerPayload::Receipt { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn peer_import_rejects_conflicts_gaps_and_unauthorized_events() {
        let dir = tempfile::tempdir().unwrap();
        let pc_id = "11111111-1111-4111-8111-111111111111";
        let mac_id = "22222222-2222-4222-8222-222222222222";
        let project = "33333333-3333-4333-8333-333333333333";
        let recipient = SessionRef {
            machine: mac_id.into(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        let mut pc = Store::open(&dir.path().join("pc.sqlite3")).unwrap();
        let mut mac = Store::open(&dir.path().join("mac.sqlite3")).unwrap();
        pc.set_machine(pc_id).unwrap();
        mac.set_machine(mac_id).unwrap();
        pc.send(
            &Actor::Human {
                machine: pc_id.into(),
            },
            &Draft {
                key: "k".into(),
                project: project.into(),
                to: vec![Target::Agent(recipient)],
                body: "first".into(),
                reply_to: None,
            },
        )
        .unwrap();
        let event = pc.export_events(project, 0, 10).unwrap().remove(0);
        let allowed = vec![project.to_owned()];
        assert!(mac.import_event(mac_id, &event, &allowed).is_err());
        assert!(mac.import_event(pc_id, &event, &[]).is_err());
        let mut gap = event.clone();
        gap.seq = 2;
        assert!(mac.import_event(pc_id, &gap, &allowed).is_err());
        assert!(matches!(
            mac.import_event(pc_id, &event, &allowed).unwrap(),
            ImportResult::Applied { through: 1 }
        ));
        let mut changed = event.clone();
        if let PeerPayload::Message { message } = &mut changed.payload {
            message.draft.body = "changed".into();
        }
        assert!(mac.import_event(pc_id, &changed, &allowed).is_err());
        let mut reused = event.clone();
        reused.seq = 2;
        assert!(mac.import_event(pc_id, &reused, &allowed).is_err());
        assert_eq!(
            mac.connection
                .query_row("SELECT COUNT(*) FROM imported_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn peer_import_and_receipt_replay_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let pc_id = "11111111-1111-4111-8111-111111111111";
        let mac_id = "22222222-2222-4222-8222-222222222222";
        let project = "33333333-3333-4333-8333-333333333333";
        let path = dir.path().join("mac.sqlite3");
        let recipient = SessionRef {
            machine: mac_id.into(),
            incarnation: uuid::Uuid::new_v4().to_string(),
        };
        let mut pc = Store::open(&dir.path().join("pc.sqlite3")).unwrap();
        let mut mac = Store::open(&path).unwrap();
        pc.set_machine(pc_id).unwrap();
        mac.set_machine(mac_id).unwrap();
        let message = pc
            .send(
                &Actor::Human {
                    machine: pc_id.into(),
                },
                &Draft {
                    key: "k".into(),
                    project: project.into(),
                    to: vec![Target::Agent(recipient.clone())],
                    body: "first".into(),
                    reply_to: None,
                },
            )
            .unwrap();
        let event = pc.export_events(project, 0, 10).unwrap().remove(0);
        let allowed = vec![project.to_owned()];
        mac.import_event(pc_id, &event, &allowed).unwrap();
        drop(mac);
        let mut mac = Store::open(&path).unwrap();
        assert!(matches!(
            mac.import_event(pc_id, &event, &allowed).unwrap(),
            ImportResult::Duplicate { through: 1 }
        ));
        let receipt = mac.export_events(project, 0, 10).unwrap().remove(0);
        assert!(matches!(
            pc.import_event(mac_id, &receipt, &allowed).unwrap(),
            ImportResult::Applied { through: 1 }
        ));
        let target = serde_json::to_string(&Target::Agent(recipient)).unwrap();
        let state: String = pc
            .connection
            .query_row(
                "SELECT state FROM delivery_receipts WHERE message_id = ?1 AND target_key = ?2",
                params![message.id, target],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "received_remotely");
        pc.record_peer_ack(mac_id, project, 1).unwrap();
        assert!(pc.record_peer_ack(mac_id, project, 2).is_err());
    }

    #[test]
    fn peer_import_rollback_leaves_no_message_or_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let pc_id = "11111111-1111-4111-8111-111111111111";
        let mac_id = "22222222-2222-4222-8222-222222222222";
        let project = "33333333-3333-4333-8333-333333333333";
        let mut pc = Store::open(&dir.path().join("pc.sqlite3")).unwrap();
        let mut mac = Store::open(&dir.path().join("mac.sqlite3")).unwrap();
        pc.set_machine(pc_id).unwrap();
        mac.set_machine(mac_id).unwrap();
        let message = pc
            .send(
                &Actor::Human {
                    machine: pc_id.into(),
                },
                &Draft {
                    key: "one".into(),
                    project: project.into(),
                    to: vec![Target::Agent(SessionRef {
                        machine: mac_id.into(),
                        incarnation: uuid::Uuid::new_v4().to_string(),
                    })],
                    body: "atomic".into(),
                    reply_to: None,
                },
            )
            .unwrap();
        let event = pc.export_events(project, 0, 10).unwrap().remove(0);
        mac.connection
            .execute_batch(
                "CREATE TRIGGER fail_import BEFORE INSERT ON imported_events
             BEGIN SELECT RAISE(ABORT, 'interrupted import'); END;",
            )
            .unwrap();
        assert!(mac.import_event(pc_id, &event, &[project.into()]).is_err());
        assert!(mac.message(&message.id).is_err());
        assert!(mac.export_events(project, 0, 10).unwrap().is_empty());
        mac.connection
            .execute_batch("DROP TRIGGER fail_import")
            .unwrap();
        assert!(matches!(
            mac.import_event(pc_id, &event, &[project.into()]).unwrap(),
            ImportResult::Applied { through: 1 }
        ));
    }

    #[test]
    fn ended_remote_presence_cannot_reopen_incarnation() {
        let dir = tempfile::tempdir().unwrap();
        let pc_id = "11111111-1111-4111-8111-111111111111";
        let mac_id = "22222222-2222-4222-8222-222222222222";
        let project = "33333333-3333-4333-8333-333333333333";
        let mut mac = Store::open(&dir.path().join("mac.sqlite3")).unwrap();
        mac.set_machine(mac_id).unwrap();
        let mut state = SessionState {
            registration: Registration {
                session: SessionRef {
                    machine: pc_id.into(),
                    incarnation: uuid::Uuid::new_v4().to_string(),
                },
                project: project.into(),
                name: "pm".into(),
                client: "codex".into(),
                native_id: "native".into(),
                process_start: "boot:42".into(),
                eligible: false,
            },
            connected: false,
            ended: true,
        };
        let event = PeerEvent {
            id: uuid::Uuid::new_v4().to_string(),
            origin: pc_id.into(),
            seq: 1,
            project: project.into(),
            payload: PeerPayload::Presence {
                session: state.clone(),
                epoch: 1,
            },
        };
        mac.import_event(pc_id, &event, &[project.into()]).unwrap();
        state.ended = false;
        state.connected = true;
        state.registration.eligible = true;
        let revived = PeerEvent {
            id: uuid::Uuid::new_v4().to_string(),
            seq: 2,
            payload: PeerPayload::Presence {
                session: state,
                epoch: 2,
            },
            ..event
        };
        assert!(mac
            .import_event(pc_id, &revived, &[project.into()])
            .is_err());
        assert_eq!(mac.load_remote_sessions().unwrap().len(), 1);
        assert!(mac.load_remote_sessions().unwrap()[0].ended);
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
