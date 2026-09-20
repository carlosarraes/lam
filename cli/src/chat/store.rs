use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use super::types::{
    Actor, Attempt, ClientEvent, Draft, FeedEvent, Handoff, Limits, Message, Registration,
    SessionRef, SessionState, Target,
};

const SCHEMA_VERSION: u32 = 5;
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
        let now = Utc::now().to_rfc3339();
        transaction.execute("UPDATE delivery_attempts SET state = ?2, outcome_json = ?3, completed_at = ?4 WHERE id = ?1", params![attempt_id, if state == "queued" { "not_submitted" } else { state }, encoded, now])?;
        let ids = transaction.prepare("SELECT message_id FROM delivery_attempt_messages WHERE attempt_id = ?1 ORDER BY ordinal")?.query_map([attempt_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for id in &ids {
            transaction.execute("UPDATE delivery_receipts SET state = ?3, evidence_json = ?4, updated_at = ?5, explicit_retry = CASE WHEN ?3 = 'queued' THEN explicit_retry ELSE 0 END WHERE message_id = ?1 AND target_key = ?2", params![id, target, state, encoded, now])?;
            append_event(
                &transaction,
                id,
                &FeedEvent::Receipt {
                    message_id: id.clone(),
                    recipient: attempt.recipient.clone(),
                    attempt_id: Some(attempt.id.clone()),
                    state: state.into(),
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
    transaction.execute("INSERT INTO events(event_id, project, project_seq, payload_json, kind, message_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![uuid::Uuid::new_v4().to_string(), project, to_sql_integer(sequence)?, serde_json::to_string(event)?, kind, id])?;
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
            assert_eq!(receipt.1, "queued");
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
