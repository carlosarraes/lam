#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRef {
    pub machine: String,
    pub incarnation: String,
}

/// Internal adapter contract: native identity and process evidence must already
/// have been validated by the selected client's adapter, never by caller JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub session: SessionRef,
    pub project: String,
    pub name: String,
    pub client: String,
    pub native_id: String,
    pub process_start: String,
    pub eligible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionState {
    pub registration: Registration,
    pub connected: bool,
    pub ended: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Actor {
    Agent(SessionRef),
    Human { machine: String },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Target {
    Agent(SessionRef),
    Human { machine: String },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub key: String,
    pub project: String,
    pub to: Vec<Target>,
    pub body: String,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub id: String,
    pub sender: Actor,
    pub sender_seq: u64,
    pub created_at: String,
    pub draft: Draft,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub inline_bytes: usize,
    pub batch_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenderedBatch {
    pub text: String,
    pub full_ids: Vec<String>,
    pub preview_ids: Vec<String>,
    pub overflow_count: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Attempt {
    pub id: String,
    pub recipient: SessionRef,
    pub batch: RenderedBatch,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Handoff {
    NotSubmitted { reason: String },
    Accepted { receipt: String },
    Refused { reason: String },
    Unknown { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientEvent {
    Idle,
    Busy,
    Hook,
    Disconnected,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeedEvent {
    Message {
        message: Message,
    },
    Receipt {
        message_id: String,
        recipient: SessionRef,
        attempt_id: Option<String>,
        state: String,
        outcome: Option<Handoff>,
    },
    Exposure {
        message_id: String,
        recipient: SessionRef,
        state: String,
    },
    Fetched {
        message_id: String,
        recipient: SessionRef,
    },
}
