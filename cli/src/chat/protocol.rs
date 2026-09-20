use std::io::{Read, Write};

use super::types::{ClientEvent, Draft, Handoff, SessionRef, Target};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub operation: Operation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Register {},
    End {},
    Lifecycle {},
    Send {
        draft: Draft,
    },
    Reply {
        id: String,
        key: String,
        body: String,
        #[serde(default)]
        all: bool,
    },
    Sessions {
        project: String,
    },
    Inbox {
        #[serde(default)]
        cursor: Option<String>,
        limit: u16,
    },
    Show {
        id: String,
    },
    History {
        project: String,
        #[serde(default)]
        cursor: Option<String>,
        limit: u16,
    },
    HistoryTail {
        project: String,
        #[serde(default)]
        before: Option<u64>,
        limit: u16,
    },
    Subscribe {
        project: String,
        #[serde(default)]
        cursor: Option<String>,
        limit: u16,
    },
    Status {},
    Delivery {
        event: ClientEvent,
        epoch: u64,
    },
    Observe {
        event: ClientEvent,
        epoch: u64,
    },
    Finish {
        attempt: String,
        outcome: Handoff,
    },
    Retry {
        id: String,
    },
    SetState {
        eligible: bool,
    },
}

pub fn decode_request(value: Value) -> Result<Request> {
    let request: Request = serde_json::from_value(value)?;
    ensure!(request.version == 1, "unsupported Chat protocol version");
    request.operation.validate()?;
    Ok(request)
}

impl Operation {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Send { draft } => {
                validate_project(&draft.project)?;
                ensure!(
                    !draft.key.is_empty() && draft.key.len() <= 128,
                    "invalid idempotency key"
                );
                ensure!(draft.body.len() <= 65_536, "Chat body exceeds 64 KiB");
                ensure!(
                    !draft.to.is_empty() && draft.to.len() <= 128,
                    "Chat needs 1..128 recipients"
                );
                for target in &draft.to {
                    match target {
                        Target::Agent(session) => validate_session(session)?,
                        Target::Human { machine } => validate_uuid(machine)?,
                    }
                }
                if let Some(id) = &draft.reply_to {
                    validate_uuid(id)?;
                }
            }
            Self::Reply { id, key, body, .. } => {
                validate_uuid(id)?;
                ensure!(
                    !key.is_empty() && key.len() <= 128,
                    "invalid idempotency key"
                );
                ensure!(body.len() <= 65_536, "Chat body exceeds 64 KiB");
            }
            Self::Show { id } | Self::Retry { id } => validate_uuid(id)?,
            Self::Finish { attempt, outcome } => {
                validate_uuid(attempt)?;
                let evidence = match outcome {
                    Handoff::Accepted { receipt } => receipt,
                    Handoff::Unknown { reason }
                    | Handoff::Refused { reason }
                    | Handoff::NotSubmitted { reason } => reason,
                };
                ensure!(
                    !evidence.is_empty() && evidence.len() <= 4096,
                    "invalid native handoff evidence"
                );
            }
            Self::Delivery { epoch, .. } => {
                ensure!(*epoch <= i64::MAX as u64, "invalid observation epoch");
            }
            Self::Observe { event, epoch } => {
                ensure!(
                    matches!(event, ClientEvent::Idle | ClientEvent::Busy),
                    "only native idle/busy observations may wake queue delivery"
                );
                ensure!(
                    *epoch > 0 && *epoch < i64::MAX as u64,
                    "invalid queue observation epoch"
                );
            }
            Self::Sessions { project } => validate_project(project)?,
            Self::Inbox { cursor, limit } => validate_page(cursor, *limit)?,
            Self::History {
                project,
                cursor,
                limit,
            }
            | Self::Subscribe {
                project,
                cursor,
                limit,
            } => {
                validate_project(project)?;
                validate_page(cursor, *limit)?;
            }
            Self::HistoryTail {
                project,
                before,
                limit,
            } => {
                validate_project(project)?;
                ensure!((1..=100).contains(limit), "Chat page limit must be 1..100");
                ensure!(
                    before.is_none_or(|value| value > 0 && value <= i64::MAX as u64),
                    "invalid Chat history boundary"
                );
            }
            Self::Status {}
            | Self::Register {}
            | Self::End {}
            | Self::Lifecycle {}
            | Self::SetState { .. } => {}
        }
        Ok(())
    }
}

fn validate_page(cursor: &Option<String>, limit: u16) -> Result<()> {
    ensure!((1..=100).contains(&limit), "Chat page limit must be 1..100");
    ensure!(
        cursor
            .as_ref()
            .is_none_or(|token| !token.is_empty() && token.len() <= 4096),
        "invalid Chat cursor"
    );
    Ok(())
}

pub fn validate_uuid(id: &str) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(id)?.to_string() == id,
        "Chat ID must be a canonical UUID"
    );
    Ok(())
}

pub fn validate_session(session: &SessionRef) -> Result<()> {
    validate_uuid(&session.machine)?;
    validate_uuid(&session.incarnation)
}

pub fn validate_project(project: &str) -> Result<()> {
    if let Some(local) = project.strip_prefix("local:") {
        let (machine, hash) = local
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid local project ID"))?;
        validate_uuid(machine)?;
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid project root hash"
        );
        Ok(())
    } else {
        validate_uuid(project)
    }
}

pub const MAX_FRAME_BYTES: usize = 1_048_576;

pub fn read_frame<R: Read>(reader: &mut R) -> Result<Value> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    ensure!(
        length > 0 && length <= MAX_FRAME_BYTES,
        "invalid Chat frame length"
    );
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

pub fn write_frame<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    ensure!(
        !body.is_empty() && body.len() <= MAX_FRAME_BYTES,
        "invalid Chat frame length"
    );
    writer.write_all(&(body.len() as u32).to_be_bytes())?;
    writer.write_all(&body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_observation_accepts_only_bounded_idle_or_busy_epochs() {
        for (event, epoch, allowed) in [
            (ClientEvent::Idle, 1, true),
            (ClientEvent::Busy, 2, true),
            (ClientEvent::Hook, 3, false),
            (ClientEvent::Disconnected, 4, false),
            (ClientEvent::Idle, 0, false),
            (ClientEvent::Idle, i64::MAX as u64, false),
        ] {
            assert_eq!(
                Operation::Observe { event, epoch }.validate().is_ok(),
                allowed
            );
        }
    }

    #[test]
    fn internal_fetch_completion_cannot_be_forged_on_wire() {
        assert!(decode_request(
            serde_json::json!({"version": 1, "operation": {"op": "fetch_complete", "ids": []}})
        )
        .is_err());
        assert!(decode_request(serde_json::json!({"version": 1, "operation": {"op": "finish", "attempt": "11111111-1111-4111-8111-111111111111", "outcome": {"Accepted": {"receipt": ""}}}})).is_err());
    }

    #[test]
    fn typed_requests_reject_versions_authority_fields_and_invalid_ids() {
        for value in [
            serde_json::json!({"version": 2, "operation": {"op": "status"}}),
            serde_json::json!({"version": 1, "role": "observer", "operation": {"op": "status"}}),
            serde_json::json!({"version": 1, "operation": {"op": "status", "sender": "Human"}}),
            serde_json::json!({"version": 1, "operation": {"op": "show", "id": "bad"}}),
            serde_json::json!({"version": 1, "operation": {"op": "history", "project": "bad", "limit": 1}}),
            serde_json::json!({"version": 1, "operation": {"op": "history", "project": "a6eebdad-52bb-4f4b-aa7b-1c041efa9091", "limit": 0}}),
        ] {
            assert!(decode_request(value).is_err());
        }
        assert!(
            decode_request(serde_json::json!({"version": 1, "operation": {"op": "status"}}))
                .is_ok()
        );
    }

    #[test]
    fn nested_draft_and_recipient_fields_are_strict() {
        let valid = serde_json::json!({"version": 1, "operation": {"op": "send", "draft": {"key": "one", "project": "22222222-2222-4222-8222-222222222222", "to": [{"Human": {"machine": "11111111-1111-4111-8111-111111111111"}}], "body": "hello", "reply_to": null}}});
        assert!(decode_request(valid.clone()).is_ok());
        let mut sender = valid.clone();
        sender["operation"]["draft"]["sender"] = serde_json::json!("other");
        assert!(decode_request(sender).is_err());
        let mut target = valid;
        target["operation"]["draft"]["to"][0]["Human"]["role"] = serde_json::json!("observer");
        assert!(decode_request(target).is_err());
    }

    #[test]
    fn rejects_oversized_frame_before_reading_a_body() {
        let header = (1_048_577_u32).to_be_bytes();
        assert!(super::read_frame(&mut std::io::Cursor::new(header)).is_err());
    }

    #[test]
    fn frames_round_trip_and_reject_empty_truncated_or_invalid_json() {
        let value = serde_json::json!({"version": 1, "operation": {"op": "status"}});
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &value).unwrap();
        assert_eq!(read_frame(&mut bytes.as_slice()).unwrap(), value);
        for invalid in [
            vec![0, 0, 0, 0],
            vec![0, 0, 0],
            vec![0, 0, 0, 2, b'{'],
            vec![0, 0, 0, 1, b'x'],
        ] {
            assert!(read_frame(&mut invalid.as_slice()).is_err());
        }
        assert!(write_frame(&mut Vec::new(), &Value::String("x".repeat(1_048_576))).is_err());
    }
}
